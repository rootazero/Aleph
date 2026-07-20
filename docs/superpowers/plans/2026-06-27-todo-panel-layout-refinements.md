# Todo 面板布局打磨 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把单聊 Todo 面板从对话区顶部移到输入框正上方、折叠态瘦身成一行（小环+右侧%），并把项目/模型/上下文环/导出并入附件工具栏行。

**Architecture:** 纯 Leptos 表现层改动（R4），改三个文件的 DOM 位置与 CSS，数据通路（`scratchpad`→`ChatState.plan`→`TodoPanel`）一字不改。零 core / 零协议 / 零依赖 / 零新数据连线。

**Tech Stack:** Rust + Leptos (CSR/WASM)，`interfaces/webchat`（crate `aleph-panel`）。

## Global Constraints

> 以下每条对所有 Task 隐式生效，值逐字取自 spec。

- **R4（纯 I/O）**：Panel 只读 `ChatState.plan` 投影渲染，不加业务逻辑。
- **数据通路不变**：`scratchpad`→`tool_call_completed`→`events.rs`→`ChatState.plan`→`TodoPanel`，本轮不改。
- **零 core / 零协议 / 零依赖 / 零新连线**：只动 `view.rs` / `composer/mod.rs` / `todo_panel.rs` 三个 panel 文件 + dist 重编。
- **进度环颜色**：Todo 完成度环保持 `var(--color-success)`（绿），仅缩尺寸；不与 ContextGauge 的蓝/橙/红混淆。
- **工具栏宽度策略**：合并行采用 nowrap + 既有截断（ProjectMenu chip `max-w-[160px]`、ModelPicker 自带截断）；过挤才退 `flex-wrap`（可选，QA 决定）。
- **TodoPanel 单挂载点**：全仓只有一个 `<TodoPanel/>` 挂载点（迁移后位于 `composer/mod.rs` 栈顶）。
- **构建策略（项目级，极度节制 cargo）**：**实现者不跑 cargo/just**；每个 Task 完成提交后，由**控制器**运行 `just wasm` 作编译门（期望 GREEN）。无新增可单测纯函数 → 不写空断言测试；权威验证 = 末尾运行时截图 QA。
- **提交规范**：英文 commit message，格式 `<scope>: <description>`。

---

### Task 1: 折叠态瘦身（小环 + 右侧 % + 单行）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs:37-52`（折叠 header 标记）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs:85-99`（`TODO_PANEL_CSS` 中 header/ring/meta 规则）

**Interfaces:**
- Consumes: `PlanView::{percent, done_count, total, current_step, complete}`（已存在，不改）。
- Produces: 无新 Rust 接口（纯标记/CSS）。展开态清单（`aleph-todo-rows`）保持原样供视觉回归。

**背景**：当前折叠 header 是 36px conic 圆环（环内显示 `{pct}%`）+ 两行 meta（`任务计划 · done/total` 上、`正在：current` 下）。目标：缩成与 ContextGauge 同尺寸的 18px 小环，% 移到环右侧，正文压成单行 `任务计划 · n/total · 正在：current`，整体约一行文字高。展开态不动。

- [ ] **Step 1: 替换折叠 header 标记**

把 `todo_panel.rs` 第 37-52 行这段：

```rust
                view! {
                    <div class="aleph-todo-wrap" class:done=move || complete>
                        // ── header (always visible) — click to toggle ──
                        <button
                            class="aleph-todo-head"
                            on:click=move |_| expanded.update(|e| *e = !*e)
                        >
                            <span class="aleph-todo-ring" style=ring_style>
                                <span class="aleph-todo-ring-inner">{move || format!("{pct}%")}</span>
                            </span>
                            <span class="aleph-todo-meta">
                                <b>{move || format!("任务计划 · {done}/{total}")}</b>
                                <small>{header_label}</small>
                            </span>
                            <span class="aleph-todo-chev" class:open=move || expanded.get()>"▾"</span>
                        </button>
```

替换为（小环无内文字、% 在右、正文单行；`pct`/`done`/`total`/`header_label` 已是本次渲染的定值，直接 `format!` 内联即可）：

