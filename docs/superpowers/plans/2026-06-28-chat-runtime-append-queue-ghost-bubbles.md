# 运行中追加发言 — 幽灵气泡 + 回合边界插入 + 强制插队 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Panel 单聊"运行中追加发言"从「客户端干等到 run 结束的输入框上方 chip」改造成「对话流底部幽灵气泡 + 回合边界 Steer 插入 + Esc/⚡ 强制插队」，接通前后端、零 Core 改动。

**Architecture:** 纯前端（`interfaces/webchat/` = `aleph-panel` crate + `shared/ui_logic/` = `shared-ui-logic` crate）。客户端 `ChatState::prompt_queue` 暂存幽灵；`events.rs` 在 `agent_trace.turn_started` 回合边界 bump 一个 `flush_pulse`；composer 的 Effect 据此把整批经 `ChatApi::send` 冲出去——运行中走后端既有 **Steer**（注入活跃会话、下个 turn 织入），空闲则起新 run。强制插队 = 折入草稿 + `chat.abort`（不设抑制标志）→ 复用既有 busy→idle 自动排空。

**Tech Stack:** Rust + Leptos (WASM)；reactive `RwSignal`/`Effect`/`Memo`；i18n `t_string!`；后端 JSON-RPC `chat.send`（默认 `BusyInputMode::Steer`）/ `chat.abort`（均已存在，不改）。

## Global Constraints

- **零 Core / Gateway / harness 改动**：仅改 `interfaces/webchat/` 与 `shared/ui_logic/`。复用现有 `chat.send` + `chat.abort`，不新增 RPC（spec 非目标）。
- **R4**：Panel 是纯 I/O，不在前端处理业务逻辑。
- **范围 = wide 单聊**：不碰 phone 端、不碰 team chat 路由分支。
- **构建策略（项目约定）**：实现者**不跑 cargo**；由控制器批量验证。WASM 构建门 = `just wasm`；host 单测 = `cargo test -p shared-ui-logic <filter>` / `cargo test -p aleph-panel <filter>`。
- **提交规范**：English，`panel: <desc>` / `ui-logic: <desc>`。
- **幽灵颜色沿用 Panel 深色靛蓝**：Tailwind `primary` / `border-dashed border-primary/60 bg-primary/10`。
- **run_id 语义**：运行中冲队的 `chat.send` 返回新 run_id 但被 Steer 进原 run（`execute.rs:193` 在 `RunAccepted` emit 之前 `return Ok(())`）→ **忽略该返回 run_id**；`active_run_id` 只由 `run_accepted` 事件驱动。

---

### Task 1: 纯函数 — 回合边界冲队判定 (`shared-ui-logic`)

**Files:**
- Modify: `shared/ui_logic/src/state/composer_queue.rs`
- Modify: `shared/ui_logic/src/state/mod.rs:9`

**Interfaces:**
- Produces: `pub const fn should_flush_on_turn_boundary(queue_len: usize, is_busy: bool) -> bool`（Task 3 在 composer 的回合边界 Effect 里消费）。

- [ ] **Step 1: 写失败测试** — 在 `composer_queue.rs` 的 `#[cfg(test)] mod tests` 内追加：

```rust
    #[test]
    fn flushes_on_boundary_when_busy_with_queue() {
        assert!(should_flush_on_turn_boundary(1, true));
    }

    #[test]
    fn no_flush_when_queue_empty() {
        assert!(!should_flush_on_turn_boundary(0, true));
    }

    #[test]
    fn no_flush_when_idle() {
        assert!(!should_flush_on_turn_boundary(2, false));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p shared-ui-logic composer_queue`
Expected: FAIL — `cannot find function should_flush_on_turn_boundary`.

- [ ] **Step 3: 写实现** — 在 `composer_queue.rs` 紧接 `should_auto_drain_on_settle` 之后追加：

