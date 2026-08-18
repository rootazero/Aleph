# FEATURE_LOCATOR_BACKLOG.md — 打磨待办清单

> 来源：[FEATURE_LOCATOR.md](FEATURE_LOCATOR.md) 附录 A 的 6 个 `⚠️/❌` gap。
> 目的：把"现状与描述不符"的项拆成可排期的待办，标清**类型（微调 / 新功能 / 仅澄清）**、**工作量（S/M/L）**、**涉及文件**、**前置决策**、**验收标准**、**启动话术**。
> 工作量图例：**S** ≈ 单文件/半天 ｜ **M** ≈ 跨 3-6 文件/1-2 天 ｜ **L** ≈ 跨子系统/需设计 + 多日。
> 状态：均为 **未开始**。动手前先读对应词条的"打磨话术"。

## 优先级总览

| # | 项目 | 类型 | 工作量 | 前置决策 | 建议优先级 |
|---|------|------|--------|----------|-----------|
| ~~G6~~ | ~~错误沉淀教训 wiring 复核~~ | 微调(先验证) | — | 无 | ✅ **已完成 2026-06-16（零代码）** |
| ~~G4~~ | ~~per-model 压缩阈值~~ | 新增配置 | M | 无 | ✅ **已实现 2026-06-16（待用户统一 cargo 验证）** |
| ~~G1~~ | ~~doctor LLM 修复 + `f` 入口~~ | 新功能 | S（实际） | 无 | ✅ **已实现 2026-06-16（待用户统一 cargo 验证）** |
| ~~G5~~ | ~~"DAG 工具执行"命名澄清~~ | 仅澄清 | — | 无 | ✅ **已澄清 2026-06-16（零代码，四处文档区分两概念）** |
| ~~G2+G3~~ | ~~Panel 真双层权限~~ | 新建子系统 | L | 信任模型已决策 | ⛔ **已被取代 2026-06-17**：双层 device tier → 单层 Gateway-token（见 [FEATURE_LOCATOR §6.2](FEATURE_LOCATOR.md)） |

---

## G6 — 错误即时沉淀教训：端到端 wiring 复核 ✅ 已完成（2026-06-16，零代码）

- **类型**：微调（先验证，再决定是否补 wiring）→ **验证结论：链路已通，零开发**
- **查证结论**：纠正/教训沉淀**端到端已连且生产存活**，分三跳逐一证实——
  1. **写入** ✅ `src/builtin_tools/flag_user_correction.rs` 工具，构造于 `src/executor/builtin_registry/builder/constructor.rs:1793`（有 `memory_db` 即注册，**非死代码**），写 `aleph://correction/{id}`。
  2. **蒸馏** ✅ `src/memory/dreaming/stages/feedback_distill.rs` 按前缀 + watermark 幂等读 → LLM 蒸馏 `feedback/` note，调度于 `src/memory/dreaming/mod.rs:172,218`（Consolidate + Synthesize 双 dream path）。
  3. **召回** ✅ assembler `gather.rs:284`/`envelope.rs:34` 表面化 `feedback/` note；goal 教训另有 `GoalLessonsPromoteStage`→`lesson/`。
- **关键判断（为何不补 wiring）**：backlog 原设想的"工具失败/LLM 拒绝 → 自动写 raw memory"的 **harness 错误 hook 故意不存在，也不应加**——违 R10「不做错误恢复」+ R7 LLM 主权，且会用瞬时报错噪声淹没记忆。沉淀**刻意做成 LLM/工具驱动**（R8）：LLM 判断"值得记"才调 `flag_user_correction`。因此"链路断"的前提不成立。
- **若将来仍想强化**（非本 gap 范畴）：唯一正当方向是**强化 prompt 引导**（`special_actions.rs`）让 LLM 更主动调工具记教训，或加一个 LLM 可调的 `flag_lesson` 工具——**仍是工具驱动，不是 harness 自动 hook**。
- **遗留可选项**：逐跳单测已有，但缺一个**端到端集成测试**（correction 工具写 → dream distill → feedback note 落地）作回归锁（防 `constructor.rs` 注册被悄悄摘除，类比历史 mutation_gate 死代码 bug）。**非必须**，列为可选加固。
- **详见**：[FEATURE_LOCATOR.md §2.5③](FEATURE_LOCATOR.md)。

---

## G4 — 按模型窗口的差异化压缩阈值（kimi 20w vs claude 100w）✅ 已实现（2026-06-16）