```rust
                view! {
                    <div class="aleph-todo-wrap" class:done=move || complete>
                        // ── header (always visible) — click to toggle ──
                        // Slim single line: 18px ring (same size as ContextGauge)
                        // + percentage to its right + one-line summary that
                        // ellipsis-truncates its tail. No 36px ring, no two-row meta.
                        <button
                            class="aleph-todo-head"
                            on:click=move |_| expanded.update(|e| *e = !*e)
                        >
                            <span class="aleph-todo-ring" style=ring_style>
                                <span class="aleph-todo-ring-inner"></span>
                            </span>
                            <span class="aleph-todo-pct">{format!("{pct}%")}</span>
                            <span class="aleph-todo-line">
                                {format!("任务计划 · {done}/{total} · {header_label}")}
                            </span>
                            <span class="aleph-todo-chev" class:open=move || expanded.get()>"▾"</span>
                        </button>
```

> 说明：`ring_style`（line 30-32 的 conic-gradient）保留不变，绿色语义不动；只是环尺寸由 CSS 缩小。`header_label`（line 33-36 已算好的 `正在：current` / `已完成` / `待开始`）拼进单行。

- [ ] **Step 2: 更新 `TODO_PANEL_CSS` 的 header/ring 规则，删除废弃的 meta 规则**

把 `todo_panel.rs` 第 86-99 行这段 CSS：

```rust
.aleph-todo-wrap{margin:6px auto 0;max-width:760px;border:1px solid var(--color-border);
  border-radius:14px;background:color-mix(in oklch,var(--color-surface-overlay) 92%,transparent);
  backdrop-filter:blur(8px);overflow:hidden;font-size:13px}
.aleph-todo-head{display:flex;align-items:center;gap:12px;width:100%;padding:9px 13px;
  background:transparent;border:0;cursor:pointer;color:var(--color-text-primary);text-align:left}
.aleph-todo-ring{flex:0 0 auto;width:36px;height:36px;border-radius:50%;display:grid;place-items:center}
.aleph-todo-ring-inner{width:27px;height:27px;border-radius:50%;background:var(--color-surface-raised);
  display:grid;place-items:center;font-size:10px;font-weight:700;font-variant-numeric:tabular-nums}
.aleph-todo-meta{display:flex;flex-direction:column;gap:1px;min-width:0}
.aleph-todo-meta b{font-size:13px}
.aleph-todo-meta small{font-size:11.5px;color:var(--color-text-secondary,oklch(0.55 0.01 310));
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;max-width:52ch}
.aleph-todo-chev{margin-left:auto;font-size:11px;transition:transform .18s;color:var(--color-text-secondary,#888)}
.aleph-todo-chev.open{transform:rotate(180deg)}
```

替换为（环 18px / 内孔 12px 无字；新增 `.aleph-todo-pct` 与 `.aleph-todo-line`；删除 `.aleph-todo-meta*`；wrap 底边距改为下方留白，为 Task 2 迁移到输入框上方做准备）：

```rust
.aleph-todo-wrap{margin:0 auto 6px;max-width:760px;border:1px solid var(--color-border);
  border-radius:14px;background:color-mix(in oklch,var(--color-surface-overlay) 92%,transparent);
  backdrop-filter:blur(8px);overflow:hidden;font-size:13px}
.aleph-todo-head{display:flex;align-items:center;gap:8px;width:100%;padding:5px 12px;
  background:transparent;border:0;cursor:pointer;color:var(--color-text-primary);text-align:left;font-size:13px}
.aleph-todo-ring{flex:0 0 auto;width:18px;height:18px;border-radius:50%;display:grid;place-items:center}
.aleph-todo-ring-inner{width:12px;height:12px;border-radius:50%;background:var(--color-surface-raised)}
.aleph-todo-pct{flex:0 0 auto;font-size:11px;font-weight:700;font-variant-numeric:tabular-nums;
  color:var(--color-text-secondary,#888)}
.aleph-todo-line{flex:1 1 auto;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
  color:var(--color-text-primary)}
.aleph-todo-chev{flex:0 0 auto;margin-left:auto;font-size:11px;transition:transform .18s;
  color:var(--color-text-secondary,#888)}
.aleph-todo-chev.open{transform:rotate(180deg)}
```

> `.aleph-todo-rows` 及其后所有展开态/动画规则（line 100 起）**保持不动**。

- [ ] **Step 3: 自查 diff**

