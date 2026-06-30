# Chat 中间过程显示：左简右详 + 扁平化（wide）

- **Date**: 2026-06-30
- **Scope**: `interfaces/webchat`（Leptos/WASM Panel），仅 **wide**（桌面分屏）布局。phone 另起 spec。
- **Status**: Design approved — pending implementation plan。
- **Surface 红线对照**: 纯 Panel 前端改动，不碰 Core / native / Server；符合 R2（UI 唯一源）、R4（Interface 纯 I/O）、P2/P6。

## 1. 背景与问题

Chat 窗口的「中间过程」（一个 run 的 Think→Act 步骤、工具调用、推理）当前存在两个体验病灶：

1. **运行中太吵**：`messages.rs::StepStrip` 在 run 运行期默认**展开**成一个 220px 内部滚动条，把所有步骤铺开。用户其实只想知道「任务在逐步推进」，不需要全程盯着滚动条。
2. **展开后多层折叠套娃**：查看一个工具详情时，折叠层级深达 4–7 层：
   ```
   StepStrip▾ → ToolCard▾ → <details>input/result▾ → JsonViewer 递归节点▸ → 字符串 "more"
   ```
   根源是 `tool_card.rs` 的 `default_body` / `search_body` 把一个**本身就递归可折叠的 `JsonViewer` 树**又套进 `<details>` 里。

此外，左侧聊天「中间过程」与右侧「工具·详情栏」(`WorkspacePanel`) 渲染同一批 `ToolCard`，但没有清晰的「谁负责紧凑、谁负责详情」的分工。

### 参考实现的收敛结论（codex / kimi-cli / pi）

三个成熟 agent CLI（Rust/ratatui、Python/Rich、TS）在中间过程渲染上**高度一致**，提炼为「黄金法则」：

- **一步 = 一行**：动词 + 目标 + 状态；状态靠颜色/子弹/背景色编码，不靠单独徽章；时长内联隐藏。
- **永远扁平，绝不树**：步骤是兄弟节点，不是子节点。
- **只有一层展开**：内联封顶 N 行 → 溢出指向**单一**详情面（codex 的 ctrl+t pager / pi 的全局 toggle / kimi 的 ctrl+e pager）。**绝无 expand-within-expand。**
- **推理 collapse 成一条会变的行**，全文不进历史流。
- **紧凑视图与详情视图是同一数据的不同详尽度**，零漂移。
- 折叠预览 = 尾部 N 行 + 统一 `… (N more, 键 to expand)` 提示。

Aleph 比 TUI 多一个先天优势：**已有第二个表面（右侧工具·详情栏）**，正好充当 codex 用 pager 模拟的那个详情面。

## 2. 已敲定的设计决策

| # | 决策 | 取值 |
|---|------|------|
| D1 | 整体模型 | **左简右详**：左侧聊天 = 紧凑一行流；右侧「工具·详情栏」= 唯一全量详情面 |
| D2 | 左侧点开详尽度 | **内联扁平一层 + 溢出去右**：点开显示扁平体（diff/尾 N 行/搜索命中），封顶 ~8 行；溢出/点「完整」→ 右栏全量 |
| D3 | 运行中状态行 | **图标 + 最新动作 + 步数**：主行 `🔍 搜索 "美股暴跌" ⠹`，淡色副行 `└ 12 步 · 正在编辑 main.rs` |
| D4 | 范围 | 仅 **wide**；phone 另起 spec |
| D5 | 每卡 ▾ 三角 | **保留**（与所选预览一致，左右已同步）；不引入 pi 式全局开关 |

## 3. 详细设计

### 3.1 左侧三态（`platform/wide/views/chat/messages.rs::StepStrip` 重做）

`StepStrip` 是一个 run 的中间步骤容器。三态行为：

| 态 | 现状 | 新设计 |
|---|------|--------|
| **运行中** (`!completed`) | 展开 220px 滚动条 | **一条会变的行**：`{icon} {最新动作 headline} {spinner}`，下方淡色副行 `└ {N} 步 · {正在做什么}`。点击 → 展开扁平步骤流 |
| **完成·收起** (`completed`) | `N steps ▸` | `✓ {N} 步 · {末步摘要} ▸`（仍一行） |
| **展开**（任一态） | 气泡 + 内联工具体堆叠 | **扁平步骤流**：每步 = 淡色单行叙述(截断) + 该步 `ToolCard` 一行流（收起态） |