- **类型**：新增 per-model 配置
- **实现结论**：阈值现可按模型覆盖，**纯新增配置、向后兼容、零行为漂移**。
- **设计要点（为何这样接）**：
  - 阈值 **key 在决定预算的"链上最小窗口模型"**（`derive_chain_min_budget`），与 `token_budget` 的尺寸来源同源——若 key 在别的模型上会出现"按 A 定预算、按 B 定阈值"的不自洽。
  - **不动 `pressure.rs` / `ContextBudget`**：阈值是 `ContextBudgetConfig` 的入参，连线点只在 `build_context_budget_config`（config→config）。压缩策略本身一行未改（守 R6/R10）。
  - **不加 `token_budget` per-model 覆盖**：预算已经 model-aware（按窗口派生），重复（YAGNI）。
- **改动文件（4）**：
  1. `src/config/types/phase6_wiring.rs` — `ContextBudgetToml` 加 `model_thresholds: Vec<ModelThresholdToml>`；新 `ModelThresholdToml{model, warning_threshold?, critical_threshold?}`；`threshold_override_for(model, provider)` 首匹配子串。
  2. `src/orchestrator/deps_builder.rs::build_context_budget_config` — 无条件 `derive_chain_min_budget` 取模型身份；override 逐项 `.or(global).unwrap_or(内置)`；解析后过同一 `0<warning<critical≤1.0` 防御闸。
  3. `src/lib.rs` — re-export `ModelThresholdToml`。
  4. `docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md` — 配置文档 + TOML 示例。
- **匹配规则**：`model` 字段是大小写不敏感**子串**，先匹配 resolved model id，再匹配 provider key；声明顺序首匹配胜；空串永不匹配。
- **配置示例**：
  ```toml
  [context_budget]
  enabled = true
  warning_threshold = 0.70
  critical_threshold = 0.85

  [[context_budget.model_thresholds]]
  model = "kimi"            # 窄窗口模型更早压缩
  warning_threshold = 0.60
  critical_threshold = 0.78
  ```
- **测试**：phase6_wiring 5 个（解析 + 匹配/回退/首匹配/空串/无匹配）+ deps_builder 5 个（覆盖应用 / 逐项回退 / 非匹配字节等同 / 显式 token_budget 下仍生效 / 反转阈值禁用）。
- **遗留**：**NOT cargo-checked**（用户统一 cargo）。静态已核：所有 `ContextBudgetToml` 穷举字面量补 `model_thresholds`/`..default`，re-export 链通，`Option<&T>` Copy + `&String` 实现 Pattern 无误。

---

## G1 — Doctor 的 LLM 推理修复 + `f` 触发入口 ✅ 已实现（2026-06-16）

- **类型**：新功能（Panel `f` 入口）
- **核实纠正 backlog 原设想**：精读后发现「LLM 推理修复层」**大部分已存在、且按 R7/R10 不应再建**——
  - `src/builtin_tools/doctor.rs` 的 `DoctorOutput`/`src/diagnostics/finding.rs` 的 `Finding` **早已结构化**：`repairable: bool`（机械可修 vs 不可修的区分已有）+ `fix_hint: Option<String>`（不可修时引导走 `vault_store`/`self_config`）+ `repair_outcome`（Repaired/Failed/Skipped），整个 `DiagnosticReport` 已 `Serialize`，作为 JSON `tool_result` 自然流回 LLM。
  - LLM 现在就能：`doctor(fix=false)` → 读 findings → 机械项 `doctor(fix=true)`、配置/凭据项 `self_config`/`vault_store`。CLI 侧（`interfaces/cli/src/commands/doctor.rs:262-351` 的 `prompt_for_repair()`）甚至已有完整 AI 修复回路。
  - **不应加 harness 错误 hook**（违 R10「不做错误恢复策略选择」）——续跑只在 goal/loop 的 execution-engine hook，不在工具返回值里。