确认：① 折叠 header 不再有环内文字；② `.aleph-todo-pct`/`.aleph-todo-line` 已新增且被标记引用；③ `.aleph-todo-meta*` 三条规则已删净（无孤儿 CSS）；④ 展开态 `<Show>`/`<For>`/`aleph-todo-rows` 区块未被触碰。

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs
git commit -m "panel: slim collapsed Todo header to one line (18px ring + right-side %)"
```

- [ ] **Step 5: 编译门（控制器执行，实现者不跑）**

控制器运行：`just wasm`
期望：`✓ WASM` GREEN，无编译错误。

---

### Task 2: 把 Todo 面板搬到输入框正上方

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/view.rs:12`（删 `use super::TodoPanel;`）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/view.rs:210-214`（删顶部挂载块）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:23`（加 `use super::TodoPanel;`）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:684-685`（栈顶挂 `<TodoPanel/>`）

**Interfaces:**
- Consumes: `super::TodoPanel`（`pub use todo_panel::TodoPanel;` 已在 `chat/mod.rs:22` 导出；从 `composer/mod.rs` 视角 `super` = `chat` 模块）。
- Produces: 迁移后 `<TodoPanel/>` 是 `node_ref=stack_ref` 中心栈的首个子元素。

**背景**：当前 TodoPanel 浮在对话区顶部（`view.rs` `absolute top-0 z-[11]`）。要移到底部输入浮层中心栈（`max-w-3xl mx-auto`，`node_ref=stack_ref`）的最顶端，使其位于消息流之下、输入框之上。进入 `stack_ref` 后，现有 ResizeObserver（`--composer-clearance`）会自动为其高度预留净空，无需新增代码。

- [ ] **Step 1: 删 `view.rs` 顶部挂载块**

删除 `view.rs` 第 210-214 行：

```rust
                    // Single-chat sticky Todo panel — below the tab strip,
                    // above the message flow. Hidden when no active plan.
                    <div class="absolute inset-x-0 top-0 z-[11] px-3 pt-9 pointer-events-none">
                        <div class="pointer-events-auto"><TodoPanel /></div>
                    </div>
```

删除后，上方紧邻的 `<div class="absolute inset-x-0 top-0 z-10"><SessionTabs /></div>`（line 209）与下方的 TeamParticipants `<Show>`（line 230 起）保持原样。

- [ ] **Step 2: 删 `view.rs` 的 TodoPanel 导入**

删除 `view.rs` 第 12 行：

```rust
use super::TodoPanel;
```

（删后 `view.rs` 不再引用 `TodoPanel`，避免未用导入告警。）

- [ ] **Step 3: 在 `composer/mod.rs` 加 TodoPanel 导入**

在 `composer/mod.rs` 第 23 行 `use super::project_menu::ProjectMenu;` 之后新增一行：

```rust
use super::project_menu::ProjectMenu;
use super::TodoPanel;
```

- [ ] **Step 4: 在底部栈顶挂 `<TodoPanel/>`**

把 `composer/mod.rs` 第 684-685 行：

```rust
            <div class="max-w-3xl mx-auto pointer-events-auto" node_ref=stack_ref>
                <AttachmentPreviewBar attachments=attachments />
```

替换为：

```rust
            <div class="max-w-3xl mx-auto pointer-events-auto" node_ref=stack_ref>
                // Single-chat sticky Todo panel — top of the bottom input
                // stack (below the message flow, above the input box).
                // Hidden when no active plan. Living inside `stack_ref` lets
                // the existing ResizeObserver reserve `--composer-clearance`
                // for its height, so messages never hide behind it.
                <TodoPanel />
                <AttachmentPreviewBar attachments=attachments />
```

- [ ] **Step 5: 自查 diff**

确认：① `view.rs` 既无 `TodoPanel` 挂载也无其导入；② `composer/mod.rs` 有且仅有一处 `<TodoPanel/>`（栈顶，`AttachmentPreviewBar` 之前），导入已加；③ 全仓 `<TodoPanel` 挂载点仍为 1 个：
```bash
grep -rn "<TodoPanel" interfaces/webchat/src/   # 期望仅 composer/mod.rs 一处
```

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/view.rs \
        interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs
git commit -m "panel: relocate Todo panel from chat top to above the input box"
```

- [ ] **Step 7: 编译门（控制器执行）**

控制器运行：`just wasm`
期望：`✓ WASM` GREEN。

---

### Task 3: 项目/模型/上下文环/导出 并入附件工具栏行

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:743-776`（删 `.aleph-project-row` 块）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:830`（工具栏行并入四件）

**Interfaces:**
- Consumes: `ProjectMenu`（已导入 line 23）、`crate::components::model_picker::ModelPicker`、`super::context_gauge::ContextGauge`、`super::transcript::download_transcript`（均在旧 project-row 块中已引用，仅搬位）。
- Produces: 无新接口。

**背景**：当前输入区上方有一条独立的 `.aleph-project-row`（项目、模型、上下文环、导出），与下方输入卡片内的附件📎工具栏分属两行。目标：解散该独立行，把四件移进附件工具栏行，与📎同水平。斜杠/提及面板与注入护栏横幅锚定在包裹卡片的 `.relative` 上、不动；项目/模型下拉仍 `bottom-full` 向上弹。

