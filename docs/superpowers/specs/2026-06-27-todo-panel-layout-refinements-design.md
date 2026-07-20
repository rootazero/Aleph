# 单聊 Todo 面板 · 布局打磨（搬位 + 折叠瘦身 + 工具栏合并）设计

> 续作：`2026-06-27-single-chat-sticky-todo-panel-design.md`。本轮纯表现层（R4），
> 零 core / 零协议 / 零新数据连线。数据通路（`scratchpad` → `ChatState.plan` →
> `TodoPanel`）完全不变，只调整三处布局/样式。

## 目标（Goal）

落地用户三条修改意见，**彻底减少 Todo 面板对对话窗口空间的挤占**，并把输入区上方
散落的两排控件收拢为一排：

1. Todo 面板从对话区**顶部**移到「消息区下面、输入框上面」。
2. 折叠态去掉大圆环 —— 改成**与 ContextGauge 同尺寸（≈18px，一行文字高度）的小环 +
   右侧百分比**，整行压到约一行高。
3. `进入工作目录 / 模型选择 / 上下文占用% / 导出` 这一排，**下沉并入附件上传(📎)
   所在的工具栏行**，与 📎 同一水平线。

> 澄清记录：用户原话「完成度百分比」经确认即 **上下文占用比(ContextGauge)**，
> 非任务完成度 —— 故意见3 是纯布局合并，无需新连线。

## 架构合规（Constitution）

- **R4（Interface 纯 I/O）**：Panel 只读 `ChatState.plan` 投影渲染，不含业务逻辑。
- **R10 / 数据通路不变**：`scratchpad` 工具 → `tool_call_completed` wire →
  `events.rs` 投影 → `ChatState.plan` → `TodoPanel`，本轮一字不改。
- **P6（简洁）**：解散无 CSS 规则的标记类 `.aleph-project-row`；圆环只缩尺寸不换画法。

## 现状锚点（Anchors，改前）

- `view.rs:209-214` — 顶部浮层：`SessionTabs`(z-10) 之后，TodoPanel 挂在
  `absolute inset-x-0 top-0 z-[11] px-3 pt-9 pointer-events-none` 里。
- `composer/mod.rs:684` — 底部输入浮层中心栈
  `<div class="max-w-3xl mx-auto pointer-events-auto" node_ref=stack_ref>`，
  子序：`AttachmentPreviewBar` → `QueuedPromptBar` → `TeamTaskStrip` →
  `.relative{ 斜杠/提及面板, 注入护栏, .aleph-project-row, .aleph-composer }`。
- `composer/mod.rs:748-776` — `.aleph-project-row`：`ProjectMenu` · `ModelPicker` ·
  `ContextGauge` · `ml-auto` 导出按钮。
- `composer/mod.rs:830` — `.aleph-composer` 卡片内工具栏行
  `<div class="flex items-center gap-2">`：左 `📎附件` + `🎤语音`，
  `ml-auto` 右组 `✕清除 / ⊕排队 / ■停止 / ↑发送`。
- `todo_panel.rs:30-52` — 折叠态 header：36px conic 圆环(环内 `{pct}%`) +
  `任务计划 · done/total` + `正在：current` + 雪佛龙。
- `todo_panel.rs:85-116` — `aleph-todo` 自包含样式（圆环 36px、环内 27px）。

核验：`.aleph-project-row` 全仓仅此一处用、无 CSS 规则；`TodoPanel` 全仓唯一挂载点
`view.rs:213`；`aleph-todo` 样式仅 `todo_panel.rs` 内 → 三处均可安全改。

## 改动设计（Changes）

### 改动 A — 搬位（意见1）

- **删**：`view.rs` 顶部 TodoPanel 挂载块（`absolute … top-0 … <TodoPanel/>`）
  及其 `use super::TodoPanel;` 导入。
- **加**：`composer/mod.rs` 中 `use super::TodoPanel;`；把 `<TodoPanel/>` 作为
  `node_ref=stack_ref` 中心栈的**第一个子元素**（`<AttachmentPreviewBar>` 之前）。
- **净空自动处理**：TodoPanel 进入 `stack_ref` → 现有 ResizeObserver
  （`--composer-clearance`）自动为其高度预留空间（折叠≈一行、展开≈清单高），
  消息不被遮挡；无活动计划时 `<Show>` 渲染空 → 零高度 → 零净空影响。**无需新增 clearance 代码。**

### 改动 B — 折叠态瘦身（意见2，已细化）

