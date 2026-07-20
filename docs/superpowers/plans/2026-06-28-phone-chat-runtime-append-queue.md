# Phone Chat Runtime-Append Queue (Ghost Bubbles) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the wide chat's runtime-append queue (ghost bubbles + turn-boundary Steer flush + ⚡ force-insert) into the phone composer, so appending a message mid-run stages it as a ghost bubble instead of sending it blindly.

**Architecture:** Single-file change to `interfaces/webchat/src/platform/phone/chat/composer.rs`. All infrastructure is already shared and live on phone — ghost rendering (`QueuedGhosts` inside the shared `MessageList`), the `flush_pulse` turn-boundary bump (`events.rs`), `ChatState` queue state/ops, the host-tested pure decisions, and queue reset on new-chat. The phone composer is the only consumer that still sends directly mid-run; we replace that with the wide behavior set, reduced to phone's surface (no attachments, no model override, no slash/mention/voice).

**Tech Stack:** Rust + Leptos 0.7 (WASM), `shared_ui_logic` pure helpers, `ChatApi` JSON-RPC. Build via `just wasm`.

## Global Constraints

- **Scope:** only `interfaces/webchat/src/platform/phone/chat/composer.rs` changes. No Core, no shared-logic, no other panel file, no new dependency, no new RPC.
- **R4 (pure I/O):** the composer only marshals input → `ChatApi::send`/`abort` and renders state. No business logic, persistence, or planning.
- **No client-side prompt-injection guard** on enqueue — matches phone's existing send path (server is the prompt-injection authority).
- **Phone reductions:** queued/flushed sends always pass `attachments = Vec::new()` and `model_override = None`. No `expand_doctor_command`, slash-commands, @-mentions, or voice.
- **Steered-send run_id is ignored** — `active_run_id` is owned by the `run_accepted` event; a mid-run send emits none.
- **Button visibility matrix (running):** draft typed → `＋ ⚡ ◼`; no draft + ghosts exist → `⚡ ◼`; no draft + no ghosts → `◼`. Idle → `▲` Send. (This matrix governs: `＋` is *shown when there is a draft*, not shown-and-disabled — it resolves the minor wording overlap in spec §3.6.)
- **Build policy (project convention "极度节制 cargo"):** the implementer does NOT run `cargo`/`just`. The controller runs `just wasm` once as the compile gate after the task.

## Reference (already in the codebase — do not modify)

- `shared_ui_logic::state::should_auto_drain_on_settle(was_busy: bool, is_busy: bool, queue_len: usize, user_interrupted: bool) -> bool`
- `shared_ui_logic::state::should_flush_on_turn_boundary(queue_len: usize, is_busy: bool) -> bool`
- `crate::views::chat::state::QueuedPrompt { pub text: String, pub attachments: Vec<PendingAttachment> }`
- `ChatState` methods (all exist): `enqueue_prompt(QueuedPrompt)`, `drain_all_queued() -> Vec<QueuedPrompt>`, `push_user_message(&str)`, `set_send_error(ChatSendError)`; signals `prompt_queue: RwSignal<Vec<QueuedPrompt>>`, `flush_pulse: RwSignal<u32>`, `active_run_id`, `session_key`, `agent_id`, `active_project_root`, `phase`.
- `ChatApi::send(&dash, text, session_key, attachments, agent_id, project_root, model) -> Result<SendResponse, _>` and `ChatApi::abort(&dash, run_id)`.
- `QueuedGhosts` (in `platform/wide/views/chat/messages.rs`) already renders ghosts at the stream tail with ✕-delete + tap-to-edit; phone's `thread.rs` mounts the shared `MessageList`. **No change needed.**

---

### Task 1: Phone composer — runtime-append queue, flush Effects, ⚡ force-insert

**Files:**
- Modify (full rewrite): `interfaces/webchat/src/platform/phone/chat/composer.rs`

**Interfaces:**
- Consumes: the Reference symbols above (all pre-existing).
- Produces: nothing new for other tasks (terminal task; `PhoneComposer` signature unchanged: `pub fn PhoneComposer() -> impl IntoView`).

**Behavior contract this task must satisfy (verified at runtime QA):**
- B1 idle: Enter/Send → send, run starts.
- B2 running: Enter or ＋ → ghost bubble at stream bottom; composer clears.
- B3 running: append again → second ghost stacks.
- B4 turn boundary with ghosts queued → whole batch steers into the live run.
- B5 natural settle (busy→idle) with ghosts → batch flushes as a fresh run.
- B6 Stop with ghosts → run aborts, ghosts remain.
- B7 ⚡ Force with draft and/or ghosts → interrupt + flush everything as a new run.
- B8 ⚡ Force with no run → plain send.
- B9 ghost ✕ delete / tap-to-edit → handled by shared `QueuedGhosts` (text only on phone).
- B10 ✎ new chat / cold boot → no stale ghosts (shared `clear_session` resets the queue).

