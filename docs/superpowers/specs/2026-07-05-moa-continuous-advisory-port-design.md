# MoA 连续咨询移植设计 (Hermes MoA → Aleph Virtual Provider Facade)

- **日期**: 2026-07-05
- **状态**: 设计已确认（brainstorming 三节逐节确认）
- **第二轮修订**: [2026-07-05-moa-round2-optimization-design.md](2026-07-05-moa-round2-optimization-design.md)（8 修复 / 3 连线 / 3 增强 / 重构与测试补齐）
- **参考实现**: hermes-agent (`/Volumes/TBU4/Github/hermes-agent`) — `agent/moa_loop.py` (1058L), `agent/moa_trace.py`, `hermes_cli/moa_config.py`, `hermes_cli/moa_cmd.py` + 14 个测试文件钉死的行为契约
- **任务目标**: 完整移植 hermes MoA（Mixture of Agents）模块——架构映射而非照抄，利用 Rust/Tokio 在类型安全与并发上对齐并超越参考实现

---

## 0. 一句话

MoA = 每个 Think 迭代前，把当前对话状态的「顾问视图」并行发给 N 个无工具的 advisor 模型咨询，把它们的建议作为私密指导追加到聚合器（= 行动模型）prompt 末尾；聚合器照常行动（工具、thinking、最终回复）。**整个机制藏在 provider 层门面里，agent loop 完全无感知。**

## 1. 术语映射

| hermes | Aleph | 说明 |
|---|---|---|
| reference models | **advisors**（顾问） | 无工具咨询调用 |
| aggregator | **聚合器** | 就是行动模型，工具/流式/全量转写归它 |
| `MoAClient`（OpenAI 兼容 client 门面） | `MoaProvider`（`AiProvider` 门面） | agent loop 无感知的结构本质 |
| `/moa <prompt>` one-shot + 模型选择器虚拟 provider | `/moa` one-shot + `moa` 工具 session sticky | 选择器集成后置（用户已决定） |
| `config.yaml [moa]` presets | `config.toml [moa]` presets（typed + schemars） | |

## 2. 已确认的关键决定（用户澄清）

1. **触发形态 = 命令 + 配置工具**：`/moa` 命令（开销由用户显式决定，刻意不让模型自主开启）+ `moa` builtin 工具按 R8 提供对话式 preset 管理。
2. **并发语义 = 等全 + 超时预算**：默认等全体 advisor（聚合器拿完整顾问集，行为可预测，对齐 hermes），新增 per-preset `advisor_timeout_secs`（hermes 无超时控制）；超时 advisor 降级为标签注记。不做 K-of-N 竞速。
3. **激活面 = 工具为主，选择器后置**：v1 不做 Panel 模型选择器 / `select_model` 的 "moa" 虚拟 provider 集成。
4. **架构 = 方案 A 虚拟 Provider 门面**（备选 B 执行引擎注入、C 扩展 subagent 工具均被否：B 做不了 per-iteration 节奏且永久污染转写；C 不是移植，撞模型自我表扬问题）。
5. **advisor 用量不混入 `ProviderResponse.usage`**（保 context gauge 诚实，防误触发压缩），另发汇总事件补可见性。
6. **每回合显式模型覆盖（panel model_override）优先于 MoA**：用户显式选模型的回合跳过 MoA。**实施期修订（2026-07-05 验证）**：gateway 每回合 `model_override` 在 harness 路径上只进 `ModelResolved` 事件与健康上报，从不到达 runner Step 3（`FlowRequest` 无模型字段）——该覆盖今天对线上模型本就无效。故优先级实现为 MoA > select_model > agent pin > brain；model_override 交互留待其管道补通时挂钩。
7. **默认 `fanout = "per_iteration"`** 对齐 hermes；`"user_turn"` 同步移植。
8. **不硬编码默认 preset 模型**（hermes 内置 gpt-5.5/deepseek/opus 默认，Aleph 各安装 provider 不同）：无 preset 时报带指引的错误，工具对话式建 preset。
9. **`/moa` 排除出 L0 fast path**（照 `/loop` 先例，`slash_command.rs:75-77`）：one-shot 要把余下文本当提示词发起完整 run。

