# Aleph 运行时模型热切换设计

日期：2026-10-18
状态：待用户审批

## 意图

用户希望在 Aleph 执行任何任务的任意时刻切换模型，并让切换后的下一个 LLM 请求立即使用新模型。目标问题是：当前 Aleph 已有会话模型 pin、单次请求 override 和 provider 动态选择，但会话模型通常在 run 创建时绑定，因此 CLI/Panel 的选择无法可靠地影响正在运行的 Think→Act 循环。

目标行为是：当前正在传输的 provider 请求保持自己的不可变快照；操作者发出的切换在并发边界安全地发布；当前请求结束后，下一次 LLM 请求重新读取并解析最新选择。切换需要同时由 CLI/命令入口和 Panel UI 触发，两者必须共用 Rust Core 的同一写入口。

## 目标

1. 在 active run 内支持模型切换，下一次 LLM 请求使用新的 `provider/model`。
2. 保证单个 provider 请求内的 model、provider、认证引用、能力和 route generation 来自同一快照，不出现 torn state。
3. 复用 `SessionModelHandle` / `SessionPinSink`、`RouteHandle`、`SessionRunRegistry`、现有 provider registry 和 session store，不建立第二套模型事实源。
4. CLI 和 Panel 通过同一 session-scoped control seam 修改选择，并通过现有 JSON-RPC/Gateway event seam 获得结果和状态变化。
5. 对显式 `provider/model`、裸 model id 歧义、未认证、未知模型和不支持能力给出可审计的 fail-closed 诊断。
6. 保留模型选择、当前请求实际模型和路由/失败信息之间的 provenance，便于恢复、审计和 UI 展示。
7. 清理因旧的 run-bound model 假设而产生的冗余分支、死代码和重复入口，并补齐 `FEATURE_LOCATOR.md` 及相关 reference 文档。

## 非目标

- 不在本次工作中实现自动 failover、基于 prompt 的任务分类、关键词路由或新的 virtual model router。
- 不在 provider 请求已经开始后强行中断 HTTP streaming；切换从下一个请求边界生效。
- 不把 session state、route diagnostics、auth 信息或 model change 记录自动塞进 provider prompt。
- 不把 provider/auth/catalog 责任集中成一个新的全能 runtime。
- 不把业务逻辑放进 Panel、CLI、Channel 或其他 Interface 层。

## 失败条件

即使代码能运行，下列结果也视为失败：

- CLI 和 Panel 各自维护模型状态；
- 只修改 run 开始时的 pin，导致 active run 仍继续使用旧模型；
- 一个请求从不同 generation 读取 provider、model 或 auth；
- provider 变更时沿用旧 provider 的凭据；
- 模型解析遇到歧义时静默选择第一个；
- 为实现切换而把复杂决策塞进 `src/harness/`，扩大 R10 harness 边界。

## 当前状态与差距

| 维度 | Aleph 当前能力 | 参考项目 Pi 的可迁移语义 | 本次映射 |
|---|---|---|---|
| 会话选择 | `SessionModelHandle` / `SessionModelPin` 已提供 pin、hydrate、clear 和 durable sink | `agent.state.model` 与 `model_change` entry 分离 | 保留现有 handle；补充 live generation 和明确的 control API |
| 请求选择 | `pick_llm`、`BrainRef`、`RunRequest.model_override` 已能做 run/单次 override | `prepareRequest` 每次请求解析 session selection 到物理模型 | 在 bridge request boundary 生成 `ResolvedRequestTarget` |
| active run | `rehydrate_turn_model_pin` 在 run 开始恢复 pin，当前循环不能保证重新读取 | `user` / `continuation` / `retry` 在请求边界区分 | 只在每次 LLM 请求前读取 live selection；不改 harness 循环语义 |
| 并发发布 | `RouteHandle` 已使用完整 route snapshot；`SessionRunRegistry` 管理 run claim | runtime route 结果只覆盖当前请求 | 用同样的 snapshot 原子发布模型选择 |
| 控制入口 | 已有 `select_model` 工具和 Panel model picker；sessions.patch 当前拒绝 model 字段 | Pi RPC 有 `set_model`、`get_state`、异步 event | 新增/接通一个 session model control seam，CLI 与 Panel 都调用它 |
| 解析安全 | provider registry、model catalog、默认 provider 已存在 | `provider/model` 优先；歧义拒绝；auth 分层 | 保持 registry 简单，在 resolver 返回 `Resolved/Ambiguous/Unknown/Unauthenticated/Unsupported` |
| 状态传播 | Gateway/session event 与现有 JSON-RPC 基础设施存在 | command/response/event 分流 | 发布 model selection、effective request target、诊断和 correlation id |
| Harness 边界 | R10 要求薄 harness、12 文件棘轮 | Pi 将 projection/routing 放在 request seam | 以 bridge/provider adapter 完成动态解析，不在 `src/harness/` 增加智能策略 |