- 「最新动作」由纯函数 `latest_action_label(steps) -> String` 计算：取**末步的末个工具** headline；无工具时回落到末步叙述首行（截断）。可在宿主机 `--lib` 单测。
- 展开/收起仍按 `run_id` 存于 `ChatState`（现有 `strip_is_open`/`toggle_strip`），承受 keyed `<For>` 的每 token 重挂载。
- 展开态下每个步骤的 `ToolCard` 默认走 `default_open` 规则（文件改动展开、其余收起）。

### 3.2 ToolCard 扁平化（`components/tool_card.rs`）—— 砍嵌套核心

**头部保持现状**（图标 + 一行标题 + 运行脉冲 + diff 统计 + ▾ 箭头），只重写卡片体 `render_body`，并新增 surface 维度。

新增参数：
```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolSurface { Inline, Detail }
```
- `Inline`（左侧聊天）：body 封顶 `MAX_INLINE_LINES = 8`；溢出渲染一行 `… +N 行  → 详情栏`。
- `Detail`（右侧详情栏）：body 不封顶，全量扁平。

per-kind 卡片体（全部**扁平一层**，无 `<details>`、无递归树）：

| 工具类 | 现状 | 新 body |
|--------|------|---------|
| edit / patch | `diff_view`（无上限） | 扁平 diff，Inline 封顶；溢出 → 详情栏 |
| write / read | `CollapsibleText` | 扁平文本尾部，封顶 |
| shell (Bash) | cmd + stdout/stderr + exit | `$ {cmd}` + 尾部 stdout/stderr（封顶）+ exit 徽章 |
| **search** | count + **JsonViewer 递归树** | **扁平命中列表**：每条命中一行 `{title}` + 次行淡色 `{url}`，封顶；字段缺失时回落紧凑 pretty 块 |
| **default** | **`<details>` input/result × JsonViewer 递归** | **扁平 `key: value` 块**（一层）；嵌套值压成紧凑单行 / 紧凑 pretty `<pre>`，封顶 |

- 溢出提示统一：`… +N 行  → 详情栏`。点击 = 调用 `reveal_tool(run, iteration, tool_id)`（见 3.4），自动开右栏并定位/展开对应卡。
- 展开态(open/closed)仍走 `WorkspaceState::toggle_event(tool_id)`，**左卡▾ ⇄ 右卡▾ 自动同步**；封顶差异由 `surface` 决定（同一展开状态、不同详尽度，零漂移）。
- 错误优先渲染（现有 `error_message` 逻辑）保留，扁平展示。

#### 扁平渲染的纯逻辑（可单测）
- `search_hits(result: &Value) -> Vec<(String /*title*/, Option<String> /*url*/)>`：从 `Success.output.results[]` 提取常见字段（`title`/`name`、`url`/`link`），缺失回落。
- `flat_kv(value: &Value) -> Vec<(String, String)>`：把对象压成顶层 `key: value` 行，嵌套值用紧凑单行 JSON（`serde_json::to_string`）。
- 截断复用现有 `split_preview(text, max)`。

### 3.3 右侧详情栏 = 全量详情面（`components/workspace_panel.rs`）
- `StepCard` 内的 `ToolCard` 改用 `surface = Detail`（不封顶、全量、仍无递归树）。
- 叙述：左侧截断单行，右侧 `StepCard` 渲染**完整 markdown**（现状已是，不动）。
- 左侧「→详情栏」/点行 → `focus_step(run, it)` 交叉高亮 + 滚动定位（现有 `is_step_focused` + view.rs 的 scroll Effect）。

### 3.4 联动机制（`state/layout.rs::WorkspaceState`）

现成可复用：
- `focus_step(run, it)` —— **已会在非 Split 时自动 `set_layout(Split)`**（layout.rs:247），所以「点左侧行 → 右栏弹出并定位」几乎零新代码。
- `toggle_event(tool_id)` / `is_event_toggled` —— 左右卡展开状态共享。
- `focused_step` / `current_iteration` —— 交叉高亮 + scroll。

新增一个小 helper（加法，不改既有签名）。关键约束：`toggle_event` 存的是「相对 `default_open` **翻转过**的 tool_id 集合」，即 `expanded = default_open ^ is_event_toggled`。所以「确保展开」必须结合该 tool 的 `default_open`，且要**幂等**（已展开不再翻转，避免误折叠）：
```rust
/// 打开右栏 + 聚焦该步 + 幂等确保该工具展开。
/// `default_open` 由调用方按 ToolKind 传入（与卡片渲染同源）。
pub fn reveal_tool(
    &self,
    run_id: impl Into<String>,
    iteration: usize,
    tool_id: &str,
    default_open: bool,
) {
    self.focus_step(run_id, iteration); // 已自动开 Split（layout.rs:247）
    let expanded_now = default_open ^ self.is_event_toggled(tool_id);
    if !expanded_now {
        self.toggle_event(tool_id); // 仅在当前折叠时翻开
    }
}
```