## 3. 与现有能力的关系（防重复建设）

- **`subagent` 工具的一次性 MoA**（`proposer_models` + `synthesize` + `aggregator_model`，`src/agents/subagent_tool/`，引 Wang et al. 2406.04692）：**保留不动，互补**。它是模型自主发起的「任务 fan-out」（fresh context，全 harness 子代理）；本移植是用户控制的「对话状态连续咨询」（看现场转写，raw provider 调用）。顺手在 MULTI_AGENT_SYSTEM.md 补一段它的文档（当前零文档）。
- **`src/group_chat/`**：persona 的无工具单发 `provider.process()` 调用形态（`executor.rs:264-284`）是 advisor 调用的参考模式；不复用其代码（其定位是用户可见的多角色讨论）。
- **`src/arena/`**：无关（DDD 黑板，无模型调用）。

## 4. 架构（方案 A）

### 4.1 Provider 链形态

```
生产链（今天）:  Metering(root) → [ModelOverride] → Failover → Http
MoA 链:         Metering(root) → MoaProvider ─┬→ 聚合器:  ModelOverride → named_providers[p] (Failover 链)
                                              ├→ advisor1: Metering("moa:1:p:m") → ModelOverride → Failover 链
                                              └→ advisorN: Metering("moa:N:p:m") → ModelOverride → Failover 链
```

- 插入点：`src/orchestrator/harness_bridge/runner_impl.rs` Step 3 brain-pick（`:101-133`，`ModelOverrideProvider` 同款缝，在根 Metering 包裹之前）。
- **零改 `src/harness/`**（`trace.rs` 加 3 个枚举变体除外，~20 行；`LoopTraceEvent` 是 `#[non_exhaustive]` 正为此设计）。`deps.rs:35` 文档原话：harness 对 provider 形状无感知（R10 by construction）。
- advisor slot 解析：`named_providers[slot.provider]`（route 化 FailoverProvider 链）→ `ModelOverrideProvider::new(chain, slot.model)` → 独立 `MeteringProvider` —— **advisor 自动继承熔断器/429 冷却/降级链**（超越点，hermes slot 是裸调用）。slot.provider 不在 named_providers 时构建期报错回退（见 §7）。

### 4.2 `MoaProvider` 身份面（全部委托聚合器）

| 方法 | 行为 | 原因 |
|---|---|---|
| `serving_model_hint` | 聚合器的 | gauge 分母（context window）+ 运行定价按聚合器模型解析（`runner_impl.rs:196-208`） |
| `protocol` / `model_behavior_override` / `behavior_hint` | 聚合器的 | `resolve_behavior` 选 prompt 行为族（`runner_impl.rs:331`） |
| `supports_native_tools` | 聚合器的 | 工具提取门控 |
| `name` / `color` | `"moa:<preset>"` / 自选色 | 日志可辨识 |
| `as_http_provider` | **`None`（默认）** | 透传聚合器的 HttpProvider 会让 `think.rs:1613` 直接绕过门面（advisor 永不运行）。生产 failover 路径今天就是 `None`（非 token 级流式），行为零变化 |

### 4.3 `process()` 内部流程（每个 Think 迭代）

