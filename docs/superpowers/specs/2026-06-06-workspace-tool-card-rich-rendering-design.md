# Workspace 工具卡片富渲染设计 (Tool-Card Rich Rendering)

> Date: 2026-06-06 · Status: Approved (design) · Scope: `interfaces/webchat` only (R4 纯 I/O 渲染层)
>
> 承接：[workspace-panel-redesign](2026-06-05-workspace-panel-redesign-design.md) ·
> [workflow-echo-workspace-integration](2026-06-05-workflow-echo-workspace-integration-design.md) ·
> [workspace-trace-persistence](2026-06-06-workspace-trace-persistence-design.md) ·
> [workspace-panel-narration](2026-06-06-workspace-panel-narration-design.md)

## 1. 问题 (Problem)

WebChat 工作区面板的流式回显是「一堆工具调用 + 极少叙述」：每个工具调用只显示
`tool_name COMPLETED · 28189MS` 这样的状态行，展开后是 `JsonViewer` 渲染的**原始 JSON 树**。

参照物 `goal.md`（Claude Code 风格）是：每步有清晰中文说明、工具调用带参数摘要、
**文件修改内联展开成 diff（增/删行）**、bash 显示 `$ command` + 输出。参照物 opencode
进一步用 unified diff + 行号 + 语法高亮渲染 edit，bash 用 `$ cmd` + 输出 + 截断展开。

用户诉求（原话）：「优化 Aleph 的回显内容。尤其是读写操作和 shell(bash) 操作，
在修改文件的时候，能显示正在修改的内容（展开文件修改区域显示删除/增加行）。」

## 2. 关键事实 (Findings — 决定方案的现状)

调研结论：**渲染所需数据几乎全部已被面板捕获**，这是一个纯前端渲染任务，核心零改动。

### 2.1 数据已就位

`WorkspaceState.tool_payloads: HashMap<(run_id, tool_id), ToolPayload>`
（`state/layout.rs:90`）已存每个工具调用的 `ToolPayload { args, result }`（均为原始
`serde_json::Value`），由 `apply_trace_event`（`views/chat/events.rs`）在 live + replay
两条路径统一写入。`ToolCallEntry`（status/duration）来自 `ChatState.messages`。

各工具入参/出参（`src/builtin_tools/`）含的内容：

| 工具 | args（已捕获） | result（已捕获） | 可渲染 |
|------|------|------|------|
| `bash`/`code_exec` | `code`（命令） | `stdout, stderr, exit_code, duration_ms, truncated`（`code_exec.rs:147`） | `$ cmd` + 输出 + 退出码 |
| `file_write` | `path, content`（`write.rs:37`） | `path, bytes_written` | 全文 + 行号 |
| `file_edit` | `old_string, new_string, path`（`edit.rs:56`） | `replacements, message`（**无 diff**） | 由 old→new 构造 before→after |
| `apply_patch` | `patch`（已是 diff，`apply_patch.rs:63`） | outcome | 直接着色 patch |
| `file_read` | `path` | content | 内容 + 行号 |

> **核心不改**：`file_edit` 的 result 不含 diff，但 args 的 `old_string`/`new_string`
> 足够在前端构造 before→after。用户已选「纯前端 before→after，不改核心」。

### 2.2 当前渲染路径（两个面，各一套）

- **左侧主聊天**：`MessageBubble`（`messages.rs:375`）把工具渲染成紧凑 chips
  （状态图标 + tool_name + 耗时），点击调 `ws.focus_tool_row` 打开右面板。
  **StepStrip（`messages.rs:611`）把每个 step 渲染成 `MessageBubble`** —— 与主气泡共用同一组件。
- **右侧工作区面板**：`ActivityRow → PayloadBlock → JsonViewer`（`workspace_panel.rs:182/283`），
  渲染原始 JSON。

### 2.3 死抽象

`ToolRendererRegistry`（`components/tool_renderer.rs`，279 行，trait 注册表）在
`app.rs:72` 注册进 context，但**全局无任何活跃 `.render()` 消费者**（只有自身单测 +
`messages.rs:327` 一条过时注释引用）。其 `render(entry)` 只拿 `ToolCallEntry`，
拿不到 `ToolPayload`。属 R10/P6「零消费者抽象」范畴。

## 3. 已确认的设计决策 (Decisions)

经与用户确认：

