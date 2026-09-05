# SESSION_KNOBS.md — 会话旋钮 (Session Knobs)

> 由根 `CLAUDE.md`「开发指南 → 会话旋钮」一行指针指向本文。**这里是母本**，CLAUDE.md 那侧只留一句话与指针。
> 每一根旋钮的**子系统全文**在它自己的文档里（见「单一源」列）；本文回答的是**只有把六根并排才答得出**的那一问：
> 它们各自管什么、谁在拨、加第七根要动哪几处。

**别在这里维护一个数目**——上一版标题写着「三根」而表里已经不止三行。全部**正交**，且**不是每一根都有 pill**：见「谁在拨」列。

前五根共用一套机制：值住在 `SessionMetadata.identity_meta.custom[<key>]`，precedence 一律 **请求 > 会话 > 全局**，请求携带的值会被**盖回会话**（所以选择活过它所在的那一轮），解析各在 `src/gateway/execution_engine/turn_*.rs` 的孪生模块里。加第六根之前先读那五个文件里任意一个——它们逐行同形是有意的。

| 旋钮 | 值 | 管什么 | 谁在拨 | 单一源 |
|---|---|---|---|---|
| **执行档位 Exec Tier** | `Plan` / `Ask` / `Auto`(默认) / `Full` | 工具执行**审批**。读工具**声明的元数据**（幂等/destructive），不认名字；未知工具在 `Ask` 档 fail-closed。**`Plan` 是只读规划档，仅会话可选**（`builtin_tiers()` 装机三档 vs `session_tiers()` 四档）：mutating 一律拒绝，人类在 `scratchpad(action='request_approval')` 批准后**同一轮当场**交回 restore 档。⚠️ `Plan` 的拒绝是**地板**（`effective_permission` rung 0），排在 explicit `[policies.tool_permissions]` 条目**之上**——一条 `"bash"="allow"` 掀不翻它；其余三档的 explicit 优先逐字节不变 | Panel pill + `chat.send{exec_tier}` + TUI `/tier` | `src/tools/scoped/`（唯一强制点）+ `src/tools/plan_gate.rs` → [SECURITY.md](SECURITY.md) |
| **会话模式 Session Mode** | `chat` / `work`(默认) / `code` | 工具**呈现面**静态分区（R10 渐进披露例外）。不授予不拒绝任何权限 | Panel pill + `chat.send{mode}` + `session_set_mode` 工具 + TUI `/mode` | `src/config/types/policies/session_mode.rs` → [MODE_SYSTEM.md](MODE_SYSTEM.md) |
| **推理档 Think Level** | `off`…`xhigh`，**未设=不发指令** | 模型被要求想多深（reasoning token 按 output 计费） | Panel pill + `chat.send{thinking}` + `self_config` + TUI `/think` | `src/agents/thinking.rs` + `execution_engine/turn_thinking.rs` |
| **记忆模式 Memory Mode** | `on` / `off`（默认跟 `[memory] enabled`） | 这一轮 prompt **注入**不注入 curated memory / 笔记索引 / 召回。**不闸工具、不闸写** | Panel pill + `chat.send{memory}` + TUI `/memory-mode` | `src/memory/session_memory_mode.rs` + `harness_bridge/prompt_build.rs`（唯一闸点）→ FEATURE_LOCATOR §5.23 |
| **模型 pin Model Pin** | 任意 model id（+可选 provider） | 这个对话此后用哪个模型（下一 run 起生效） | **只有 `select_model` 工具**（R8 对话式）——`sessions.patch` 明确拒绝该键。TUI 的 `/providers` 选择器**不是第二个写者**：它确认时发 `/model <id>` 这条网关命令，仍然落到同一个工具。Panel 的 `ModelPicker` **只显示不设置**（无 per-turn override 时 pill 印的就是 pin，否则它会报出用户刚换掉的那个模型） | `src/providers/session_model_handle.rs` + `gateway/session_model_pin.rs` + `execution_engine/turn_model.rs` |
| **繁忙输入 Busy Input** | `Steer`(默认) / `Interrupt` / `Queue` | 会话已有 run 在跑时新消息怎么办 | **per-channel 配置**（channel 实例配置块里的扁平键 `busy_input_mode`，经 `ChannelPolicyConfig` 解析）+ 三个写死的生产者（team run / OpenAI 兼容面 / 续跑，全钉 `queue`）。**Panel 靠手势而非旋钮**：`＋`/Enter = 客户端幽灵队列（≈Queue，且可 ↑ 撤回）· 轮边界自动 flush = Steer（服务端默认档）· `⚡`/Esc = abort + 重排（≈Interrupt） | `src/gateway/busy_queue/` → FEATURE_LOCATOR §4.8 |