- [ ] **Step 1: 删除独立的 `.aleph-project-row` 块**

删除 `composer/mod.rs` 第 743-776 行整段（含注释、`<div class="aleph-project-row ...">` 到其闭合 `</div>`）：

```rust
                // Project + model row — both pickers sit directly above
                // the composer so their dropdowns flip upward. The
                // workspace toggle lives at the chat-surface top-right
                // (see views/chat/view.rs) so it stays at the boundary
                // when the workspace pane is open.
                <div class="aleph-project-row flex items-center gap-2 px-1 pb-1">
                    <ProjectMenu />
                    <crate::components::model_picker::ModelPicker />
                    // Live context-window gauge (mirrors hermes-desktop's
                    // ContextGauge): an SVG ring of the last turn's prompt-token
                    // occupancy. Self-hides until the first usage event lands.
                    <super::context_gauge::ContextGauge />
                    // Export conversation → Markdown download. Pushed to the far
                    // right; only present once the thread has content.
                    <Show when=move || !chat.messages.get().is_empty()>
                        <button
                            class="ml-auto p-1.5 rounded-lg text-text-tertiary hover:text-text-primary
                                   hover:bg-surface-sunken transition-colors flex-shrink-0"
                            title="导出对话为 Markdown"
                            on:click=move |_| {
                                let msgs = chat.messages.get_untracked();
                                super::transcript::download_transcript(&msgs);
                            }
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4"
                                 viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                                <polyline points="7 10 12 15 17 10" />
                                <line x1="12" y1="15" x2="12" y2="3" />
                            </svg>
                        </button>
                    </Show>
                </div>
```

> 删除后，紧邻其下的 `// Composer card — two zones...` 注释 + `<div class="aleph-composer flex flex-col gap-1.5 px-3 py-2">`（原 line 778-782）成为 `.relative` 内下一个元素。

- [ ] **Step 2: 把四件并入附件工具栏行**

把 `composer/mod.rs` 工具栏行（原 line 830 起）：

```rust
                    // Toolbar row — left: attach + voice; right cluster: the
                    // conditional clear / queue / abort / send buttons.
                    <div class="flex items-center gap-2">
                        <button
                            class="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary
                                   hover:bg-surface-sunken transition-colors flex-shrink-0"
                            title=move || t_string!(i18n, chat.attach).to_string()
                            on:click=on_attach_click
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5"
                                 viewBox="0 0 20 20" fill="currentColor">
                                <path fill-rule="evenodd"
                                      d="M15.621 4.379a3 3 0 0 0-4.242 0l-7 7a3 3 0 0 0 4.241 4.243h.001l.497-.5a.75.75 0 0 1 1.064 1.057l-.498.501-.002.002a4.5 4.5 0 0 1-6.364-6.364l7-7a4.5 4.5 0 0 1 6.368 6.36l-3.455 3.553A2.625 2.625 0 1 1 9.52 9.52l3.45-3.451a.75.75 0 1 1 1.061 1.06l-3.45 3.451a1.125 1.125 0 0 0 1.587 1.595l3.454-3.553a3 3 0 0 0 0-4.242Z"
                                      clip-rule="evenodd" />
                            </svg>
                        </button>

                        // Voice loop — record → STT → send → spoken reply.
                        <voice::VoiceInputButton
                            disabled=Signal::derive(move || is_sending.get())
                        />

                        <div class="ml-auto flex items-center gap-2">
```

替换为（在语音按钮后插入 项目/模型/上下文环；把导出 `<Show>` 移到右组 `ml-auto` 容器内首位、并去掉导出按钮自身的 `ml-auto`）：