1. **顾问视图**（`advisory_view.rs`，忠实移植 hermes `_reference_messages` 规则）：
   - 丢弃原 system prompt / system_blocks；
   - assistant 轮：文本保留，`tool_calls` 渲染为 `[called tool: name(args)]` 文本行；空 assistant 轮丢弃；
   - 工具结果：head+tail 截断（每结果 4000 字符预算，`[... N chars omitted ...]` 中缝标记）折进**前一条** assistant 轮 `[tool result: ...]`；无可挂靠时独立成 assistant 行；
   - **必以 user 轮收尾**：尾部是 assistant → 追加合成 advisory-instruction user 轮（Anthropic 无预填规则，且不删上下文）；尾部已是 user → 原样；
   - 空退化 → 最后一条 user 消息。
   - 产出零 tool-role 消息、零 tool_calls 数组 —— 严格 provider（Mistral 类）不会 400。
2. **顾问 payload**：advisory system prompt（英文，移植 hermes `_REFERENCE_SYSTEM_PROMPT` 措辞——你是 MoA 顾问不是行动者、无工具、假定引用的文件/URL 存在、直接给建议不加免责声明）+ 视图；`tools: None`、无 `think_level`、`temperature = advisor_temperature`（None = 不发参数）、`max_tokens = advisor_max_tokens`（None = 不封顶；只封顾问，聚合器永不封顶——顾问输出是延迟主宰项，hermes 实测 turn 延迟与顾问输出 token 相关 ~0.88）。
3. **签名缓存与节奏**：视图逐轮 `role:content` 拼接后 SHA-256。
   - `per_iteration`（默认）：签名变化（新工具结果/新 user 轮）= miss = 重跑顾问；同签名（harness 空响应重试等内部重入）= hit = 复用，不重复计费不重发事件。
   - `user_turn`：run 内只在首次 `process()` 跑，后续迭代一律复用（MoaProvider 按 run 构建，「每用户回合一次」天然成立）。
   - 缓存态 = `Mutex<MoaCache>` 内部可变性（`process(&self)`）。
4. **并行 fan-out**：`futures::future::join_all`（无 hermes `_MAX_REFERENCE_WORKERS=8` 上限），每顾问 `tokio::time::timeout(advisor_timeout_secs)`（默认 120s）。失败/超时 → `[failed: <err>]` / `[timeout after Ns]` 注记，**永不 panic 整回合**。结果顺序按 preset slot 序稳定。整体在 harness `turn_timeout` 内完成（外层 `race_llm_call` 约束）；取消 = drop 整个 future，reqwest 子调用随 drop 取消，安全。
5. **指导注入**（移植 `_attach_reference_guidance`）：`[Mixture of Agents context]` 块（preset 名 + 聚合器标签 + 顾问标签列表 + "你是聚合器兼行动模型，顾问回复是私密上下文" + 各顾问全文）**追加到消息序列末尾**——尾轮是 user 纯文本则并入，否则新增 user 轮。前缀稳定 = KV-cache 可复用（hermes 实测教训：并进较早的 user 轮会让每个工具迭代全量重预填）。
6. **聚合器调用**：原 payload 全量（tools / system_prompt / system_blocks / metadata / tool_choice）+ 注入后消息；`temperature = aggregator_temperature`（未设则透传原 payload 值）。返回其 `ProviderResponse` **原样**（tool_calls / thinking / stop_reason / usage 全透传——聚合器就是行动模型，harness Act 阶段照常消费）。
7. **事件发射**（仅缓存 miss；经构建时注入的 `TraceSink`，`MeteringProvider` 同款模式）：每顾问一条 `MoaAdvisor`，随后一条 `MoaAggregating`，再一条汇总 `MoaAdvisorSpend`；`save_traces=true` 时另发重量 `MoaTurnTrace`。

### 4.4 组件与文件清单