- [ ] **Step 1: Read the current file and orient**

Read `interfaces/webchat/src/platform/phone/chat/composer.rs` in full (it is ~112 lines). Note that today `send()` has no `running()` guard, so a mid-run Enter sends directly. This task replaces that with enqueue-while-running plus the flush machinery. Do not touch any other file.

- [ ] **Step 2: Replace the file with the complete implementation below**

Overwrite `interfaces/webchat/src/platform/phone/chat/composer.rs` with exactly this content:

```rust
//! Minimal phone composer with a runtime-append queue. While a run is active,
//! Enter (or the ＋ button) stages the draft as a ghost bubble instead of
//! sending it blindly; the batch auto-flushes at the next turn boundary (Steer)
//! or when the run settles naturally, and ⚡ force-inserts immediately by
//! interrupting the run. Faithful subset of the wide composer's queue flow,
//! reduced to phone's surface: no attachments, no model override, no
//! slash-commands / @-mentions / voice (server remains the prompt-injection
//! authority). Ghost bubbles render in the shared `MessageList`; this file only
//! feeds the shared `ChatState` queue and the existing `ChatApi` send/abort.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::chat::ChatApi;
use crate::context::DashboardState;
use crate::views::chat::state::{ChatPhase, ChatSendError, QueuedPrompt};
use crate::views::chat::ChatState;
use shared_ui_logic::state::{should_auto_drain_on_settle, should_flush_on_turn_boundary};

#[component]
#[must_use]
pub fn PhoneComposer() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();

    let input_text = RwSignal::new(String::new());
    let is_sending = RwSignal::new(false);
    // Set by Stop to suppress exactly one auto-drain (B6 — Stop keeps ghosts).
    let user_interrupted = RwSignal::new(false);

    // True while a run is in flight → the composer shows Queue/Force/Stop.
    let running = move || {
        matches!(chat.phase.get(), ChatPhase::Thinking | ChatPhase::Streaming)
            || chat.active_run_id.get().is_some()
    };

    // Phone has no attachments, so a draft is just non-empty text.
    let has_draft = Memo::new(move |_| !input_text.get().trim().is_empty());

    // Force-insert is available while a run is active and there's something to
    // insert — queued ghosts or the current draft.
    let can_force = Memo::new(move |_| {
        chat.active_run_id.get().is_some()
            && (!chat.prompt_queue.get().is_empty() || !input_text.get().trim().is_empty())
    });

    // Idle send: start a fresh run. Unchanged from the original composer.
    let send = move || {
        if is_sending.get_untracked() {
            return;
        }
        let text = input_text.get_untracked().trim().to_string();
        if text.is_empty() {
            return;
        }
        is_sending.set(true);
        input_text.set(String::new());
        chat.push_user_message(&text);

        let session_key = chat.session_key.get_untracked();
        let agent_id = chat.agent_id.get_untracked();
        let project_root = chat.active_project_root.get_untracked();
        let dash = dashboard;
        spawn_local(async move {
            let res = ChatApi::send(
                &dash,
                &text,
                session_key.as_deref(),
                Vec::new(),
                agent_id.as_deref(),
                project_root.as_deref(),
                None,
            )
            .await;
            match res {
                Ok(resp) => chat.session_key.set(Some(resp.session_key)),
                Err(e) => chat.set_send_error(ChatSendError::classify(e)),
            }
            is_sending.set(false);
        });
    };

    // Queue a follow-up while a run is active → it becomes a ghost bubble.
    // No client-side prompt-injection guard (server is the authority).
    let enqueue = move || {
        let text = input_text.get_untracked().trim().to_string();
        if text.is_empty() {
            return;
        }
        chat.enqueue_prompt(QueuedPrompt {
            text,
            attachments: Vec::new(),
        });
        input_text.set(String::new());
    };

    // Flush the entire queue. While a run is active each send Steer-injects into
    // the live session (picked up at the next turn boundary); when idle the first
    // send starts a fresh run and the rest steer into it. Sends are awaited
    // sequentially so the backend coalesces them in order. The returned run_id of
    // a steered send is intentionally ignored — `active_run_id` is owned by the
    // `run_accepted` event, and a steered send emits none.
    let flush_queue = move || {
        let batch = chat.drain_all_queued();
        if batch.is_empty() {
            return;
        }
        let session_key = chat.session_key.get_untracked();
        let agent_id = chat.agent_id.get_untracked();
        let project_root = chat.active_project_root.get_untracked();
        let dash = dashboard;
        spawn_local(async move {
            for entry in batch {
                chat.push_user_message(&entry.text);
                match ChatApi::send(
                    &dash,
                    &entry.text,
                    session_key.as_deref(),
                    Vec::new(),
                    agent_id.as_deref(),
                    project_root.as_deref(),
                    None,
                )
                .await
                {
                    Ok(resp) => chat.session_key.set(Some(resp.session_key)),
                    Err(e) => chat.set_send_error(ChatSendError::classify(e)),
                }
            }
        });
    };

    // Force-insert (B7): fold the current draft into the queue, then interrupt the
    // run WITHOUT setting `user_interrupted` — so the resulting busy→idle settle
    // runs the normal auto-drain, flushing the whole queue as a fresh run. With no
    // active run it degrades to a plain send (B8).
    let force_insert = move || {
        if chat.active_run_id.get_untracked().is_none() {
            send();
            return;
        }
        enqueue(); // no-op when the draft is empty
        user_interrupted.set(false); // ensure the upcoming settle is NOT suppressed
        if let Some(run_id) = chat.active_run_id.get_untracked() {
            let dash = dashboard;
            spawn_local(async move {
                let _ = ChatApi::abort(&dash, &run_id).await;
            });
        }
    };

    // Stop (B6): abort and suppress exactly one auto-drain so the queued ghosts
    // aren't immediately re-fired (the "Stop does nothing" trap).
    let stop = move || {
        user_interrupted.set(true);
        let Some(run_id) = chat.active_run_id.get_untracked() else {
            return;
        };
        let dash = dashboard;
        spawn_local(async move {
            let _ = ChatApi::abort(&dash, &run_id).await;
        });
    };

    // Queue auto-drain — when a run settles naturally (busy→idle), flush the
    // queue. An explicit Stop set `user_interrupted`, suppressing exactly one
    // drain. The decision is the pure `should_auto_drain_on_settle`.
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
        if was_busy && !is_busy {
            user_interrupted.set(false);
        }
        is_busy
    });

    // Turn-boundary flush — `events.rs` bumps `flush_pulse` when the agent crosses
    // into a new Think iteration with prompts still queued. Steer the whole batch
    // into the live run now (pure decision: `should_flush_on_turn_boundary`).
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

    // Enter sends (idle) or queues (running); Shift+Enter inserts a newline.
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            if running() {
                enqueue();
            } else {
                send();
            }
        }
    };

    view! {
        <div style="flex:none; display:flex; align-items:flex-end; gap:8px; padding:8px 12px calc(8px + env(safe-area-inset-bottom)); border-top:1px solid var(--color-border-subtle); background:var(--color-surface);">
            <textarea
                prop:value=move || input_text.get()
                on:input=move |ev| input_text.set(event_target_value(&ev))
                on:keydown=on_keydown
                placeholder="Message…"
                rows="1"
                style="flex:1; resize:none; max-height:140px; min-height:38px; padding:9px 12px; border:1px solid var(--color-border); border-radius:var(--radius-xl); background:var(--color-surface-raised); color:var(--color-text-primary); font:inherit; font-size:15px; outline:none;"
            ></textarea>
            {move || if running() {
                view! {
                    <div style="flex:none; display:flex; align-items:flex-end; gap:8px;">
                        <Show when=move || has_draft.get()>
                            <button
                                on:click=move |_| enqueue()
                                style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:var(--color-surface-raised); color:var(--color-text-secondary); cursor:pointer; display:flex; align-items:center; justify-content:center;"
                                aria-label="Queue"
                            ><svg width="18" height="18" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M10 3a.75.75 0 0 1 .75.75v5.5h5.5a.75.75 0 0 1 0 1.5h-5.5v5.5a.75.75 0 0 1-1.5 0v-5.5h-5.5a.75.75 0 0 1 0-1.5h5.5v-5.5A.75.75 0 0 1 10 3Z" clip-rule="evenodd"></path></svg></button>
                        </Show>
                        <Show when=move || can_force.get()>
                            <button
                                on:click=move |_| force_insert()
                                style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:color-mix(in oklch, var(--color-primary) 15%, transparent); color:var(--color-primary); cursor:pointer; display:flex; align-items:center; justify-content:center;"
                                aria-label="Force insert"
                            ><svg width="18" height="18" viewBox="0 0 20 20" fill="currentColor"><path d="M11 3 4 11h4l-1 6 7-8h-4l1-6Z"></path></svg></button>
                        </Show>
                        <button
                            on:click=move |_| stop()
                            style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:var(--color-danger); color:white; cursor:pointer; display:flex; align-items:center; justify-content:center;"
                            aria-label="Stop"
                        ><svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"></rect></svg></button>
                    </div>
                }.into_any()
            } else {
                view! {
                    <button
                        on:click=move |_| send()
                        style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:var(--color-primary); color:white; cursor:pointer; display:flex; align-items:center; justify-content:center;"
                        aria-label="Send"
                    ><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="19" x2="12" y2="5"></line><polyline points="5 12 12 5 19 12"></polyline></svg></button>
                }.into_any()
            }}
        </div>
    }
}
```