- **真实缺口仅一个**：Panel 没有 `f` 入口。「让 LLM 修复」= 用一句修复 prompt 把现有 loop + 现有工具串起来（R9 智慧在 prompt），**不在 doctor 里写修复分支**。故工作量从 M 收敛为 S，**doctor 工具/diagnostics 后端零改动**。
- **设计（用户裁定：内联 prompt 注入，最小/R10 纯）**：`f` 键注入一句诊断-修复指令到 composer 并走现有 send 管线 → 现有 agent loop 接管。
- **改动文件（5，纯前端）**：
  1. `interfaces/webchat/src/views/chat/state.rs` — 加 `repair_pulse: RwSignal<u32>` + `request_repair()`（1:1 镜像 `retry_pulse`/`request_retry`，同样排除出 `SessionSnapshot`）。
  2. `interfaces/webchat/src/views/chat/composer/mod.rs` — `chat.doctor_repair_prompt` locale 键（R9 的「智慧」载体，2026-08-18 从 `DOCTOR_REPAIR_PROMPT` Rust 常量搬进 `locales/{en,zh}.json`）+ 监听 `repair_pulse` 的 Effect：注入 prompt → run 空闲时 `send_message()`、活跃时 `enqueue_message()`（与 Enter 同语义）。
  3. `interfaces/webchat/src/state/hotkey.rs` — `HotkeyState` thread 进 `ChatState`；裸 `f` 绑定 + `focus_is_editable()` 护栏。
  4. `interfaces/webchat/src/app.rs` — `ChatState::new()` 绑定后传入 `HotkeyState::new(voice_mode, chat_state)`。
- **关键正确性点（backlog 漏判）**：裸 `f` 会与「在输入框打字打到 f」冲突。护栏：**任何修饰符**（meta/ctrl/alt/shift）、palette/voice overlay 开启、或焦点在 `<input>`/`<textarea>`/contenteditable 时一律不触发；仅在全清时 `prevent_default` + `request_repair()`。Esc/⌘K 免护栏因其非用户会打入正文的字符。
- **修复 prompt（R9）**：「运行 doctor 工具诊断系统健康状况。对可机械修复的问题（repairable=true）调用 doctor(fix=true) 修复；对不可机械修复的问题，按其 fix_hint 用 self_config / vault_store 等对应工具修复；全部处理后再次运行 doctor 验证，并简要报告修复结果。」
- **验收**：Panel 焦点不在输入框时按 `f` → 注入并发送上述指令 → LLM 调 doctor 读 findings 并按 repairable/fix_hint 路由修复 → 再 doctor 验证。在输入框打字打到 `f` 不触发。
- **遗留**：**NOT cargo-checked**（用户统一 cargo）。静态已核：`HotkeyState::new` 唯一 caller（app.rs）已改；`ChatState` 路径 `crate::views::chat::state` 三级 pub 链通；`repair_pulse` 非 snapshot 字段无需改 `capture_snapshot`；`send_message`/`enqueue_message` 闭包全 Copy 捕获可被第二个 Effect 复用；`focus_is_editable` 用 `JsCast`（已 import）。

---

## G5 — "DAG 工具执行"命名澄清 ✅ 已澄清（2026-06-16，零代码）

- **类型**：仅认知澄清 → **交付物 = 文档区分，零代码**
- **核实结论**：backlog 论断**代码逐字证实**——`src/tools/concurrency.rs` 头部自述 "Resource-scope-aware concurrency claims … a data-race guard, not an LLM judgement (so it stays inside R7/R10: no intent inference, no relevance scoring, no tool filtering) … this only schedules them"。即三态 claim（`Shared` / `Exclusive{Global, Paths}`）的**资源群分并行**（群内并行、群间串行），**不是**任务依赖图。
- **真正的任务级 DAG**（已验证锚点）：
  - `src/workflow/compile.rs`：头部 "materialised into the existing coordination-task DAG"，`step.depends_on → coord_task.blocked_by`，**拓扑序**物化。
  - `src/teams/dispatcher/`：按 `blocked_by` 边扫描 Runnable、选最闲 owner 并发执行。
- **澄清落点（四处，FEATURE_LOCATOR.md）**：速查表行（`工具并发(群分) ≠ 任务 DAG`，标签即教学）、§3.3（状态翻 ✅ + concurrency.rs 自述引用 + 双层锚点）、§4.3 Workflow / §4.4 Task（任务 DAG 真身）、术语表"DAG"条目。
- **何时才变成开发项**：仅当确有"工具调用之间需要真正的依赖图编排"（而非群分）——但该需求应上升到 Workflow 层表达，**不在工具层重造 DAG**（违 R6 简洁性）。本 gap 不触发任何代码。
- **启动话术**（留作未来描述参考）：「这条不是 bug 是命名。要‘多步骤依赖编排’走 Workflow（`src/workflow/`）；`tools/concurrency.rs` 维持群分并行即可，不要在工具层造第二个 DAG。」