## 设计

### 1. 三层模型

保持三层职责：

- `SessionModelSelection`：session-scoped 的用户/工具选择，含 provider、model、source 和 process-local generation。live handle 是进程内事实，现有 durable sink 负责恢复；hydrate 不能覆盖已经发生的 live update。
- `ResolvedRequestTarget`：在每个 LLM request boundary 读取 selection、route snapshot、provider availability、auth reference 和 model capability，解析成一次不可变目标。解析失败必须返回结构化诊断。
- Execution：provider executor 只接收已解析目标和 provider-visible context，不再在执行中猜测模型或从多个全局状态拼装目标。

`RouteHandle` 的完整 snapshot 语义用于 route generation；模型选择发布也必须作为一个完整值发布。generation 只用于并发/观测，不作为跨重启的业务值。

### 2. active run 的请求边界

在 `src/orchestrator/harness_bridge/llm.rs` 与其现有调用 seam 中复用 `pick_llm` 的 provider 选择逻辑，增加一个 session-aware LLM adapter 或等价的已有 `model_override_provider` 扩展：

```text
每次 LLM request
  -> load session selection snapshot
  -> load route snapshot
  -> resolve provider/model/auth/capability
  -> build immutable ResolvedRequestTarget
  -> reuse existing context/tool projection
  -> call exactly that provider target
  -> emit actual provider/model + generation provenance
```

adapter 不拥有新的 catalog、auth cache 或 session map，也不改变 Think→Act 的机械循环。当前请求开始后，selection 更新不会改变该请求；下一次 adapter 调用重新读取 selection，因此满足“下一个请求生效”。

provider 切换时必须重新执行 auth 和 capability resolution，禁止将旧 provider 的凭据传给新 provider。模型 context window、tool-use 支持、reasoning 参数等约束应在 request target resolve / 现有 context preparation seam 中验证；若新模型无法容纳当前历史，复用已有 context compaction，再按新模型能力重新准备请求；压缩结果和失败原因必须可观测，禁止静默丢历史或改变工具语义。

### 3. 统一控制入口

增加或补齐 session-scoped 的 Core 控制操作，语义类似：

```text
select_model(session_key, provider/model, source) -> ModelSelectionResult
clear_model_selection(session_key) -> ModelSelectionResult
get_model_state(session_key) -> ModelState
```

控制操作执行权限、canonical provider/model 解析、选择时的 auth/capability/context 基础校验和 `SessionPinSink` 持久化；未知、歧义、未认证或当前不支持的选择在写入前拒绝，保留原有有效选择。请求开始前仍需重新校验实时可用性，因为认证轮换、provider 下线或能力变化可能发生在选择之后。CLI、Panel、Channel 和模型工具只做输入输出适配。Panel 不直接操作 provider，CLI 不复制 resolver。

当前 `select_model` 工具继续走这个控制操作。Panel model picker 和 CLI 命令都调用同一 RPC/Gateway seam；成功后发布带 session key、selection generation、canonical target 和 diagnostics 的异步 state event。命令 response 与 event 分离，避免 UI 轮询或把异步状态伪装成同步返回。

### 4. 选择与实际请求 provenance

模型选择记录的是长期意图；每次 assistant 请求的实际 provider/model 是执行事实。两者都必须通过现有 session store/event projection 跨重启持久化，以便恢复后审计切换和实际请求模型。需要区分：

- 用户/工具选择的 session model；
- 当前请求解析后的实际 target；
- provider failure、unsupported、unauthenticated 或其他 resolve 原因。

只有影响恢复和状态的 entry 进入 durable state；这些 entry 不自动进入 LLM prompt。若现有事件类型不足，只新增最小的 model selection / effective target 事件，并为其定义投影，不增加第二份 map。

### 5. CLI/Panel 状态一致性

两类入口必须通过同一个 `session_key` 和 control seam。Panel 订阅现有 Gateway event；CLI 使用同一 JSON-RPC/命令协议，response 带 correlation id，异步 event 带 session key 和 generation。session 切换、恢复、fork 或 rebind 时必须重新绑定状态订阅，避免 UI 显示旧 session 的模型。

## 具体改动范围

实现前由工作树中的代码核对最终确认，但预期集中在以下边界：

