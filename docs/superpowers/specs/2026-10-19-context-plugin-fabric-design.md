# Aleph Context Fabric：Capability 化上下文编排与第三方压缩插件设计

- **日期**：2026-10-19
- **状态**：待用户批准（approval required before implementation）
- **范围**：上下文 Admission、Lifecycle、恢复/检索、内置压缩与第三方插件接入
- **基线**：Aleph 当前 `src/context/*`、`src/tool_output/*`、`src/extension/*` 与已有 Context Fabric 实现
- **参考**：`context-mode`、`billion-context`、pi、pi-ctx

## English abstract

Aleph needs a composable context fabric rather than a single hard-coded compactor. Tool output should be reduced before it enters the model context, oversized history should be compressed incrementally and recoverably, and raw hidden reasoning should not inflate persisted or replayed context. Aleph's built-in implementation remains the default best implementation, while third-party context systems such as billion-context can replace or compose with individual phases through a stable capability contract.

The governing design principle is: **Everything externally composable is a Capability.** Any behavior that an Agent, Runtime, or another system must discover, invoke, replace, subscribe to, or manage is a Capability. Context admission, compaction, restoration, search, status, lifecycle hooks, and plugin ownership therefore need explicit descriptors, stable contracts, and observable lifecycle rather than private wiring.

The session event log remains Aleph's durable source of truth. Plugins may propose bounded transformations or derived artifacts, but they cannot silently become a second event store or mutate session history outside the core's atomic write path.

## 1. Intent and problem statement

用户希望解决四个相互关联的问题：

1. 工具输出携带大量对后续推理无价值的日志、重复内容和原始大结果，导致上下文快速膨胀。
2. 思维链或中间推理信息进入持久化、重放或压缩路径后，消耗上下文预算并掩盖真正需要保留的事实。
3. Aleph 当前的压缩时机、阶段划分和恢复路径仍然分散；已有 Admission 与 Lifecycle 能力没有统一的编排入口。
4. Aleph 内置压缩应当是默认最佳方案，但用户不能被锁定在单一实现中；billion-context 等第三方工具需要能够替换或组合 Aleph 的上下文能力。

目标不是增加一个孤立的“压缩模块”，而是把现有能力收敛为可发现、可替换、可组合、可观测的 Context Fabric。

## 2. Current state and gap analysis

| 维度 | 参考项目提供的启发 | Aleph 当前状态 | 结论 |
|---|---|---|---|
| 工具输出裁剪 | context-mode 的 byte/char/JSON 安全边界与内容外置 | `src/tool_output/structured/*`、`hygiene`、`result_store`、`ctx_search` 已有结构化裁剪和恢复入口 | 保留并统一为 Admission Capability |
| 阶段性压缩 | billion-context 的增量 fold、可寻址恢复、search/status、分层 nudge | `ContextCompactor`、`FoldRecorded`、`session_decompress`、fold economics、增长 nudge 已存在 | 主要缺少统一 facade 与外部能力契约 |
| 生命周期接入 | pi/context-mode 的 session/tool/context/compact 生命周期事件 | Aleph 有 extension hooks 与 compaction 路径，但 context hook 尚未成为独立能力面 | 新增 Context Lifecycle Capability |
| 插件接入 | pi adapter 通过宿主事件注册；billion-context native/standalone 二选一避免双压缩 | `PreflightStage` 是 trait，但 `default_pipeline` 硬编码；extension 系统没有 context capability | 将注册、冲突检测、组合策略提升为 Capability registry |
| 恢复一致性 | billion-context 分离原文、fold、refs、checkpoint、plugin state | Aleph 以 `session_events` 为唯一真源，`Retire::Through` 保留检索数据 | 不复制第二事件库；插件状态必须是派生/可重建状态 |
| 缓存稳定性 | 动态恢复内容追加到消息尾部、避免修改 system prompt | Aleph 已有 fresh tail、cut quantum、minimum savings 和 read-family 回读通道 | 契约固定为工具结果/动态尾部，不改稳定前缀 |
| 进程生命周期 | context-mode 父进程存活、stdin EOF、空闲回收、in-flight 保护 | Aleph extension/MCP/WASM 已有生命周期基础，但 context plugin 尚未声明这些约束 | 复用现有 extension runtime，补 context plugin 的资源预算和清理契约 |