- 折叠 header 改为单行高：
  - `.aleph-todo-ring` 36px → **18px**；`.aleph-todo-ring-inner` 27px → **12px**
    （保留甜甜圈孔，**移除环内文字**）。
  - 百分比移到环**右侧**为兄弟 `<span>`：`[环] 25%`。
  - 行内容：`[环] 25% · 任务计划 · 1/4 · 正在：<current…> ▾`；当前步骤
    `text-overflow:ellipsis` 截断；雪佛龙 `▾` 居最右。
  - 收紧 header 纵向 padding，整行 ≈ 一行文字高度。
- 进度环**保持成功绿**（`var(--color-success)` conic）—— 仅对齐尺寸，不与
  ContextGauge 的蓝/橙/红混淆。
- **展开态清单（`aleph-todo-rows` done/active/pending + 打勾/闪动动画）一字不动。**

### 改动 C — 工具栏合并（意见3）

- **删**：`composer/mod.rs:743-776` 整个 `.aleph-project-row` 块。
- **移入** `.aleph-composer` 卡片工具栏行（`:830`），最终排布：
  - 左组：`📎附件 · 🎤语音 · 📁ProjectMenu · ModelPicker · ◔ContextGauge`
  - 右组(`ml-auto`)：`⬇导出 · ✕清除 · ⊕排队 · ■停止/↑发送`
  - 导出按钮去掉原 `ml-auto`（改由右组容器统一 `ml-auto`），其余条件渲染
    （`✕/⊕/■/↑`）保持原 `<Show>` 门控不变。
- 斜杠/提及面板、注入护栏横幅不动（仍锚定包裹卡片的 `.relative`）；
  `ProjectMenu`/`ModelPicker` 下拉仍 `bottom-full` 向上弹。
- **宽度**：max-w-3xl(~768px) 下左~410px + 右~150px 不挤；项目激活时 chip 截断 160px。
  采用 **nowrap + 既有截断**；若 QA 实测过挤，退回 `flex-wrap` 作安全阀（记为可选）。

## 数据流（Data Flow）

不变。`TodoPanel` 仍 `expect_context::<ChatState>()` 读 `chat.plan`
（`RwSignal<Option<PlanView>>`），`PlanView::{percent, done_count, total, current_step,
complete, items}` 全部沿用。`ProjectMenu`/`ModelPicker`/`ContextGauge`/导出按钮内部逻辑
零改 —— 仅 DOM 位置变化。

## 错误/边界（Error & Edge）

- 无活动计划：TodoPanel `<Show when=visible>` 不渲染 → 输入区回到无 Todo 行的形态。
- 计划完成：折叠行 `complete` 时 `current_step` 为空 → header 文案回退「已完成」
  （沿用既有逻辑）。
- 展开 + 长清单：栈 `bottom-0` 锚定，展开向上挤压消息区（ResizeObserver 已处理）。

## 测试与验证（Testing）

- 纯布局/CSS，**无新增可单测纯函数** → `plan.rs` 既有投影单测、`events.rs` 回归测试
  不受影响，无需改动。
- 权威验证 = **运行时截图 QA**（同原特性流程）：重编 wasm + 重启本地 core，
  发起一个 scratchpad 计划对话，截图确认：
  1. Todo 面板出现在**输入框正上方**（非顶部）。
  2. 折叠态为**单行**：小环 + 右侧 % + `任务计划 · n/总 · 正在：…`，不再挤占对话。
  3. 点击展开仍出三态清单 + 打勾动画。
  4. 工具栏一行内含 `📎 语音 项目 模型 上下文环 … 导出 发送`，与 📎 同水平。
  5. 项目/模型下拉向上弹正常；发送/停止/排队/清除照常。

## 触及文件（Files）

- 改：`interfaces/webchat/src/platform/wide/views/chat/view.rs`（删顶部挂载 + 导入）
- 改：`interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs`
  （加 TodoPanel 挂载于栈顶；解散 project-row 并入工具栏行；加 `use super::TodoPanel;`）
- 改：`interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs`（折叠 header + CSS）
- 重编：`interfaces/webchat/dist/*`（控制器侧 `just wasm`）

## 不做（Out of Scope）

- 不动任务完成度的数据来源/语义（仍来自 `scratchpad` 计划）。
- 不动展开态清单结构与动画。
- 不动 ContextGauge/ProjectMenu/ModelPicker/导出 各自内部逻辑。
- 不动手机端（phone 聊天另有形态；本特性仅单聊 wide）。
- 不引入 core/协议/依赖变更。
