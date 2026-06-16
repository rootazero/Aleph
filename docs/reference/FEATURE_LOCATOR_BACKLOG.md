# FEATURE_LOCATOR_BACKLOG.md — 打磨待办清单

> 来源：[FEATURE_LOCATOR.md](FEATURE_LOCATOR.md) 附录 A 的 6 个 `⚠️/❌` gap。
> 目的：把"现状与描述不符"的项拆成可排期的待办，标清**类型（微调 / 新功能 / 仅澄清）**、**工作量（S/M/L）**、**涉及文件**、**前置决策**、**验收标准**、**启动话术**。
> 工作量图例：**S** ≈ 单文件/半天 ｜ **M** ≈ 跨 3-6 文件/1-2 天 ｜ **L** ≈ 跨子系统/需设计 + 多日。
> 状态：均为 **未开始**。动手前先读对应词条的"打磨话术"。

## 优先级总览

| # | 项目 | 类型 | 工作量 | 前置决策 | 建议优先级 |
|---|------|------|--------|----------|-----------|
| G6 | 错误沉淀教训 wiring 复核 | 微调(先验证) | S | 无 | **高**（先验证，可能零成本或小补） |
| G4 | per-model 压缩阈值 | 新增配置 | M | 无 | **中**（独立、收益清晰） |
| G1 | doctor LLM 修复 + `f` 入口 | 新功能 | M | 无 | **中** |
| G5 | "DAG 工具执行"命名澄清 | 仅澄清 | — | 无 | **低**（无需开发） |
| G2+G3 | Panel 真双层权限 | 新建子系统 | L | **架构决策（LAN-trust）** | **暂缓**（先决策再排期） |

---

## G6 — 错误即时沉淀教训：端到端 wiring 复核

- **类型**：微调（先验证，再决定是否补 wiring）
- **现状**：`feedback`/`lesson` 分类目录与 `DistillAction::FeedbackDistill` 已定义（`src/memory/dreaming/distill_action.rs`），但"错误捕获 → 记录 raw memory → dream daemon distill 成 lesson"的端到端是否每次错误都触发，未确认。
- **涉及文件**：`src/memory/dreaming/distill_action.rs`、`src/memory/dreaming/mod.rs`、`src/memory/notes/indexer.rs`、错误捕获侧（harness 失败路径 / tool error → raw memory 写入点）
- **目标**：确认或补齐"工具失败 / LLM 拒绝 → 错误事实落 raw memory → dream 周期 distill"链路。
- **工作量**：S（验证为主；若链路已通则零开发，若断则补 1-2 处写入点）
- **验收**：构造一次工具失败，确认 raw memory 出现错误事实，且 dream 周期后生成对应 `lesson/feedback` note。
- **启动话术**：「验证‘错误即时沉淀教训’三支柱③的端到端链路：从工具失败/LLM 拒绝到 raw memory 写入，再到 dream daemon 的 FeedbackDistill。先只读追踪 `src/memory/dreaming/` + 错误捕获侧，确认哪一环没接，再决定是否补 wiring——别先改代码。」

---

## G4 — 按模型窗口的差异化压缩阈值（kimi 20w vs claude 100w）

- **类型**：新增 per-model 配置
- **现状**：压缩触发按当前模型 `token_budget` **自动按比例浮动**（warning≈0.70 / critical≈0.85），但**没有 per-model 专属阈值**——无法"给 kimi 单独设更激进的触发点"。
- **涉及文件**：`src/context/budget/pressure.rs`、`src/context/budget/mod.rs`、`src/providers/model_catalog/`（读 per-model 配置）、config types（新增 per-model 阈值字段）
- **目标**：允许按 provider/model 覆盖 warning/critical 阈值；无覆盖时回退现有比例逻辑（向后兼容）。
- **工作量**：M
- **前置**：无（与现有逻辑正交，缺省 = 当前行为）
- **验收**：为某模型配置自定义阈值后，该模型在自定义点触发压缩；未配置模型行为不变。
- **启动话术**：「给压缩触发加 per-model 阈值覆盖：在 `src/context/budget/pressure.rs` 的 ratio 阈值上叠一层‘按当前 model 查 model_catalog 的可选 override’，无 override 走现有比例逻辑（向后兼容）。改 config types 加 per-model warning/critical 字段。这是‘新增配置’不是重写压缩策略。」

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

1. **先 G6**（验证，可能零成本）→ **G4 / G1**（独立新功能，收益清晰，互不依赖，可并行排期）。
2. **G5** 只在文档/沟通层澄清，不进开发队列。
3. **G2+G3** 单独拉一次架构决策会（信任模型），决策通过后再按 L 级子系统排期；否则维持现状并在 FEATURE_LOCATOR §6.2 标注"刻意不实现"。
</content>
