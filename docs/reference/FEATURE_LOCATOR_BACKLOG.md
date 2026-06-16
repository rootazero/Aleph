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
| ~~G2+G3~~ | ~~Panel 真双层权限~~ | 新建子系统 | L | 信任模型已决策（loopback=operator / remote=chat） | ✅ **已实现 2026-06-16** |

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
  2. `interfaces/webchat/src/views/chat/composer/mod.rs` — `DOCTOR_REPAIR_PROMPT` 常量（R9 的「智慧」载体）+ 监听 `repair_pulse` 的 Effect：注入 prompt → run 空闲时 `send_message()`、活跃时 `enqueue_message()`（与 Enter 同语义）。
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

## G2 + G3 — Panel 真双层权限（✅ 已实现 2026-06-16）

- **类型**：新建子系统（"恢复/新建双层"，非微调）
- **决策**：信任模型已拍板——**loopback（本机 App）= operator（Config）保单机零配置；remote（局域网 Panel）= Chat 默认，须显式提权**。tier 治理的是"配置变更"，不是"执行"（`bash`/PTY 仍随网络信任边界）。"配对时选 tier" + "事后 `devices.set_level`" 两条都做。
- **实勘修正（实施时发现，与原假设有出入）**：
  - 前端 2 层权限**早已 100% 建好**（`DashboardState.role` + `ConfigGate` 包 17 个配置页 + `LockedNotice`/`PermissionBanner`/`friendly_error` + i18n "re-pair selecting Config"），只是后端 `connect.rs` 硬编码 `"role":"operator"` 让整套**恒真失效**。
  - 原假设的 `devices.set_level` / device 表 tier 字段 / pairing 选 tier **均不存在**（`devices` 表是 cluster 节点专用）。`ChannelPermissionLevel{Chat,Config}` 枚举已存在（→`guest`/`operator`），工具门 `tool_requires_operator` 已存在但因恒 operator 从不触发。
- **落地（实际改动）**：
  - **新建** `src/gateway/panel_devices.rs`（`PanelDeviceStore` + SQLite `panel_devices.db` + 进程全局 + `resolve_tier`；loopback→Config，remote→持久化 per-device tier 默认 Chat）+ `utils/paths.rs::get_panel_devices_db_path`
  - **连线** `server/handler.rs`：`connect` 握手按 `device_id`+loopback 解析 tier → 写入 `ConnectionState.caller_role`（新字段，loopback 默认 operator）→ 改写 connect 响应 `role`（前端 ConfigGate 即激活）→ 新设备发 `panel.device.pairing` 事件；删掉 line 492 的硬编码 operator，改为按连接读取
  - **纵深防御** `method_authz.rs::rpc_requires_operator` + `OPERATOR_RPC_METHODS`（config 类 RPC 白名单）+ handler 调度前 RPC 门（chat tier 拦配置 RPC，挡手搓 RPC 绕过隐藏 UI）；工具门复用已存在的 `tool_requires_operator`
  - **新建** `handlers/devices.rs`（`devices.list`/`set_level`/`revoke`，operator-only）+ `start/mod.rs` 注册 + 全局 store 安装
  - **前端** `context.rs` 生成持久 `device_id`（localStorage UUID）+ `device_name` 进握手；`views/settings/security/devices.rs` 新增 `PanelDevicesSection`（设备列表 + Grant Config / Set Chat / Revoke）
- **验收**：remote chat 设备连接后，配置类 RPC + 工具被后端真实拦截（PERMISSION_DENIED）；前端 ConfigGate 真生效；配对默认 Chat；`devices.set_level` 能提权/降权；loopback 始终 operator。
- **遗留**：pairing 实时 toast 未做（新设备靠 Settings→Security 列表 + Refresh 呈现，授权 = 选 tier）；i18n 用英文字面量（未进 locale 文件）；**NOT cargo-checked**（用户统一验证）。

---

## 建议执行顺序

1. ~~**先 G6**（验证，可能零成本）~~ → ✅ **G6 已完成（零代码，链路已通）**。
2. ~~**G4**（per-model 压缩阈值，新增配置）~~ → ✅ **G4 已实现 2026-06-16**。~~**G1**（doctor LLM 修复 + `f` 入口）~~ → ✅ **G1 已实现 2026-06-16**（纯前端 `f` 入口，doctor 后端零改动）。下一步 **G5**（命名澄清，无需开发）/ **G2+G3**（需架构决策）。
3. ~~**G5** 只在文档/沟通层澄清，不进开发队列。~~ → ✅ **G5 已澄清 2026-06-16**（零代码，FEATURE_LOCATOR 四处区分"工具并发群分"vs"任务 DAG"）。
4. ~~**G2+G3** 单独拉一次架构决策会（信任模型）。~~ → ✅ **G2+G3 已实现 2026-06-16**（信任模型决策：loopback=operator / remote=chat 默认 + 显式提权；新建 `panel_devices` 子系统 + RPC/工具双门 + 前端设备管理）。**Backlog 全部清空。**
</content>