| # | 文件 | 内容 |
|---|---|---|
| 1 | `src/providers/moa/mod.rs` | 模块出口 + `MoaRuntime` 构建（preset → 已解析 advisor/聚合器链） |
| 2 | `src/providers/moa/provider.rs` | `MoaProvider`（AiProvider impl、fan-out、缓存、事件发射） |
| 3 | `src/providers/moa/advisory_view.rs` | 视图变换（纯函数，重点单测对象） |
| 4 | `src/providers/moa/prompts.rs` | advisory system prompt / 聚合指导块模板（英文） |
| 5 | `src/config/types/moa.rs` + `structs.rs` 挂点 | `[moa]` 配置（见 §5）+ 加载期校验 |
| 6 | `src/providers/session_moa_handle.rs` | session 粒度 sticky/one-shot 状态（`session_model_handle.rs` 镜像） |
| 7 | `src/builtin_tools/moa_manage.rs` + 3-touch 注册（`definitions.rs`） | `moa` 工具（§6），自动获得 `/moa` 命令形态 |
| 8 | `runner_impl.rs` Step 3 | 读 handle → 解析 preset → 构建 MoaProvider；one-shot consume-and-clear |
| 9 | `src/harness/trace.rs`（+协议镜像 + `is_step_event` + panel `apply_trace_event`） | `MoaAdvisor` / `MoaAggregating` / `MoaAdvisorSpend`（轻，上线）+ `MoaTurnTrace`（重，只落库） |
| 10 | `gateway/execution_engine/slash_command.rs` | `/moa` 加入 fast-path 排除表 |
| 11 | `docs/reference/MULTI_AGENT_SYSTEM.md` | 补 MoA 文档（连续咨询 + 既有 subagent 一次性 MoA） |

R10 审计：`src/harness/` 仅 `trace.rs` 单文件 +~20 行枚举变体；加代码前 3 问——是脚手架（事件载体非认知）✅；模型升级不消解（显示/审计需求恒在）✅；真实消费者 = panel 显示 + trace 回放 ✅。

## 5. 配置 schema

```toml
[moa]
default_preset = "default"     # /moa one-shot 与裸 on 使用
save_traces = false            # 重量 MoaTurnTrace 门控

[moa.presets.default]
enabled = true                 # false = 跳过顾问，聚合器裸跑（hermes 语义）
advisors = [
  { provider = "openai", model = "gpt-5.5" },
  { provider = "deepseek", model = "deepseek-v4" },
]
aggregator = { provider = "anthropic", model = "claude-opus-4-8" }
fanout = "per_iteration"       # | "user_turn"
advisor_timeout_secs = 120
# advisor_max_tokens = 600     # 可选；聚合器永不封顶
# advisor_temperature = 0.6    # 缺省 = 不发参数（provider 默认），杜绝 hermes 旧版 0.6 强制缺陷
# aggregator_temperature = 0.4
```

- Rust 类型：`MoaConfig { default_preset: String, save_traces: bool, presets: HashMap<String, MoaPreset> }`；`MoaPreset { enabled, advisors: Vec<MoaSlot>, aggregator: MoaSlot, fanout: MoaFanout, advisor_timeout_secs, advisor_max_tokens: Option<u32>, advisor_temperature: Option<f32>, aggregator_temperature: Option<f32> }`；`MoaSlot { provider: String, model: String }`；`MoaFanout::PerIteration | UserTurn`（serde snake_case）。全部 derive `Serialize/Deserialize/JsonSchema` + `#[serde(default)]`（模板：`src/config/types/execution.rs`）。
- 加载期校验：preset 名非空；`advisors` 非空（除非 `enabled=false`）；slot provider/model 非空 trim；**slot.provider == "moa"（不区分大小写）拒绝**（递归防护第 1 层）。
- 不移植：hermes 顶层扁平 legacy 兼容视图、`__HERMES_MOA_TURN_V1__` 标记编解码、`active_preset` 全局持久开关（Aleph 用 session 粒度状态替代）、preset 级 `max_tokens=4096` legacy 字段。

## 6. 用户面语义

### `moa` 工具（`AlephTool`，action 枚举风格照 `loop_manage.rs`）