1. **两个面都富渲染**（不是仅右面板）。
2. **`file_edit` diff = 纯前端 before→after**（用 args 的 old/new，无上下文行、无真实行号），不改核心工具。
3. **展开默认值**：`file_edit`/`file_write`/`apply_patch` 默认展开内容；`bash`/`search`/`file_read`/其余默认折叠成摘要行。
4. **范围 = 渲染 + 叙述兜底**：叙述密度由近期 prompt 侧（guidelines rule 17 等）负责，不在本 spec；
   本 spec 仅加一个前端兜底：step 无 narration 时由工具调用合成一句占位标题。
5. **架构方案 A**：删死抽象 `ToolRendererRegistry`，新建单一 `ToolCard` 组件用 enum dispatch
   （符合 P3「enum dispatch 优于 trait 膨胀」+ R10/P6 YAGNI）。
6. **新依赖**：引入 `similar`（纯 Rust、wasm 友好、行级 LCS diff），用于 `file_edit` before→after 与 `apply_patch` 解析辅助。
7. **v1 diff 不做行内语法高亮**（仅 +/- 着色）；`file_write` 全文复用现有 syntect。语法高亮留作后续可选增强。

## 4. 架构 (Architecture)

```
ChatState.messages ──┐
   (ToolCallEntry:    │   reactive lookup
    status/duration)  ├──► <ToolCard run_id tool_id tool_name/>
WorkspaceState        │        │
 .tool_payloads ──────┘        │  match ToolKind::from(tool_name)
   (ToolPayload:               ▼
    args/result)        ┌──────────────────────────────────────┐
                        │ FileEdit  → diff_view(old,new)        │
   used by BOTH:        │ FileWrite → file_view(content)        │
   - workspace_panel.rs │ ApplyPatch→ patch_view(patch)         │
     ActivityRow body   │ Bash      → shell_view(cmd,out,exit)  │
   - messages.rs        │ FileRead  → file_view(content)        │
     MessageBubble      │ Search    → search_view(query,hits)   │
     (⇒ StepStrip too)  │ Default   → <JsonViewer/> (fallback)  │
                        └──────────────────────────────────────┘
```

### 4.1 新建 `components/tool_card.rs`

单一 Leptos `#[component] fn ToolCard(run_id, tool_id, tool_name)`：

- 内部 reactive 查 `WorkspaceState.get_tool_payload(run_id, tool_id)`（snapshot），
  并从 `ChatState` 查该 tool 的 live status/duration（沿用 `ActivityRow` 现有 Memo 写法）。
- **头部行**（始终可见，可点击折叠/展开）：
  `[icon] [友好名] [status ✓/✗/⟳] [duration] [file path?] [+N −M?]`
- **可展开体**：`match ToolKind` 分流（见 §4.3）。展开状态走
  `WorkspaceState.is_event_expanded` / 现有 toggle（保持右面板深链 `focus_tool_row` 行为）。

`ToolKind` enum + `from(tool_name: &str) -> ToolKind`（小写匹配，含别名：
`bash`/`code_exec`→Bash，`file_edit`→FileEdit，`file_write`→FileWrite，
`apply_patch`→ApplyPatch，`file_read`→FileRead，`search`/`web_search`→Search，其余→Default）。

### 4.2 子渲染（纯函数 + 小组件）

- `diff_view(old: &str, new: &str)`：用 `similar::TextDiff::from_lines` 算行级 diff，
  渲染 `- 删除行`（红底）/ `+ 新增行`（绿底）/ 上下文行（中性）。头部 `+N −M` 统计由此得出。
  无真实文件行号（按决策 2）。
- `file_view(content, path)`：等宽 + 左侧行号槽。大文件（>~200 行）截断 + 「展开剩余 N 行」。
  `file_write` 复用 syntect 语法高亮（按 path 扩展名取语法）；`file_read` v1 可不高亮。
- `patch_view(patch)`：按行首 `+`/`-`/`@@` 着色渲染 patch 字符串。
- `shell_view(cmd, stdout, stderr, exit_code)`：首行 `$ {cmd}`；随后 stdout（中性）、
  stderr（暗红）；退出码徽章（0 绿 / 非 0 红）。输出 >~20 行截断 + 展开（借鉴 opencode
  `collapse-tool-output`：保留前 N 行 + `…`）。
- `search_view(query, hits)`：query 行 + 命中条数/标题列表（沿用现 `SearchToolRenderer` 思路）。

### 4.3 截断策略 (Truncation)