---

## G2 + G3 — Panel 真双层权限（⛔ 已被取代 2026-06-17 → 单层 Gateway-token）

> **取代说明**：双层 device tier（2026-06-16 实现）于 **2026-06-17 被单层 Gateway-token 模型取代**——远程“授权后权限即同本地”，无 Chat/Config 子层。本节保留为决策史；**当前真相见 [FEATURE_LOCATOR §6.2](FEATURE_LOCATOR.md)**。下方“落地”所列文件多已随收敛删除，勿按其定位现状（见“取代后的去留”）。

- **类型**：新建子系统（"恢复/新建双层"，非微调）
- **原决策（已废）**：loopback（本机 App）= operator（Config）；remote（局域网 Panel）= Chat 默认，须显式提权。tier 治理“配置变更”而非“执行”。
- **2026-06-16 落地（已被取代，多数文件已删）**：
  - 新建 `src/gateway/panel_devices.rs`（`PanelDeviceStore` + SQLite）+ `handlers/devices.rs`（`devices.list`/`set_level`/`revoke`）
  - `server/handler.rs` connect 握手按 `device_id`+loopback 解析 tier，回填 `role`
  - `method_authz.rs::rpc_requires_operator` + `OPERATOR_RPC_METHODS` RPC 门；前端 `views/settings/security/devices.rs::PanelDevicesSection` + `ConfigGate` 包 17 配置页
- **取代后的去留（2026-06-17 收敛）**：
  - **删除**：`src/gateway/panel_devices.rs`、`src/gateway/handlers/devices.rs`、前端 `views/settings/security/devices.rs`(`PanelDevicesSection`)、`components/permission.rs`(`ConfigGate`)、`method_authz.rs::rpc_requires_operator`/`OPERATOR_RPC_METHODS`、`devices.set_level` RPC。
  - **保留并改义**：`server/handler.rs` connect 握手改为校验 Gateway token（`connect_authorized`）→ `caller_role` 仅 operator/guest 二值；登录墙（非 operator 仅放行 `connect`）取代逐 RPC 白名单；`method_authz.rs` 仅余 **channel** tier 的 `tool_requires_operator`（panel 已解耦）。
  - **未受影响**：`security/store/devices.rs` 的 device 表（始终是 **cluster 节点**专用，与 panel tier 无关，仍由 `handlers/cluster.rs` 使用）。
- **当前真相**：见 [FEATURE_LOCATOR §6.2](FEATURE_LOCATOR.md) 与附录 A #2/#3（均 ✅ 单层）。

---

## 建议执行顺序

1. ~~**先 G6**（验证，可能零成本）~~ → ✅ **G6 已完成（零代码，链路已通）**。
2. ~~**G4**（per-model 压缩阈值，新增配置）~~ → ✅ **G4 已实现 2026-06-16**。~~**G1**（doctor LLM 修复 + `f` 入口）~~ → ✅ **G1 已实现 2026-06-16**（纯前端 `f` 入口，doctor 后端零改动）。下一步 **G5**（命名澄清，无需开发）/ **G2+G3**（需架构决策）。
3. ~~**G5** 只在文档/沟通层澄清，不进开发队列。~~ → ✅ **G5 已澄清 2026-06-16**（零代码，FEATURE_LOCATOR 四处区分"工具并发群分"vs"任务 DAG"）。
4. ~~**G2+G3** 单独拉一次架构决策会（信任模型）。~~ → 双层 2026-06-16 实现，**2026-06-17 被单层 Gateway-token 取代**（远程授权后权限即同本地，无 Chat/Config 子层；两层 tier 文件已删，见 §6.2）。**G1–G6 全部清空。**

---

## 2026-06-19 扫描新增待办（Harness 3.2 / 3.9 / 3.10 / 3.11）

> 来源：对 harness 子系统 3.2/3.9/3.10/3.11 的串行深度审计（每条均逐跳读码核验）。
> **已就地修复（本批，非待办）**：① §3.10 事件驱动插件 hook 派发断裂（`HookAction::Plugin` 连线，见 [FEATURE_LOCATOR §3.10](FEATURE_LOCATOR.md)）；② §3.2 `act.rs::apply_turn_budget` 的 `already_persisted` 字节 0 误判（改 `lines().any`）。
> 下列 **H1–H11 为已核验、暂缓**项——多为"删死代码 vs 补连线"二选一，需一次 `cargo` 编译验证后落地（盲删导出类型/重连 MCP 传输风险高），故留作可排期待办而非本批盲改。