- `src/providers/session_model_handle.rs`、`src/gateway/session_model_pin.rs`：live selection snapshot、generation、统一 control 语义和持久化边界。
- `src/orchestrator/harness_bridge/llm.rs`、`src/orchestrator/harness_bridge/runner_impl.rs`：每请求解析和现有 provider 选择的接线；必要时复用 `src/providers/model_override_provider.rs`。
- `src/gateway/execution_engine/run_loop/inner.rs`、`src/gateway/execution_engine/turn_model.rs`：保留恢复语义，移除对 active run 固定模型的错误假设。
- 现有 JSON-RPC/Gateway handler、CLI command adapter、Panel model picker/Bridge adapter：接入统一 control seam，不在接口层实现业务逻辑。
- `src/builtin_tools/select_model.rs` 及相关 model catalog 工具：调用统一 control API，补齐错误和状态返回。
- 相关单元/集成测试与 `docs/reference/FEATURE_LOCATOR.md`、`docs/reference/SESSION_KNOBS.md`、架构/开发 reference：记录实际完成边界和未完成项。

若实现需要超过上述边界，必须先检查是否违反 R1/R2/R4/R7/R8/R9/R10，并优先寻找已有 seam。

## 验收标准

1. active run 连续发出至少两个 LLM 请求时，在两次请求之间通过 CLI 或 Panel 切换 `provider/model`，第二个请求命中新 target。
2. 切换发生在第一个请求 streaming 期间时，第一个请求仍使用其起始 snapshot，第二个请求使用新 snapshot。
3. CLI 与 Panel 的选择结果、generation、持久化状态和事件观察一致。
4. 同一请求的 provider/model/auth/route generation 不会来自不同版本；并发切换压力测试不产生 torn state。
5. provider 变更重新解析认证和能力；旧凭据不会泄漏到新 provider。
6. 未知、歧义、未认证、不支持的模型均 fail-closed，并返回可供 CLI/Panel 展示的诊断。
7. 工具调用历史、tool result、上下文压缩和预算行为保持可验证；模型能力不足时给出错误，不静默丢历史或改变工具语义。
8. 现有 `select_model`、session 恢复、run registry、provider catalog 和相关回归测试通过。
9. 运行适用的 `cargo fmt --check`、`cargo check`、相关 `cargo test`，并在必要时运行 clippy；文档同步且没有死代码/重复事实源。

## 方案取舍

选择“每请求 snapshot + Core 统一 control seam”，因为它满足 active run 生效和并发一致性，同时复用 Aleph 已有的 session pin、route snapshot 和 provider traits。它使每次 LLM 请求都要经过一次解析和 capability/auth 校验，增加少量请求前开销；代价是需要把当前 run-bound model 依赖从 bridge 中拆开。这个代价换来跨 provider 切换的正确凭据边界、可审计 provenance 和 CLI/Panel 的一致性。

不采用把整个 provider executor 在后台替换的方案，因为那会让正在执行的请求看到可变对象、难以证明 snapshot 一致性，并容易产生第二套生命周期/状态管理。也不采用在 harness 内读取全局状态的方案，因为它会扩大 R10 边界并把模型选择业务逻辑放入循环。

## 已确定的产品决策

- **选择时拒绝**：未知、歧义、未认证或当前不支持的模型在 control seam 写入前拒绝，旧的有效选择继续生效；请求边界仍会做实时复核。
- **同一 session 全部生效**：同一个 `SessionKey` 下的主 run、后台 run 和其他 run 都在自己的下一次 LLM 请求边界读取新选择；已有 `RunRequest.model_override` 等更具体的 run 级 override 仍按既定优先级优先。
- **复用现有压缩**：新模型上下文窗口更小时，走现有 context compaction 和请求准备路径；压缩不能静默丢弃历史或工具语义。
- **跨重启审计**：session selection、每次请求的 effective provider/model、generation 和切换/解析原因都进入现有 session event/store projection，并在恢复后可查询；它们不自动进入 provider prompt。

## 留给实现者的决定

- 根据现有 trait 签名选择 adapter 的最小落点，避免新增不必要的公共 trait。
- 判断现有 session event 是否足以记录 selection/effective target；不足时增加最小事件及迁移/投影。
- 选择已有 JSON-RPC/Gateway method 的扩展形式，并保持现有 protocol invariant。
- 根据实际 provider capability/context preparation 代码确定选择时预校验与请求时复核的具体位置。

## 已知未验证项

本设计已完成产品决策，当前仍未运行构建或测试；`T:/Github/pi` 的参考实现已只读核对。实际改动前仍需在 worktree 中确认 `model_override_provider.rs`、现有 RPC/CLI 注册路径、Panel Bridge 调用路径及 session event projection 的最新 trait 细节。