```rust
                    // Toolbar row — left: attach + voice + project/model/gauge;
                    // right cluster: export + conditional clear / queue / abort / send.
                    // (The old standalone .aleph-project-row was folded into this
                    // row so its controls sit level with the attach paperclip.)
                    <div class="flex items-center gap-2">
                        <button
                            class="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary
                                   hover:bg-surface-sunken transition-colors flex-shrink-0"
                            title=move || t_string!(i18n, chat.attach).to_string()
                            on:click=on_attach_click
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5"
                                 viewBox="0 0 20 20" fill="currentColor">
                                <path fill-rule="evenodd"
                                      d="M15.621 4.379a3 3 0 0 0-4.242 0l-7 7a3 3 0 0 0 4.241 4.243h.001l.497-.5a.75.75 0 0 1 1.064 1.057l-.498.501-.002.002a4.5 4.5 0 0 1-6.364-6.364l7-7a4.5 4.5 0 0 1 6.368 6.36l-3.455 3.553A2.625 2.625 0 1 1 9.52 9.52l3.45-3.451a.75.75 0 1 1 1.061 1.06l-3.45 3.451a1.125 1.125 0 0 0 1.587 1.595l3.454-3.553a3 3 0 0 0 0-4.242Z"
                                      clip-rule="evenodd" />
                            </svg>
                        </button>

                        // Voice loop — record → STT → send → spoken reply.
                        <voice::VoiceInputButton
                            disabled=Signal::derive(move || is_sending.get())
                        />

                        // Migrated from the old .aleph-project-row — now level
                        // with the attach paperclip. Dropdowns still flip upward.
                        <ProjectMenu />
                        <crate::components::model_picker::ModelPicker />
                        // Live context-window gauge (self-hides until first usage).
                        <super::context_gauge::ContextGauge />

                        <div class="ml-auto flex items-center gap-2">
                            // Export conversation → Markdown (far right of the
                            // cluster). Only once the thread has content.
                            <Show when=move || !chat.messages.get().is_empty()>
                                <button
                                    class="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary
                                           hover:bg-surface-sunken transition-colors flex-shrink-0"
                                    title="导出对话为 Markdown"
                                    on:click=move |_| {
                                        let msgs = chat.messages.get_untracked();
                                        super::transcript::download_transcript(&msgs);
                                    }
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4"
                                         viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                                        <polyline points="7 10 12 15 17 10" />
                                        <line x1="12" y1="15" x2="12" y2="3" />
                                    </svg>
                                </button>
                            </Show>
```

> 右组容器 `<div class="ml-auto flex items-center gap-2">` 之后原有的 clear / queue / stop / send 四个 `<Show>` 区块（原 line 850 之后）**保持不动**，依次跟在导出 `<Show>` 之后。

- [ ] **Step 3: 自查 diff**

确认：① `.aleph-project-row` 字符串全仓已消失（`grep -rn "aleph-project-row" interfaces/webchat/src/` 期望 0 命中）；② `ProjectMenu` / `ModelPicker` / `ContextGauge` / 导出 各恰好出现一次且都在工具栏行内；③ 导出按钮 class 已去掉 `ml-auto`（由右组容器统一 `ml-auto`）；④ clear/queue/stop/send 四个 `<Show>` 仍完整跟在右组内、缩进与闭合括号配对正确。

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs
git commit -m "panel: fold project/model/gauge/export into the attach toolbar row"
```

- [ ] **Step 5: 编译门（控制器执行）**

控制器运行：`just wasm`
期望：`✓ WASM` GREEN。

---

## 末尾验证（控制器，全部 Task 完成后）

无新增可单测纯函数。权威验证 = 运行时截图 QA（同原特性流程，见
`2026-06-27-single-chat-sticky-todo-panel-design.md` 记录的做法）：

1. 重编 wasm（`just wasm`）+ 重建/重启本地完整 core（`aleph-server`，重嵌新 dist）。
2. 浏览器开 `http://127.0.0.1:18790/`，发起一个会触发 `scratchpad` 计划的对话。
3. 截图确认：
   - [ ] Todo 面板出现在**输入框正上方**（不在对话区顶部）。
   - [ ] 折叠态为**单行**：18px 小环 + 右侧 `%` + `任务计划 · n/总 · 正在：…`，明显比旧 36px 卡片矮。
   - [ ] 点击展开仍出 done(✓绿勾)/active(粉高亮)/pending(空框) 三态清单 + 打勾动画。
   - [ ] 工具栏一行内含 `📎附件 语音 项目 模型 上下文环 … 导出 [清除/排队/停止/发送]`，与📎同水平、不溢出。
   - [ ] 项目/模型下拉向上弹正常；发送/停止/排队/清除/导出 功能照常。
   - [ ] 无活动计划时 Todo 面板不渲染、输入区回到无 Todo 行形态。

## 执行顺序与隔离

- Task 1（`todo_panel.rs`）与 Task 2/3（`view.rs`+`composer/mod.rs`）文件不重叠；Task 2 改 `composer/mod.rs` 栈顶（line ~685）与导入（line 23），Task 3 改其工具栏区（line 743-776、830），**区域不重叠**，可顺序执行无冲突。
- 推荐顺序：Task 1 → Task 2 → Task 3。每个 Task 结束都是独立可编译、可视觉验证的交付物。