- [ ] **Step 3: Self-review the transcription**

Confirm, by reading the file you just wrote:
- Imports include `QueuedPrompt` and the two `shared_ui_logic::state` helpers; nothing else added.
- Closures are defined before use: `send`/`enqueue`/`flush_queue` before `force_insert`; all before the Effects and `on_keydown`.
- `flush_queue` and `send` pass `Vec::new()` for attachments and `None` for model.
- `stop` sets `user_interrupted.set(true)`; `force_insert` sets `user_interrupted.set(false)`.
- Running button group renders `＋` only under `Show when=has_draft`, `⚡` only under `Show when=can_force`, `◼` always; idle renders only `▲`.
- No other file was touched.

- [ ] **Step 4: Compile gate (CONTROLLER runs — implementer does not run cargo/just)**

The controller runs:
```bash
just wasm
```
Expected: exits 0 (tailwind + WASM compile + wasm-opt + dist check all pass). Any compile error here is a transcription bug in Step 2 — fix and re-run. The `dist/*` artifacts will be regenerated (expected, committed in Step 5).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/phone/chat/composer.rs interfaces/webchat/dist/aleph_panel.js interfaces/webchat/dist/aleph_panel_bg.wasm interfaces/webchat/dist/tailwind.css
git commit -m "panel: phone chat runtime-append queue (ghost bubbles + turn-boundary flush + force-insert)"
```

---

## Verification summary

- **Compile gate:** `just wasm` exits 0 (controller).
- **No new pure logic** is introduced; all decisions reuse already-host-tested helpers (`should_flush_on_turn_boundary`, `should_auto_drain_on_settle`) and already-tested `ChatState` queue ops. Existing `shared-ui-logic` / `aleph-panel` queue tests are untouched and remain green. No new unit test is added for the trivial view glue (`if running { enqueue } else { send }`) — YAGNI.
- **Runtime QA (owner, post-merge):** iPhone simulator, full macOS app with a freshly rebuilt `aleph-server` binary (panel is embedded at compile time — see `DESKTOP_SHELL.md`). Walk B1–B10. Watch: ghosts land at the stream bottom (not fighting the Todo panel slot), Stop keeps ghosts, ✎/tab-switch leaves no stale ghosts.

## Self-Review (plan author)

- **Spec coverage:** §3.1 signals → Step 2 (`user_interrupted` added). §3.2 derived (`has_draft`, `can_force`) → Step 2. §3.3 closures (`enqueue`, `flush_queue`, `force_insert`, `stop`) → Step 2. §3.4 Effects (idle drain, turn-boundary flush) → Step 2. §3.5 keydown → Step 2. §3.6 buttons → Step 2 view + Global Constraints matrix. §4 B1–B10 → Task 1 behavior contract + runtime QA. §5 simplifications (no guard, partial duplication, no doctor/slash/voice) → Global Constraints + docstring. §6 testing → Verification summary. §7 R4/zero-Core → Global Constraints. §8 out of scope (iPad/attachments/hook/deploy) → not in plan. No gaps.
- **Placeholder scan:** none — Step 2 contains the complete file; commands are exact.
- **Type consistency:** `QueuedPrompt { text, attachments }`, helper signatures, and `ChatApi::send` arity match the verified Reference block. `should_auto_drain_on_settle(was_busy, is_busy, queue_len, user_interrupted)` arg order matches `composer_queue.rs`.
- **Resolved ambiguity:** spec §3.6 said `＋` is "disabled when !has_draft" but its matrix omits `＋` when there is no draft; the plan uses `Show when=has_draft` (matrix governs) — documented in Global Constraints.