```rust
/// Decide whether queued prompts should be flushed mid-run at a turn
/// boundary (the agent just crossed into a new Think iteration). Unlike
/// [`should_auto_drain_on_settle`], this fires *while the run is still
/// active* — the flush rides the gateway's Steer path so the agent weaves
/// the queued prompts into the ongoing run at its next turn.
#[must_use]
pub const fn should_flush_on_turn_boundary(queue_len: usize, is_busy: bool) -> bool {
    is_busy && queue_len > 0
}
```

- [ ] **Step 4: 导出** — 在 `shared/ui_logic/src/state/mod.rs:9` 改：

```rust
pub use composer_queue::{should_auto_drain_on_settle, should_flush_on_turn_boundary};
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p shared-ui-logic composer_queue`
Expected: PASS（含原有 5 个 + 新增 3 个）。

- [ ] **Step 6: Commit**

```bash
git add shared/ui_logic/src/state/composer_queue.rs shared/ui_logic/src/state/mod.rs
git commit -m "ui-logic: add should_flush_on_turn_boundary pure decision"
```

---

### Task 2: ChatState 状态管线 (`aleph-panel`)

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state.rs`

**Interfaces:**
- Produces:
  - `pub flush_pulse: RwSignal<u32>`（ChatState 字段，Task 3 在 composer Effect 读 / `events.rs` bump）。
  - `pub fn drain_all_queued(&self) -> Vec<QueuedPrompt>`（Task 3 `flush_queue` 消费）。
  - `pub fn queue_preview_label(entry: &QueuedPrompt) -> String`（Task 5 幽灵气泡消费）。
- Consumes: 既有 `QueuedPrompt { text: String, attachments: Vec<PendingAttachment> }`(state.rs:26)、`enqueue_prompt`(508)、`prompt_queue`(322)。

- [ ] **Step 1: 写失败测试** — 在 `state.rs` 文件末尾新增独立测试模块（镜像现有 `step_tests` 的 `Owner` 构造法）：

```rust
#[cfg(test)]
mod queue_tests {
    use super::*;