> **别急着给 Busy Input 加参数**：三种处置在 Panel 上都已可达且各自正确，加一条 `busy_input` wire 参数会得到零消费者的通道（R10）。要改的是**手势与模式的对应关系**，不是新增旋钮面。

> **加第六根旋钮的清单**（前五根里有一根每一步都漏过，代价见 [FEATURE_LOCATOR §5.23](FEATURE_LOCATOR.md)）：① `turn_*.rs` 孪生解析器；② `sessions.patch` 的 `knob_validators()`（census 会红）；③ `session_snapshot.rs` 的解码（census 会红）；④ 至少一个客户端面读得到它——**没有读者的 knob 和没人设的 knob 长得一模一样**；⑤ 如果它闸住了什么，那句话要同时出现在代码、doc 和**发给模型的 prompt** 里。

`[sandbox.command_policy]` 的硬底线**任何档位都压不下去**。

## 崩溃恢复：run 开始时的旋钮快照

一个被守护进程崩溃打断的 run，恢复时**不再重新问今天的旋钮**。`RunStarted` marker 上冻结着它开跑时的 envelope（`session::events::RunEnvelopeSnapshot`：`exec_tier` / `session_mode` / `think_level` / `memory_mode` / `model` / `model_provider`，字段名与本表同一套词表，由 `session_snapshot::RUN_ENVELOPE_KNOB_KEYS` 的 census 钉住），`resume_coordinator` 从**同一个** `open_run` 锚点读出来，一次性算成 `ResumePlan`。

- **优先级：快照 > 会话 > 全局**，对 model / think / mode / memory 四根成立——它们经既有的 request rung 载荷回放。日志是唯一真源（`identity_meta.custom` 里没有逐键时间戳），所以「快照胜」是唯一能从日志派生出来的规则。
- **`exec_tier` 只收紧，从不放宽。** 它**不走** request rung（那一根会赢过会话与全局，于是一个 `full` 快照能把操作员事后调紧的对话重新拉开），而是走 `RESUME_TIER_CEILING_KEY` 这个**天花板**键：三根 rung 照常解析完之后，`ExecTier::most_restrictive` 再压一次。两个方向不对称是有意的——恢复得太紧只是多一次审批，恢复得太松是在无人值守下跑一个已经被收回的档位（判据 #14）。
- **恢复不 stamp。** 四个 `turn_*.rs` 共用 `execution_engine::knob_to_stamp`，`RunRequest::is_resume()` 为真时一律不写回会话行——否则一次恢复会撤销用户在崩溃**之后**为驯服这个 run 所做的改动（④-D8）。
- **model 先校验再回放。** `validate_snapshot_model` 复用 `model_catalog::lifecycle_for` 与 `pinnable_providers()`：退役且有继任 → 换继任；无继任、或 provider 已不在本机配置里 → 落回 agent 默认链。任一降级都写进 `ResumeReport.degraded`，并**对模型说一句**（有悬空调用时附在第一条边界修复的 `ToolError` 上，没有时单独追加一条 `SystemMessage`）。`project_root` 消失走同一条路（裁定 A9）。
- **载体是 `RunRequest.model_override`**，它从不写回会话行——所以崩溃之后用户新调的 `select_model` 仍然从**下一条消息**起生效，正是 `select_model` 打印给模型的那句承诺。
- **没有快照的 marker 计入 `ResumeReport.unsnapshotted`**（envelope 出现之前写下的 marker，以及 `RunStarted` 追加失败的那种 run）。它们照旧按今天的会话/全局值恢复——数出来，而不是假装不存在。
- **MoA 预设不持久化**（裁定 A5）：恢复的 run 不重建崩溃时的 MoA 组合。