### 3.5 推理面板（`platform/wide/views/chat/reasoning.rs`，轻改）
已是「默认折叠 + 运行时尾部预览（3 行）」，符合三家「一条会变的行」精神。仅把 `PREVIEW_TAIL_LINES` 从 3 收到 **2**，更贴近单行心跳。其余不动。

### 3.6 删除 `components/json_viewer.rs`
扁平化后 `JsonViewer` 无任何消费者（已 grep 确认唯一消费者是 `tool_card.rs` 的 search/default body 三处）→ 整文件删除（含其 5 个单测），并清理 `components/mod.rs` 的 `pub mod json_viewer;` 与 i18n 的 `json_viewer.copy` 文案。约 -349 行死代码（符合 R10/P6 YAGNI）。

## 4. 受影响文件清单

**改**
- `components/tool_card.rs` —— 扁平 body + `ToolSurface` + 溢出联动；移除 `JsonViewer`/`<details>` 用法。
- `platform/wide/views/chat/messages.rs` —— `StepStrip` 三态；`latest_action_label`。
- `platform/wide/views/chat/timeline.rs` —— `StepStrip` 行模型补「最新动作 / 末步摘要 / 步数」所需字段（如已足够则仅取数）。
- `components/workspace_panel.rs` —— `StepCard` 的 `ToolCard` 用 `surface=Detail`。
- `state/layout.rs` —— 新增 `reveal_tool` helper。
- `reasoning.rs` —— `PREVIEW_TAIL_LINES` 3→2。
- locales（`interfaces/webchat/locales/*`）—— 新文案：`步`/`正在…`/`详情栏`/`→ 详情栏`/`末步摘要`等；删 `json_viewer.copy`。

**删**
- `components/json_viewer.rs`（+ `components/mod.rs` 中的声明）。

## 5. 测试策略（宿主机 `cargo test -p aleph-panel --lib`，遵守 cargo 节制）

纯逻辑单测，无需 WASM：
- `latest_action_label(steps)` —— 末步末工具 headline / 回落叙述 / 空输入。
- `search_hits(result)` —— 标准 results 形状、字段缺失回落、空结果。
- `flat_kv(value)` —— 顶层键值、嵌套值压缩、非对象输入。
- 封顶截断 —— 复用并补强 `split_preview` 边界。
- `reveal_tool` 幂等「确保展开」—— 已展开不折叠、已折叠则展开（`WorkspaceState` host 测，参照现有 layout.rs 测试）。

既有测试（`tool_card.rs` 的 ToolKind/headline、`workspace_panel.rs` 的 timeline_groups、`timeline.rs`）须保持绿。

## 6. 默认常量（除非后续调整）
- `MAX_INLINE_LINES = 8`（Inline 封顶；Detail 不封顶）。
- 运行行副文案 `{N} 步 · {正在做什么}`。
- `reasoning.rs::PREVIEW_TAIL_LINES = 2`。
- 保留每卡 ▾ 三角（不引入全局展开开关）。

## 7. 验收（运行时 QA，需重编 server 重嵌 dist）
- L1：发起一个含多步（搜索/读取/编辑/shell）的 run：
  - 运行中左侧只显示一条会变的行 + 步数副行；不再是滚动条。
  - 完成后折成 `✓ N 步 …`；展开 = 扁平步骤流，每步一行。
  - 点开任一工具：内联只一层扁平、封顶 ~8 行；溢出显示 `… +N 行 → 详情栏`。
  - 点「→ 详情栏」/点行：右栏自动打开、定位并展开对应卡，显示全量扁平详情；**全程无递归 JSON 树、无 `<details>` 套娃**。
  - 左卡▾ 与右卡▾ 展开状态联动一致。
- L2：search/default 工具结果以扁平命中列表 / 扁平 key-value 呈现，不再是可折叠树。
- L3：ChatOnly 模式点「→详情栏」会自动切 Split（`focus_step` 行为）。

## 8. 非目标 / 后续
- phone（`platform/phone/chat/*`，无右栏）另起 spec —— 需内联扁平兜底或详情子屏。
- 不引入 pi 式全局展开开关（保留每卡三角）。
- 不改 Core/Server 的事件流与数据结构（纯前端消费现有 `messages`/`tool_payloads`）。