    fn prompt(text: &str, attachments: usize) -> QueuedPrompt {
        QueuedPrompt {
            text: text.to_string(),
            attachments: (0..attachments)
                .map(|i| PendingAttachment {
                    name: format!("f{i}"),
                    mime_type: "text/plain".into(),
                    data_base64: String::new(),
                    size: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn label_uses_trimmed_text() {
        assert_eq!(queue_preview_label(&prompt("  hello  ", 0)), "hello");
    }

    #[test]
    fn label_truncates_on_codepoint_boundary() {
        let long = "a".repeat(100);
        let out = queue_preview_label(&prompt(&long, 0));
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 65); // 64 chars + ellipsis
    }

    #[test]
    fn label_falls_back_to_attachment_count() {
        assert_eq!(queue_preview_label(&prompt("   ", 2)), "📎 2");
    }

    #[test]
    fn drain_all_queued_empties_and_preserves_order() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.enqueue_prompt(prompt("a", 0));
        chat.enqueue_prompt(prompt("b", 0));
        let drained = chat.drain_all_queued();
        let texts: Vec<_> = drained.iter().map(|p| p.text.clone()).collect();
        assert_eq!(texts, vec!["a", "b"]);
        assert!(chat.prompt_queue.get_untracked().is_empty());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel queue_tests`
Expected: FAIL — `cannot find function queue_preview_label` / no method `drain_all_queued` / no field `flush_pulse` (后两个在下面补)。

- [ ] **Step 3a: 加 `flush_pulse` 字段** — 在 `state.rs` 的 `pub struct ChatState` 内、`pub retry_pulse: RwSignal<u32>,`(327) 之后追加：

```rust
    /// One-shot pulse asking the composer to flush the prompt queue into the
    /// live run at a turn boundary (bumped by `events.rs` on
    /// `agent_trace.turn_started`). Ephemeral, like `retry_pulse` — excluded
    /// from [`SessionSnapshot`], so it neither snapshots nor needs clearing.
    pub flush_pulse: RwSignal<u32>,
```

- [ ] **Step 3b: 初始化字段** — 在 `ChatState::new()` 的字段初始化里、`retry_pulse: RwSignal::new(0),`(413) 之后追加：

```rust
            flush_pulse: RwSignal::new(0),
```

- [ ] **Step 3c: 加 `drain_all_queued`** — 在 `remove_queued_prompt`(526-532) 之后追加：

```rust
    /// Remove and return every queued prompt (FIFO order preserved), leaving
    /// the queue empty. Flushes the whole batch in one shot — at a turn
    /// boundary (Steer) or on the busy→idle settle.
    #[must_use]
    pub fn drain_all_queued(&self) -> Vec<QueuedPrompt> {
        let mut out = Vec::new();
        self.prompt_queue.update(|q| out = std::mem::take(q));
        out
    }
```

- [ ] **Step 3d: 加 `queue_preview_label`** — 在 `state.rs` 的 `QueuedPrompt` 结构体定义(26-29)之后追加自由函数：

```rust
/// One-line preview for a queued prompt: trimmed text (UTF-8-safe truncation,
/// P7), or an attachment-count fallback when attachments-only. Pure — the
/// ghost bubble renders whatever this returns.
#[must_use]
pub fn queue_preview_label(entry: &QueuedPrompt) -> String {
    const MAX: usize = 64;
    let text = entry.text.trim();
    if !text.is_empty() {
        let truncated: String = text.chars().take(MAX).collect();
        if truncated.chars().count() < text.chars().count() {
            format!("{truncated}…")
        } else {
            truncated
        }
    } else {
        let n = entry.attachments.len();
        format!("📎 {n}")
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel queue_tests`
Expected: PASS（4 个测试）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/state.rs
git commit -m "panel: add flush_pulse, drain_all_queued, queue_preview_label to ChatState"
```

---

### Task 3: flush_queue + 回合边界冲队 + 空闲排空改批量 (`aleph-panel`)

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs`（import:35；enqueue_message 后 ~259；idle Effect 318-341）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs`（turn_started 臂 121-131）

**Interfaces:**
- Consumes: `should_flush_on_turn_boundary`(Task 1)、`drain_all_queued`/`flush_pulse`(Task 2)、既有 `ChatApi::send`/`ChatAttachment`(composer:26)、`chat.push_user_message`(584)、`ChatSendError::classify`(state:89)。
- Produces: composer 内 `flush_queue` 闭包（Task 4 间接复用 idle 路径，无需直接调用）。

- [ ] **Step 1: 扩展 import** — `composer/mod.rs:35` 改：

```rust
use shared_ui_logic::state::{should_auto_drain_on_settle, should_flush_on_turn_boundary};
```

- [ ] **Step 2: 加 `flush_queue` 闭包** — 在 `enqueue_message` 闭包(233-258)结束的 `};` 之后、retry Effect(264)之前插入：

```rust
    // Flush the entire prompt queue into the live run in one batch. Each prompt
    // rides the normal ChatApi::send path: while a run is active the gateway
    // Steer-injects it into the live session (picked up at the next turn
    // boundary); when idle the first send starts a fresh run and the rest steer
    // into it. Sends are awaited sequentially so the backend coalesces them in
    // order. The returned run_id of a steered send is intentionally ignored —
    // `active_run_id` is owned by the `run_accepted` event, and a steered send
    // emits none (execute.rs returns Ok before the RunAccepted emit).
    let flush_queue = move || {
        let batch = chat.drain_all_queued();
        if batch.is_empty() {
            return;
        }
        let session_key = chat.session_key.get_untracked();
        let agent_id = chat.agent_id.get_untracked();
        let project_root = chat.active_project_root.get_untracked();
        let model_override = chat.selected_model.get_untracked();
        let dash = dashboard;
        spawn_local(async move {
            for entry in batch {
                chat.push_user_message(&entry.text);
                let api_attachments: Vec<ChatAttachment> = entry
                    .attachments
                    .into_iter()
                    .map(|f| ChatAttachment {
                        name: f.name,
                        mime_type: f.mime_type,
                        data_base64: f.data_base64,
                        size: f.size,
                    })
                    .collect();
                match ChatApi::send(
                    &dash,
                    &entry.text,
                    session_key.as_deref(),
                    api_attachments,
                    agent_id.as_deref(),
                    project_root.as_deref(),
                    model_override.as_ref(),
                )
                .await
                {
                    Ok(resp) => chat.session_key.set(Some(resp.session_key)),
                    Err(e) => chat.set_send_error(ChatSendError::classify(e)),
                }
            }
        });
    };
```

- [ ] **Step 3: 空闲自动排空改为批量** — 把 idle 自动排空 Effect(318-341)的 body 替换为（用 `flush_queue` 取代单条 `dequeue_prompt_front + send_message`）：

```rust
    {
        Effect::new(move |prev_busy: Option<bool>| {
            let is_busy = chat.active_run_id.get().is_some();
            let was_busy = prev_busy.unwrap_or(false);
            let queue_len = chat.prompt_queue.get_untracked().len();
            if should_auto_drain_on_settle(
                was_busy,
                is_busy,
                queue_len,
                user_interrupted.get_untracked(),
            ) {
                flush_queue();
            }
            // Reset the one-shot interrupt flag once we've crossed the edge.
            if was_busy && !is_busy {
                user_interrupted.set(false);
            }
            is_busy
        });
    }
```

- [ ] **Step 4: 加回合边界冲队 Effect** — 紧接上面 idle Effect 的 `}` 之后插入：

```rust
    // Turn-boundary flush — `events.rs` bumps `flush_pulse` when the agent
    // crosses into a new Think iteration with prompts still queued. Steer the
    // whole batch into the live run now (the pure decision is host-tested in
    // `shared_ui_logic::state::should_flush_on_turn_boundary`).
    {
        Effect::new(move |prev: Option<u32>| {
            let pulse = chat.flush_pulse.get();
            if prev.is_some() && Some(pulse) != prev {
                let is_busy = chat.active_run_id.get_untracked().is_some();
                let queue_len = chat.prompt_queue.get_untracked().len();
                if should_flush_on_turn_boundary(queue_len, is_busy) {
                    flush_queue();
                }
            }
            pulse
        });
    }
```

- [ ] **Step 5: events.rs 在回合边界 bump pulse** — 在 `events.rs` 的 `"turn_started"` 臂(121-131)里，`workspace.set_current_iteration(run_id, iteration);`(130)之后插入：

```rust
            // Turn boundary reached with prompts still queued → ask the composer
            // (which owns the send pipeline) to steer them into the live run.
            // Guarded so we only wake the flush Effect when there's something to
            // flush.
            if chat.active_run_id.get_untracked().is_some()
                && !chat.prompt_queue.get_untracked().is_empty()
            {
                chat.flush_pulse.update(|n| *n = n.wrapping_add(1));
            }
```

- [ ] **Step 6: 构建验证**

Run: `just wasm`
Expected: WASM 编译成功，无 error/warning。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs \
        interfaces/webchat/src/platform/wide/views/chat/events.rs
git commit -m "panel: wire turn-boundary flush + batch idle drain via Steer"
```

---

### Task 4: 强制插队 — Esc + ⚡ 按钮 (`aleph-panel`)

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs`（force_insert 闭包置于 keydown handler 之前；Esc 分支；can_force Memo；⚡ 按钮）
- Modify: `interfaces/webchat/locales/zh.json`、`interfaces/webchat/locales/en.json`（chat.force_insert + 修订 chat.queue 文案）

**Interfaces:**
- Consumes: 既有 `enqueue_message`/`send_message` 闭包(Copy)、`user_interrupted`(64)、`ChatApi::abort`(api:85)、`chat.active_run_id`。
- Produces: composer 内 `force_insert` 闭包、`can_force` Memo（仅 view 内消费）。

- [ ] **Step 1: 加 `force_insert` 闭包** — 必须定义在 keydown handler 之前（它捕获 `force_insert`）。在 `flush_queue` 闭包（Task 3）之后、retry Effect(264)之前插入：

```rust
    // Force-insert (B7): the user won't wait for the next turn boundary. Fold
    // the current draft into the queue, then interrupt the running task WITHOUT
    // setting `user_interrupted` — so the resulting busy→idle settle runs the
    // normal auto-drain (Task 3), flushing the whole queue as a fresh run. With
    // no active run it degrades to a normal send (B10).
    let force_insert = move || {
        if chat.active_run_id.get_untracked().is_none() {
            send_message();
            return;
        }
        enqueue_message(); // no-op when the draft is empty
        user_interrupted.set(false); // ensure the upcoming settle is NOT suppressed
        if let Some(run_id) = chat.active_run_id.get_untracked() {
            let dash = dashboard;
            spawn_local(async move {
                let _ = ChatApi::abort(&dash, &run_id).await;
            });
        }
    };
```

- [ ] **Step 2: Esc 触发强制插队** — 在 keydown handler 里、`if ev.key() == "Enter" && !ev.shift_key() {`(586)之前插入（此处已在 palette/mention 分支 return 之后，仅普通编辑态触发）：

```rust
            // Esc while a run is active = force-insert: interrupt now and flush
            // the queue (+ the current draft) as a fresh run (B7). Palette/
            // mention Esc is handled in the branch above (it returns early), so
            // this only fires in the normal composing context.
            if ev.key() == "Escape" && chat.active_run_id.get_untracked().is_some() {
                ev.prevent_default();
                force_insert();
                return;
            }
```

- [ ] **Step 3: 加 `can_force` Memo** — 在 `has_draft` Memo(625-626)之后插入：

```rust
    // Force-insert is available while a run is active and there's *something*
    // to insert — queued ghosts or the current draft.
    let can_force = Memo::new(move |_| {
        chat.active_run_id.get().is_some()
            && (!chat.prompt_queue.get().is_empty()
                || !input_text.get().trim().is_empty()
                || !attachments.get().is_empty())
    });
```

- [ ] **Step 4: 加 ⚡ 按钮** — 在 Queue 按钮 `</Show>`(896)与 Stop 按钮 `<Show ...>`(898)之间插入：

```rust
                            // Force-insert ⚡ — interrupt now and flush the queue
                            // (+ draft) immediately instead of waiting for the
                            // next turn boundary. Mirrors Esc.
                            <Show when=move || can_force.get()>
                                <button
                                    class="w-8 h-8 rounded-full bg-primary/15 text-primary flex items-center
                                           justify-center hover:bg-primary/25 transition-colors flex-shrink-0"
                                    title=move || t_string!(i18n, chat.force_insert).to_string()
                                    on:click=move |_| force_insert()
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <path d="M11 3 4 11h4l-1 6 7-8h-4l1-6Z" />
                                    </svg>
                                </button>
                            </Show>
```

- [ ] **Step 5: i18n key** — 在 `zh.json` 与 `en.json` 的 chat 块 `"clear"`(222)之后各加一行，并修订 `"queue"`(219) 文案以匹配新语义：

zh.json：
```json
    "queue": "排队，下个回合插入",
    "queued": "已排队",
    "remove": "移除",
    "clear": "清空草稿",
    "force_insert": "立即插入（中断当前任务）",
```
en.json：
```json
    "queue": "Queue (inserts next turn)",
    "queued": "Queued",
    "remove": "Remove",
    "clear": "Clear draft",
    "force_insert": "Insert now (interrupt current task)",
```
（注：`queued`/`remove`/`clear` 行不变，仅作为定位锚点；新增 `force_insert`，改 `queue`。保持 JSON 逗号合法。）

- [ ] **Step 6: 构建验证**

Run: `just wasm`
Expected: WASM 编译成功。手动确认 i18n 宏能解析 `chat.force_insert`（缺 key 会编译期报错）。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs \
        interfaces/webchat/locales/zh.json interfaces/webchat/locales/en.json
git commit -m "panel: force-insert via Esc + ⚡ button (interrupt and flush queue)"
```

---

### Task 5: 幽灵气泡进对话流，替换输入框上方 chip (`aleph-panel`)

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/messages.rs`（新增 `QueuedGhosts` 组件 + 挂载于 257/258 之间）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs`（删 `mod queue_bar;`:12、`use queue_bar::QueuedPromptBar;`:21、挂载 `<QueuedPromptBar .../>`:694）
- Delete: `interfaces/webchat/src/platform/wide/views/chat/composer/queue_bar.rs`
- Modify: `interfaces/webchat/locales/zh.json`、`en.json`（chat.queue_hint）

**Interfaces:**
- Consumes: `super::state::queue_preview_label`(Task 2)、`chat.prompt_queue`/`remove_queued_prompt`(526)/`draft_seed`(312, 既有 one-shot 预填，composer 已有消费 Effect 304-309)。
- Produces: `QueuedGhosts` 组件（仅 `MessageList` 内消费）。

- [ ] **Step 1: 加 i18n hint key** — 在 `zh.json`/`en.json` chat 块 `"force_insert"` 行之后各加：

zh.json：`    "queue_hint": "下个回合自动插入 · Esc/⚡ 立即插入",`
en.json：`    "queue_hint": "Auto-inserts next turn · Esc/⚡ to insert now",`

- [ ] **Step 2a: 补 `QueuedPrompt` 导入** — `messages.rs:8` 改为：

```rust
use super::state::{ChatMessage, ChatPhase, ChatSendErrorCode, ChatState, QueuedPrompt};
```

- [ ] **Step 2b: 写 `QueuedGhosts` 组件** — 在 `messages.rs` 的 `MessageBubble` 组件(359)之前（或 `SendErrorBanner` 之后 ~322）追加：

```rust
/// Pending follow-up prompts rendered as right-aligned "ghost" bubbles at the
/// tail of the conversation stream. They stay here until inserted: at a turn
/// boundary (Steer) they solidify into real user bubbles, or the user can ✕
/// remove / click-to-edit (pull back into the composer via `draft_seed`) /
/// Esc·⚡ force-insert. Replaces the old above-the-input chip strip so the
/// queue lives in the stream and never fights the sticky Todo panel for the
/// fixed bottom slot.
#[component]
fn QueuedGhosts() -> impl IntoView {
    use crate::views::chat::state::queue_preview_label;
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    let enumerated = move || {
        let items: Vec<(usize, QueuedPrompt)> =
            chat.prompt_queue.get().into_iter().enumerate().collect();
        items
    };

    view! {
        <Show when=move || !chat.prompt_queue.get().is_empty()>
            <div class="space-y-2 pt-1">
                <For
                    each=enumerated
                    key=|(idx, e)| format!("{}:{}", idx, e.text)
                    children=move |(idx, entry)| {
                        let label = queue_preview_label(&entry);
                        let edit_text = entry.text.clone();
                        view! {
                            <div class="flex justify-end group">
                                <div
                                    class="relative max-w-[80%] px-3.5 py-2 rounded-2xl rounded-br-md text-sm
                                           border border-dashed border-primary/60 bg-primary/10 text-primary/90
                                           cursor-text transition-colors hover:bg-primary/15"
                                    title=move || t_string!(i18n, chat.queued).to_string()
                                    on:click=move |_| {
                                        // Edit: pull back into the composer, drop from queue.
                                        chat.draft_seed.set(Some(edit_text.clone()));
                                        chat.remove_queued_prompt(idx);
                                    }
                                >
                                    <span class="absolute -top-2 right-2 text-[9px] px-1.5 rounded-full
                                                 bg-surface-sunken border border-primary/50 text-primary/80">
                                        {(idx + 1).to_string()}
                                    </span>
                                    {label}
                                    <button
                                        class="absolute -top-2 -left-2 w-4 h-4 rounded-full bg-surface-raised
                                               border border-border text-text-tertiary text-[10px] leading-none
                                               flex items-center justify-center hover:text-danger hover:border-danger/50"
                                        title=move || t_string!(i18n, chat.remove).to_string()
                                        on:click=move |ev: web_sys::MouseEvent| {
                                            ev.stop_propagation();
                                            chat.remove_queued_prompt(idx);
                                        }
                                    >
                                        "✕"
                                    </button>
                                </div>
                            </div>
                        }
                    }
                />
                <div class="flex justify-end">
                    <span class="text-[10px] text-text-tertiary pr-1">
                        {move || t_string!(i18n, chat.queue_hint).to_string()}
                    </span>
                </div>
            </div>
        </Show>
    }
}
```

注：`QueuedPrompt` 导入已在 Step 2a 补好。`web_sys`/`t_string!`/`use_i18n` 在 `messages.rs` 已可用（on_scroll/on_jump 用 `web_sys::Event`/`MouseEvent`，i18n 见 line 13）。

- [ ] **Step 3: 挂载 `QueuedGhosts`** — 在 `messages.rs` 的 thinking-indicator 外层 `</Show>`(257)与容器收尾 `</div>`(258)之间插入（落在 `max-w-3xl` 内容块尾部 = 对话流底部）：

```rust
                            // Pending follow-up ghosts — bottom of the stream,
                            // above the composer; flow into the transcript on
                            // insert. Replaces the old chip strip.
                            <QueuedGhosts />
```

- [ ] **Step 4: 移除旧 chip bar** — 在 `composer/mod.rs`：删第 12 行 `mod queue_bar;`、第 21 行 `use queue_bar::QueuedPromptBar;`、第 694 行 `<QueuedPromptBar queue=chat.prompt_queue />`。

- [ ] **Step 5: 删除文件**

```bash
git rm interfaces/webchat/src/platform/wide/views/chat/composer/queue_bar.rs
```

（label 逻辑与其单测已在 Task 2 迁入 `state.rs::queue_preview_label` / `queue_tests`，无覆盖损失。）

- [ ] **Step 6: 构建 + 测试验证**

Run: `just wasm`
Expected: 编译成功，无对 `queue_bar` 的悬空引用。

Run: `cargo test -p aleph-panel queue_tests`
Expected: PASS（label 测试已在 state.rs）。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/messages.rs \
        interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs \
        interfaces/webchat/locales/zh.json interfaces/webchat/locales/en.json
git rm interfaces/webchat/src/platform/wide/views/chat/composer/queue_bar.rs
git commit -m "panel: render queued prompts as in-stream ghost bubbles, drop chip strip"
```

---

## 运行时 QA（用户执行，full macOS app，不带 PANEL_URL）

重建链：`just wasm` → 重编 `aleph-server` → 替换运行中 binary（见 DESKTOP_SHELL.md）。逐条验：

- B2 运行中回车 → 幽灵气泡落**对话流底部**（不抢 Todo 面板固定槽）。
- B3 连续追加多条；B4 ✕ 删除；B5 点气泡正文 → 文本回到输入框、该条消失。
- B6 agent 跨回合 → 幽灵**自动实心化**成真实发言并被作答（同一 run，无新 spinner 误亮）。
- B7 Esc 与 ⚡ 各一次 → 当前任务中断、排队（含草稿）作为新 run 自动起跑。
- B8 普通 Stop → run 停、幽灵**保留**（不冲队）。
- B9 仅在最后一个 turn 追加 → run 结束后空闲兜底冲队。
- 切 tab / 新建 chat → 幽灵不残留、不串话（`prompt_queue` 已在 clear/clear_session/snapshot 全路径）。

---

## Self-Review

- **Spec 覆盖**：B1=既有 send 不变；B2/B3=Task5 幽灵 + 既有 enqueue；B4=Task5 ✕；B5=Task5 draft_seed 编辑；B6=Task3 回合边界；B7=Task4 Esc/⚡；B8=既有 on_abort 保留（设 user_interrupted）；B9=Task3 idle 兜底；B10=Task4 无 run 退化。架构：flush_queue=Task3；run_id 忽略=Task3 注释；reset=prompt_queue 既有 + flush_pulse ephemeral(Task2)。测试：Task1 纯函数 + Task2 状态。✅ 无缺口。
- **占位符扫描**：无 TBD/TODO；每步含实际代码与命令。✅
- **类型一致**：`should_flush_on_turn_boundary`(usize,bool)→bool、`drain_all_queued()→Vec<QueuedPrompt>`、`queue_preview_label(&QueuedPrompt)→String`、`flush_pulse:RwSignal<u32>`、`flush_queue`/`force_insert`/`can_force` 跨任务命名一致。✅
