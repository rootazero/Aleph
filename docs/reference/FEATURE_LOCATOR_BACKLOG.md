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
| G1 | doctor LLM 修复 + `f` 入口 | 新功能 | M | 无 | **中** |
| G5 | "DAG 工具执行"命名澄清 | 仅澄清 | — | 无 | **低**（无需开发） |
| G2+G3 | Panel 真双层权限 | 新建子系统 | L | **架构决策（LAN-trust）** | **暂缓**（先决策再排期） |

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

## G1 — Doctor 的 LLM 推理修复 + `f` 触发入口

- **类型**：新功能
- **现状**：`doctor` 只有 `fix:bool` 的**机械修复**（建缺目录、清 stale lock）。**没有**"按 `f` 启动 LLM 完成修复"的入口，也没有 LLM 推理式修复层。
- **涉及文件**：`src/builtin_tools/doctor.rs`、`src/diagnostics/`（findings 结构化输出）、若要 `f` 键入口 → 前端 `interfaces/webchat/`（Panel doctor 视图 + keybinding）
- **目标**：doctor 把无法机械修复的 findings 结构化交给 agent loop，由 LLM 推理修复（符合 R7 LLM 主权，而非在 doctor 内写确定性修复逻辑）；可选 Panel `f` 快捷键触发这一流程。
- **工作量**：M（核心 = findings → agent loop 的工具/事件桥接；`f` 键是额外前端小项）
- **前置**：无（但需明确：LLM 修复走主循环工具，不在 doctor 内堆修复逻辑——守 R10）
- **验收**：制造一个非机械可修问题（如配置语义错误），触发后 LLM 读取 finding 并提出/执行修复；Panel `f` 能拉起该流程。
- **启动话术**：「给 doctor 加‘LLM 修复’：doctor 仍只做诊断 + 机械修复，把**剩余 findings 结构化**喂给主 agent loop 让 LLM 推理修复（R7，别在 doctor 内写修复分支）。可选给 Panel doctor 视图加 `f` 快捷键触发。注意这是新功能——‘doctor+f’当前不存在。」

---

## G5 — "DAG 工具执行"命名澄清（无需开发）

- **类型**：仅认知澄清
- **现状**：工具层 `src/tools/concurrency.rs` 是**按资源作用域的群分并行**（群内并行、群间串行），**不是**任务依赖图。真正的任务级 DAG 在 `src/workflow/compile.rs` + `src/teams/dispatcher/`。
- **目标**：无需开发。**描述时分清**："工具并发" → `tools/concurrency.rs`；"任务 DAG" → workflow/teams。
- **工作量**：—
- **何时才变成开发项**：仅当你确有"工具调用之间需要真正的依赖图编排"（而非群分）——但该需求通常应上升到 Workflow 层表达，**不建议**在工具层重造 DAG（违 R6 简洁性）。
- **启动话术**：「这条不是 bug 是命名。要‘多步骤依赖编排’走 Workflow（`src/workflow/`）；`tools/concurrency.rs` 维持群分并行即可，不要在工具层造第二个 DAG。」

---

## G2 + G3 — Panel 真双层权限（需架构决策后再排期）

- **类型**：新建子系统（"恢复/新建双层"，非微调）
- **现状**：LAN-trust 绝对化——`src/gateway/handlers/connect.rs` 给所有连接（含 Panel）硬编码 `"role":"operator"`。前端 `ConfigGate`（`interfaces/webchat/src/components/permission.rs`）有锁但后端恒放行。device 表（`src/gateway/security/store/`）**无 tier/level 字段**，**无 `set_level` API**，pairing **不能选 tier**。
- **目标（若启用双层）**：第一层 = 对话 + 默认工作目录；第二层 = 配置权限 + 自由建工作目录。配对默认第一层，可配对时选 tier 或事后 `devices.set_level` 提权。
- **涉及文件（预估）**：
  - device 表 schema + `src/gateway/security/store/{types.rs,devices.rs}`（新增 `tier`/`level` 列 + `set_level`）
  - `src/gateway/handlers/connect.rs` + `caller_identity.rs`（停止无条件 operator，按 device tier 派角色）
  - `src/gateway/pairing_store.rs` + pairing handler（approve 带 tier 参数）
  - dispatcher 权限检查真生效（`src/tools/scoped/dispatch.rs` 的 `tool_requires_operator` 在非 operator 时确实拦截）
  - admin set_level API（`src/gateway/admin_api/`）
  - 前端 pairing UI 选 tier + 设备管理页
- **工作量**：L
- **前置（必须先定）**：**这与现行架构决策直接冲突**——CLAUDE.md 明确"信任模型 = 网络边界，LAN 内任何设备获得对 agent 的完全控制权"。启用双层 = **改变信任模型**。需先决策：
  1. 是否真要在 LAN-trust 之上叠设备级 tier？还是维持"LAN = 信任边界"？
  2. 若要，tier 是"配对时选" + "事后 set_level"两条都做，还是先只做事后授权？
- **验收**：非 operator 设备连接后，配置类工具（self_config / skill_install / agent_create 等 `OPERATOR_TOOLS`）被后端真实拦截；配对默认 Chat tier；`set_level` 能提权。
- **启动话术**：「**先别动代码**——Panel 真双层权限与现行 LAN-trust 信任模型冲突（CLAUDE.md：LAN=信任边界，全员 operator）。先决策‘是否改变信任模型’。若确定要做，按‘新建子系统’对待：device 表加 tier 字段 + `connect.rs` 按 tier 派角色 + dispatcher 检查真生效 + pairing/admin tier 入口。当前前端 ConfigGate 锁只是 UI 投影，不要误以为后端已强制。」

---

## 建议执行顺序

1. ~~**先 G6**（验证，可能零成本）~~ → ✅ **G6 已完成（零代码，链路已通）**。
2. ~~**G4**（per-model 压缩阈值，新增配置）~~ → ✅ **G4 已实现 2026-06-16**（待用户统一 cargo 验证）。下一步 **G1**（doctor LLM 修复 + `f` 入口，独立新功能）。
3. **G5** 只在文档/沟通层澄清，不进开发队列。
4. **G2+G3** 单独拉一次架构决策会（信任模型），决策通过后再按 L 级子系统排期；否则维持现状并在 FEATURE_LOCATOR §6.2 标注"刻意不实现"。
</content>