参考源码受 Windows 当前环境限制：`T:\Github\` 未挂载；本设计以本机可访问的 `C:\Users\zou\.pi\agent\npm\node_modules\context-mode` 与 `billion-context` 为行为参考。该限制只影响参考源码来源，不改变 Aleph 的设计目标。

## 3. Goals

- 保留并整合现有 Admission：工具结果在进入模型上下文前完成内容感知、预算受控、UTF-8/JSON 安全的裁剪；大结果保留可检索恢复入口。
- 保留并整合现有 Lifecycle：压力感知、阶段性压缩、Fold registry、decompress、search、status、fold economics 和增长 nudge。
- 建立 `ContextFabric` facade，统一暴露 Admission、Lifecycle、Recovery、Search、Status 和 Lifecycle Hook capabilities。
- 让第三方 context plugin 能按 capability 声明接管单个阶段、整个阶段，或和内置能力按明确顺序组合。
- 内置实现默认启用；未配置插件时行为保持当前 Aleph 语义。
- 所有可外部组合能力有稳定 descriptor、版本、权限范围、预算、错误策略和可观测结果。
- 普通隐藏推理默认不持久化、不进入插件输入、不被上下文重放；用户确认的兼容性例外是：Core 按目标提供商协议保留续跑必需的推理内容或签名，插件、摘要和通用检索仍不得读取。保留用户消息、助手最终答复、工具调用/结果和完成任务所需的状态事实。 / Ordinary hidden reasoning is dropped by default. Core alone retains reasoning or signatures required by the target protocol for resumable tool turns; plugins, summaries and general retrieval cannot access these artifacts.
- 保持 `session_events` 为唯一持久化真源，保持 Core/Interface 分离和 harness 12 文件棘轮。

## 4. Non-goals and anti-goals

- 不复制第二份 session event store、messages truth source、MCP server 或 CDP/终端实现。
- 不把插件变成第二个 Agent loop，也不让插件替代 LLM 做意图识别、任务评估、路由或恢复策略选择。
- 不允许插件直接修改系统 prompt 的稳定前缀；恢复内容走已有普通工具结果或动态尾部路径。
- 不把所有已有 context 文件搬到新目录；Facade 是接线层，目录重组要有独立收益和消费者。
- 不同时启用两个拥有同一 phase 的压缩 owner；组合必须由显式 pipeline policy 允许并可观测。
- 不以原始思维链作为第三方插件的默认数据源。

## 5. Governing principle: Everything externally composable is a Capability

以下对象必须成为显式 Capability，而不能只存在于私有函数或硬编码 match：

| Capability | 需要支持的外部动作 |
|---|---|
| `context.admission` | discover、invoke、replace、subscribe result/metrics |
| `context.preflight` | register ordered stage、enable/disable、invoke |
| `context.compaction` | discover strategy、invoke、replace owner、observe result |
| `context.recovery` | restore bounded range、paginate、write-to-file |
| `context.search` | search folded/offloaded context |
| `context.status` | inspect pressure、folds、plugin health、cache/economics |
| `context.lifecycle` | subscribe before/after events、dispose cleanly |

每个 descriptor 至少包含：`id`、`api_version`、`implementation_version`、支持的 phase、输入/输出 contract、session/project namespace、最大输入/输出预算、是否能持久化、错误策略、资源限制与 owner 信息。descriptor 是唯一能力声明来源；registry、工具目录、MCP/ACP projection 和诊断都从它派生，禁止各持一份 shape。

## 6. Proposed architecture

### 6.1 Core facade

在 `src/context/` 增加一个窄 facade（实现期先验证是否能放入现有模块；只有存在真实消费者才增加文件）：

- `ContextFabric`：持有内置 Admission/Lifecycle、Capability registry、pipeline policy、plugin health 与观测 sink。
- `ContextFabricBuilder`：由 Runtime 组装，注入已有 `PreflightPipeline`、`ContextCompactor`、`ContentIndex`、session event reader 和 extension adapters。
- `ContextCapabilityDescriptor`：serde/schemars wire shape，作为 capability、tool catalog、MCP projection 和 diagnostics 的共同来源。
- `ContextPhase`：`Admission`、`Preflight`、`Compaction`、`Recovery`、`Search`、`Status`、`Lifecycle`。
- `ContextPipelinePolicy`：每个 phase 选择 `Builtin`、`Replace { owner }` 或有序 `Compose { stages }`。

Facade 只编排和校验，不保存第二份历史，不将插件输出直接写入 event store。内置实现仍通过现有模块完成真实工作。

### 6.2 Plugin contract

对 Rust 内部实现使用 trait；对 WASM、MCP、Node/HTTP 或其他进程实现使用稳定的 serde JSON contract，避免把 Rust ABI 当成插件 ABI。核心操作形状为：

- `describe(ContextCapabilityQuery) -> ContextCapabilityDescriptor`
- `admit(ContextEnvelope, AdmissionBudget) -> AdmissionResult`
- `preflight(ContextSnapshot, PreflightBudget) -> TransformResult`
- `compact(ContextSnapshot, CompactionRequest) -> CompactionResult`
- `restore(RestoreRequest) -> BoundedContextPage`
- `search(ContextSearchRequest) -> SearchPage`
- `status(ContextStatusRequest) -> ContextStatus`
- `shutdown(ShutdownReason) -> DisposeReport`

`ContextSnapshot` 只包含插件声明 scope 内可见的数据；默认去掉隐藏 reasoning、凭据、未授权项目文件和其他 session。所有结果必须携带 `input_fingerprint`、`output_bytes/tokens`、`strategy`、`continuation`（若分页）、`quality` 和 `degraded/fallback` 信息。

插件不得返回“已压缩”文字来改变 Aleph 状态。只有核心在验证结果效果、写入必要的 `FoldRecorded` 或提交合法 projection 后，状态才发生改变。

### 6.3 Replace and compose semantics

- `Builtin`：默认。现有 Aleph pipeline 完整运行。
- `Replace`：插件独占一个明确 phase；必须实现该 phase 的 required capability，失败或契约不合法时回退到内置实现并记录 degraded outcome。
- `Compose`：按 descriptor 声明的顺序运行。每个阶段声明输入/输出类型和是否可重复；禁止两个 stage 同时声称拥有最终 compaction owner。每个 stage 的 token delta、缓存影响和错误都记录。
- `Bundle owner`：允许一个插件以原子 bundle 同时拥有彼此耦合的 `compaction`、`recovery`、`search`、`status` 等 phase；bundle 必须整体完成 capability、scope、budget 和 health 校验，不能把 bundle 拆成互相矛盾的 owner。
- `Observer`：只订阅事件、指标或索引，不改变上下文。
- `Interceptor`：只有声明了对应 phase 的变换能力才可修改候选数据；不能越权写 session event。

启动时完成 owner 冲突检查；运行时拒绝未声明 phase、超预算、越 scope 或违反 output contract 的插件。Admission 失败使用内置 bounded fallback，避免原始大输出直接进入上下文；Lifecycle 失败保留原文和内置 compactor 可继续运行。所有 fallback 都让 Agent/诊断可见。

### 6.4 Lifecycle and persistence

生命周期事件至少覆盖：`session_start`、`tool_result_admitted`、`before_preflight`、`after_preflight`、`before_compaction`、`after_compaction`、`fold_recorded`、`restore_requested`、`turn_end`、`session_shutdown`。

插件状态按 project/session namespace 隔离，使用现有 extension lifecycle/disposer 和存储 resolver。插件 checkpoint、引用和缓存是派生状态；若插件需要原文，核心通过版本化 snapshot 或已有 event reader 提供。任何需要跨重启恢复的事实必须仍能从 `session_events` 和既有索引重建。

恢复继续走 `session_decompress`/普通工具结果通道，支持有界分页和 `toFile`，不新增瞬态注入路径，不修改 system prompt 稳定前缀。

### 6.5 Reasoning and output policy

- provider 返回的普通隐藏 reasoning 不进入 session event；目标协议强制回传的内容或签名作为 Core 内部协议数据保留。所有推理内容和签名均从 ContextSnapshot、第三方插件、摘要输入和通用检索投影中剔除，不向插件开放额外授权入口。 / Persist only provider-required protocol artifacts inside Core; all reasoning and signatures are excluded from plugin snapshots, summary inputs and general retrieval projections.
- 工具调用参数、工具结果、用户消息、助手最终输出和必要的运行状态按现有事件语义保留。
- tool output 先经过 Admission；结构化 JSON、日志、搜索、diff 等类型继续由现有 reducer 处理。
- 每个插件结果必须有 byte/token upper bound；字符串边界使用统一安全 API，不能各自实现 UTF-8/UTF-16/JSON 截断。
- Context Fabric 不判定用户任务“是否完成”，不替 LLM 选择恢复策略；它只提供压力、候选、恢复和能力状态。

## 7. Wiring plan

1. 先在 `src/context/` 建立 facade 和 capability wire shape，接入现有 `default_pipeline`、`ContextCompactor`、`ContentIndex`、fold/recovery/status，不改变默认行为。
2. 将 `PreflightStage` 从硬编码构造提升为由 facade/registry 组装的 ordered capability；内置 stages 以 first-party registrations 提供。
3. 在现有 `src/extension/` capability/registry/lifecycle 中增加 context plugin registration 和 adapter boundary，复用 MCP/WASM/static runtime，不新建第二套插件系统。
4. 增加 owner conflict、scope、budget、fallback、lifecycle disposal 和 stable wire shape tests。
5. 接一个最小 external adapter fixture，先验证 `Observer` 和 `Admission`，再验证 `Replace/Compose`；billion-context adapter 作为协议兼容性验证对象。
6. 更新 `FEATURE_LOCATOR.md`、相关 reference 文档和 `AGENTS.md`/`CLAUDE.md` 中确实改变的 Capability 事实；不复制代码中的单一事实。

## 8. Verification surface

- `cargo fmt --check`
- `cargo check`
- `cargo clippy -- -D warnings`
- `cargo test -p alephcore --lib`
- context domain tests covering admission, preflight, compaction, restore, owner conflicts, fallback and lifecycle disposal
- effect assertions: bounded output arrives in the next prompt; restore pages concatenate to the exact source span; failed plugin leaves the built-in path usable; assistant text cannot create a fold; duplicate owners are rejected before invocation
- prefix-cache stability tests: plugin output and restoration do not rewrite the stable system prefix
- extension and real-machine plugin QA where the changed capability path requires it
- `aleph-server prompt-size` if prompt/catalog descriptions change

## 9. Risks and decisions

- **Double compression**：descriptor owner and startup conflict check are mandatory; native/standalone billion-context modes cannot both own the same phase.
- **Plugin failure**：Admission falls back to bounded built-in processing; Lifecycle preserves source and falls back to built-in compaction. No failure may silently pass raw unbounded output.
- **Wire drift**：one `ContextCapabilityDescriptor` and one protocol module feed all projections; no copied DTOs in MCP/ACP/extension layers.
- **Cache loss**：dynamic restore goes through tool result or tail, never system prompt; pipeline policies expose cache impact in status.
- **Scope escalation**：session/project namespace and capability declaration are checked before data is sent; plugin runtime limits and disposer are required.
- **Overbuilding**：the first implementation targets the existing consumers and one external fixture. New phase types, cross-session lineage, universal scheduler and raw reasoning plugins remain separate proposals.

## 10. Decisions from builder check

DeepSeek Flash 的独立 builder check 提出了会改变实现结构的问题；本设计已按用户选择固定如下：

1. **权限**：宿主显式授予 namespace/scope；插件只声明需求；默认当前 session 隔离，project scope 不得跨 session，跨 session 读取必须单独授权并审计。
2. **旧 fold**：Core 按 `owner/provenance` 路由历史 fold；owner 切换不自动要求新插件读取旧 owner 的 fold。
3. **故障与组合**：Replace 插件失败或超时，当次调用立即使用 bounded built-in fallback；允许耦合 phase 以原子 bundle 由一个插件拥有。
4. **缓存与副作用**：允许 namespace 内可重建缓存/checkpoint/index，但不得成为真源；原文不得越过 Core retire/forget 边界；网络出口和项目目录写入默认拒绝，须显式 capability + 宿主授权 + 审计。
5. **协议必需推理例外（实施中用户确认）**：保留 Core 内部续跑所必需的提供商推理内容或签名，避免 Anthropic / DeepSeek 工具回合因缺字段而被拒绝；普通推理仍默认剔除，所有推理均不得进入插件、摘要或通用检索。 / User-approved compatibility exception: retain provider-required reasoning only inside Core for resumable tool turns.

## 11. Builder handoff

实现者仍需在批准后根据当前代码确认：

- facade 的最小文件落点是否能放入 `src/context/mod.rs`/现有模块而不制造空 abstraction；
- `src/extension/` 的现有 capability registry 和 MCP/WASM lifecycle 哪些接口可直接复用；
- `PreflightStage` 的运行时注册是否需要新 registry，还是可由现有 ContextFabricBuilder 注入；
- external adapter 的最小 transport 是现有 MCP runtime、WASM host，还是一个静态测试 backend；
- 每个 phase 的实际持久化边界和 error fallback 是否与当前 session event contract 完全一致。

这些是实现调查项，不改变本设计的用户目标、Capability 原则、默认内置实现、可替换/可组合插件要求和单一事件真源约束。

**Implementation gate:** this document must be approved before product code, dependency changes, plugin scaffolding, or implementation skills are invoked.