统一阈值常量（如 `MAX_PREVIEW_LINES = 20`）。超出时默认显示前 N 行 + `… 展开剩余 M 行`
按钮；展开后全显 + `收起`。diff 同理（超大 hunk）。防止长任务把面板撑爆（当前抱怨之一）。

### 4.4 两个面接线 (Wiring)

- **右**：`workspace_panel.rs` `PayloadBlock` 内的 `JsonViewer` 双区块替换为
  `<ToolCard .../>`（`ActivityRow` 已有 run_id/tool_id/tool_name）。
- **左**：`messages.rs` 工具区（`375–442`）的 chip 列表替换为 `<ToolCard .../>`；
  StepStrip 因共用 `MessageBubble` 自动跟随，**保留其「完成后自动折叠成一行」行为**
  （`StepStrip` 外层折叠不变，内部每个 step 的工具卡按 §3 决策 3 的默认值展开/折叠）。
- 删 `ToolRendererRegistry`：移除 `components/tool_renderer.rs`、`app.rs:28/72`、
  `mod.rs` 导出、`messages.rs:327` 过时注释。其内置 `CodeToolRenderer`/`SearchToolRenderer`
  的渲染思路迁入 `tool_card.rs` 对应分支。

### 4.5 叙述兜底 (Narration Fallback)

`workspace_panel.rs` 的 `timeline_groups`（构造 StepGroup）与左侧 iteration 标签处：
当 `narration` 为空白时，由该 step 的 `tools` 合成一句中文占位，如
「读取 3 个文件、执行 1 条命令、搜索 2 次」。纯前端派生函数 `synthesize_step_title(tools) -> String`，
不改 prompt/harness。仅在 narration 为空时使用，非空时原样渲染（不覆盖模型叙述）。

## 5. 文件改动清单 (Files)

**新增**
- `interfaces/webchat/src/components/tool_card.rs` — `ToolCard` 组件 + `ToolKind` + 子渲染 + 截断 + 单测

**修改**
- `interfaces/webchat/src/components/mod.rs` — 导出 `tool_card`，移除 `tool_renderer`
- `interfaces/webchat/src/components/workspace_panel.rs` — `PayloadBlock` 改用 `ToolCard`
- `interfaces/webchat/src/views/chat/messages.rs` — chip 列表改用 `ToolCard`；合成兜底标题；清过时注释
- `interfaces/webchat/src/app.rs` — 移除 `ToolRendererRegistry` import + provide_context
- `interfaces/webchat/Cargo.toml` — 加 `similar`
- 必要的 i18n key（友好名、「展开剩余 N 行」、兜底标题模板）

**删除**
- `interfaces/webchat/src/components/tool_renderer.rs`（279 行死代码）

**不改**：`src/`（任何核心工具、trace、协议）。守 R4。

## 6. 测试 (Testing)

- `ToolKind::from` 别名映射（bash/code_exec/file_edit/... → 正确分支，未知 → Default）。
- `diff_view` 增删统计：相同 old/new → 0/0；纯增 → +N/−0；改一行 → +1/−1。
- `synthesize_step_title`：空 tools、单工具、多工具混合的中文计数文案。
- 截断：≤阈值不截断；>阈值显示前 N 行 + 计数。
- `shell_view` 退出码徽章颜色（0 vs 非 0）。
- wasm32 编译通过（`just wasm` / `cargo build -p ... --target wasm32`）；现有面板测试不回归。

## 7. 验收 (Acceptance)

1. 工作区面板里 `file_edit` 调用展开后显示红删绿增的 before→after，头部有 `+N −M`。
2. `file_write` 展开显示全文 + 行号；`bash` 显示 `$ cmd` + 输出 + 退出码。
3. edit/write/patch 默认展开；bash/search/read 默认折叠；大输出截断可展开。
4. 左侧主聊天与 StepStrip 同样使用富卡片（StepStrip 完成后仍折叠成一行）。
5. step 无叙述时显示合成的中文占位标题。
6. `ToolRendererRegistry` 死代码已删除；`cargo`/wasm 编译干净，无回归。

## 8. 取舍 / 暂不做 (Out of Scope)

- diff 不带上下文行/真实文件行号（决策 2）。如需 opencode 级，须改核心 `file_edit` 吐 unified diff —— 另立项。
- v1 diff 行内不做语法高亮（仅 +/- 着色）；`file_read` v1 可不高亮。
- 叙述密度（prompt 侧）不在本 spec —— 由 guidelines rule 17 等负责。
- 不做实时流式部分输出（沿用「工具完成后渲染 payload」）。