| action | 语义 |
|---|---|
| `on { preset? }` | session sticky 开启（缺省 `default_preset`）；写 `session_moa_handle` |
| `off` | 清除 session 状态 |
| `once { preset? }` | 下一回合 one-shot（`one_shot: true`） |
| `status` | 当前 session 状态 + 生效 preset 的 advisor 阵容/聚合器/节奏 |
| `list` | 全部 preset 概览（default 标记 `*`） |
| `set_preset { name, advisors, aggregator, ... }` | typed upsert 写 config（经 ConfigPatcher；拒绝 "moa" slot——递归防护第 2 层） |
| `delete_preset { name }` | 删除；拒删最后一个；被删者是 default 则顺延；session 状态指向被删者则清除 |

### `/moa` 命令（经工具目录自动获得；排除 L0 fast path）

- `/moa <提示词>` = one-shot：等价 `once()` + 把提示词作为本回合消息发起完整 run；回合结束状态已被消费（见下）。**参数永远是提示词，即使恰好等于 preset 名**（hermes 测试钉死的语义）。
- 裸 `/moa` = 用法说明 + `status`。

### one-shot 恢复语义（单一机制，超越 hermes 三套发散实现）

`runner_impl.rs` Step 3 读 `session_moa_handle`：`one_shot == true` → **读取即原子 consume-and-clear**（同一锁内完成）。后续任何路径（成功/异常/取消）都无需 restore——状态在 run 构建瞬间已消失，结构性不泄漏。hermes 的 gateway `finally`/TUI 正常返回路径/CLI 实例属性三套 restore 及其各自泄漏风险全部消解。

### 优先级（run 构建时）

**MoA session 状态** > `select_model` session pick > agent pin > flow BrainRef。（model_override 今日不达 runner Step 3，见决定 #6 修订；其挂钩留待管道补通。）

## 7. 错误处理

| 故障 | 行为 |
|---|---|
| 单顾问失败/超时 | `[failed: <err>]` / `[timeout after Ns]` 注记进指导块，聚合器带幸存者行动；顾问链自身 Failover 先行兜底 |
| 全部顾问失败 | 指导块仍注入（带全失败注记），聚合器照常行动 |
| 聚合器失败 | = 正常 provider 错误，harness 现有重试/自愈原样接管（不在门面做恢复策略选择，R10 第 5 不 / A2） |
| preset 缺失/空/slot 无法解析 | run 构建期回退普通 provider 链 + 警告 trace 事件（对话不中断） |
| `enabled = false` | 跳过顾问，聚合器裸跑 |
| 递归 slot | 三层防护：config 加载校验、`set_preset` 工具校验、运行时构建二次校验 |
| 取消/turn_timeout | 外层 select drop 整个 future，子调用随 drop 取消；one-shot 已消费无泄漏 |

## 8. 可观测性与核算

- **每顾问计量**：独立 `MeteringProvider("moa:<idx>:<provider>:<model>")` → `ProviderUsage` trace 事件按顾问自身模型计量；HTTP 层 Pre/PostApiRequest 扩展钩子自动带真实 MODEL/COST_USD。异构模型各按各价——hermes 的 ~50% 成本低估教训（`last_aggregator_slot` 回归）在此结构性不存在（`serving_model_hint` 委托聚合器 + 顾问计量独立）。
- **`MoaAdvisorSpend` 汇总事件**（缓存 miss 时一条）：顾问总 tokens + 按 `pricing::estimate` 逐顾问求和的 USD——补齐 hermes「顾问花费可见」语义，但不污染 `FlowOutcome`/gauge。
- **`MoaTurnTrace`（重量）**：顾问完整输入/输出/用量/成本 + 聚合器输入 + preset 元数据；仅 `save_traces=true` 发射；**不进 `is_step_event` 白名单 = 只落 `task_traces` 不上 WebSocket**；经 `trace.by_runs` 面板时间线回放，零新 RPC。
- **Panel**：`MoaAdvisor` 渲染为折叠「Advisor i/n — provider:model」块（thinking 块样式）；`MoaAggregating` 更新状态行。频道端（Telegram 等）沿现状不投递元事件，只到聚合器最终答案（与 hermes gateway 一致）。