| # | 项目 | 模块 | 类型 | 工作量 | 严重度/置信 | 决策点 |
|---|------|------|------|--------|-----------|--------|
| H1 | 插件 bundled `.mcp.json` 被解析后丢弃（非 MCP-kind 插件的 `CapabilityDeclaration::McpServer` 在 `dispatch` 被 no-op，loader 仅对 `PluginKind::Mcp` 读 `.mcp.json`） | 3.10 | broken-wiring | M | **High / 0.9** | 连线进 loader/manager **或** 停止非 MCP-kind 适配器发该 cap |
| H2 | `parse_mcp_config_file` 丢弃 server-name map key → 无名 `McpServerConfig`（无法按名注册/拆卸） | 3.10 | dead-field | S | Med / 0.85 | 随 H1，把 name 带进声明 |
| H3 | MCP `ApprovalHandler` 全子系统零消费者（`client.rs` 仅派发 `sampling/createMessage`，其余 server 请求丢弃） | 3.9 | dead/unwired | M | Med / 0.9 | 接入 `client.rs` request_handler **或** YAGNI 删 `approval.rs`+协议类型 |
| H4 | `McpResourceManager`/`McpPromptManager` 仅测试构造（live 路径走 `McpReadResourceTool`/`McpGetPromptTool` 直连 handle） | 3.9 | dead-abstraction | S | Med / 0.92 | 删两 struct（P6 YAGNI）**或** builtins 改走 manager 单点 |
| H5 | Resource subscribe/unsubscribe 死路（`client.subscribe_resource` 无非测试 caller）+ `resources/updated` 通知未路由（`classify_list_change` 只映 `*_list_changed`） | 3.9 | broken-wiring | M | Med / 0.88 | 补 `resources/updated` 臂并发事件 **或** 删订阅管线 |
| H6 | `read_resource` 永不产 `ResourceContent::Image`（image-mime blob 被标 `binary`，消费侧 Image 臂死） | 3.9 | dead-field | S | Low / 0.85 | Blob 臂按 `mime.starts_with("image/")` 分流 **或** 删 Image 变体 |
| H7 | `read_resource` 静默丢弃首项外的所有 content（`contents.into_iter().next()`，无 log/marker；对照 `tools/call` 有省略标记） | 3.9 | silent-swallow | S | Low / 0.8 | 映射全部 items **或** `len()>1` 时 warn |
| H8 | HTTP/Auto 传输收不到 server 发起的 sampling（`set_request_handler` 仅装在 SSE 分支，`HttpTransport` 无 request-handler 支持） | 3.9 | broken-wiring | M | Low / 0.7 | 给 HttpTransport 补 request-handler **或** 文档化"sampling 仅 SSE" |
| H9 | Skill `${ALEPH_SESSION_ID}` 生产永不解析（`with_session` 仅单测调；`read.rs:298` 用 `new()` 未线 session id） | 3.11 | broken-wiring | S | Med / 0.9 | `read.rs` 线入 session id **或** 删 session_id/with_session/token 死面 |
| H10 | `InvocationPolicy.command_dispatch`（`DispatchSpec`/`ArgMode`）只写 `None`、解析器无字段可填、零读者 | 3.11 | dead-field | S | Low / 0.95 | 接 frontmatter `command-dispatch` 键 **或** YAGNI 删整套 |
| H11 | `SkillId::new` 无校验：纯标点/空格名（`"---"`/`"   "`）塌缩为空 id，registry 以空键存、不可寻址 | 3.11 | edge-case | S | Low / 0.6 | 空 id 兜底 slug 或 parse 期 reject |

**附带（同类 bug，越界 §3.2→§2 Context，未改）**：`src/context/budget/cheap_passes/tool_result_pruning.rs:88` 的 `original_text.starts_with("[Full output persisted: ")` 与本批 act.rs 修复同源——同样应改为按行扫描。属 Context 层，留待用户裁定是否一并修。

> **执行建议**：H1（High，真功能断裂）优先；H3/H4/H10 是 YAGNI 删除候选（确认零消费者后一次 cargo 验证落地）；H6/H7 是小而独立的健壮性补；H9 是一行连线。**全部需一次 `cargo check` 兜底**，符合"用户统一 cargo 验证"节奏。
</content>