## 9. 测试策略（移植 hermes 14 个测试文件钉死的契约）

全部 `cargo test -p alephcore --lib` 单测 + `MockProvider`：

1. **配置**：缺省容错、preset 解析、递归 slot 拒绝（大小写不敏感）、serde 缺字段兼容（`execution.rs` 测试模板）。
2. **顾问视图**（纯函数重点）：system 丢弃、工具调用/结果扁平化、head+tail 截断、必以 user 收尾（尾 assistant → 合成轮；新鲜 user → 原样）、空退化、零 tool-role 输出。
3. **fan-out**：并行、顺序稳定、单失败不中断、超时注记、全失败仍注入、disabled 跳过、签名缓存（hit 不重跑不重发/新工具结果 miss/user_turn run 内一次）。
4. **身份/核算**：`serving_model_hint`/`supports_native_tools` = 聚合器；`usage` 只含聚合器；工具调用透传；每顾问独立 Metering 标签。
5. **one-shot**：consume-and-clear 原子性、异常路径不泄漏、sticky on/off/status、优先级（MoA > select_model）。
6. **注入位置**：尾 user 并入 / 否则追加。
7. **工具**：action 解析、set_preset 校验、delete 守护（拒删最后一个/default 顺延）。

## 10. 超越清单（vs hermes）

① Tokio 无界并发替代 8 线程池；② 每顾问超时预算（hermes 无超时）；③ 顾问继承熔断/降级/冷却链（hermes 裸调用）；④ 单一 one-shot consume-and-clear（hermes 三套发散 restore，泄漏风险各异）；⑤ 类型化配置 + schemars + 加载期校验（hermes 运行时字符串矫正）；⑥ gauge 诚实性结构保证；⑦ 零 harness 认知代码（hermes 在会话循环热路径 duck-typing 探测 MoA）。

## 11. 范围外（明确不做）

- 模型选择器/`select_model` 的 "moa" 虚拟 provider 集成（后置，用户已决定）。
- hermes legacy 标记编解码、扁平配置兼容视图。
- advisor 调用的 Anthropic prompt-cache 装饰（hermes 有；Aleph 适配器缓存策略独立演进，留优化票——advisory 视图跨迭代 append-only，未来可享前缀缓存）。
- 频道端（Telegram 等）顾问块渲染。
- K-of-N 竞速提前聚合。

## 12. 关键锚点（实施计划引用）

- 插入缝：`src/orchestrator/harness_bridge/runner_impl.rs:101-133`（Step 3 brain pick）
- 包装器范式：`src/providers/model_override_provider.rs` / `src/providers/metering.rs` / `src/providers/failover/provider.rs:597-732`（owned-payload 重建）
- session 状态范式：`src/providers/session_model_handle.rs:22-60` + `src/builtin_tools/select_model.rs:70-82`
- 工具注册 3-touch：`src/executor/builtin_registry/definitions.rs:68+/:908`
- trace 管道：`src/harness/trace.rs`（`#[non_exhaustive]`）→ `agent_trace_emit_sink.rs:45-55`（`is_step_event`）→ panel `interfaces/webchat/src/platform/wide/views/chat/events.rs:376-384`
- fast-path 排除表：`src/gateway/execution_engine/slash_command.rs:75-77`
- 无工具咨询调用形态参考：`src/group_chat/executor.rs:264-284`
- hermes 行为契约来源：`tests/run_agent/test_moa_loop_mode.py`（27 测试）等 14 文件
- 四份深读理解报告（hermes 外围 / Aleph provider 层 / 既有能力盘点 / 命令-配置-事件面，含全部 file:line 锚点）归档于 [`assets/2026-07-05-moa/`](assets/2026-07-05-moa/)
