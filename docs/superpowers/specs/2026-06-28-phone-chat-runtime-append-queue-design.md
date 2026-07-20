# Phone Chat Runtime-Append Queue (Ghost Bubbles) — Design

**Date:** 2026-06-28
**Status:** Approved (design); pending spec review
**Scope:** Phone form factor only — `interfaces/webchat/src/platform/phone/chat/composer.rs`
**Predecessor:** `2026-06-28-chat-runtime-append-queue-ghost-bubbles-design.md` (the wide / single-chat
implementation, merged to local `main` at `ca4eaf88d`). This spec ports that feature to the phone
composer, reusing the shared infrastructure that work introduced.

---

## 1. Problem

On phone, appending a message while a run is active is broken/unusable. The phone composer
(`PhoneComposer`) sends every message directly — even mid-run — with no queue, no visual staging,
no batching, and no way to force-insert an urgent message. The wide chat already solved this with
**ghost bubbles + turn-boundary Steer + force-insert (Esc/⚡)**; phone never received the port.

## 2. Key finding — almost everything is already shared

The wide feature deliberately placed its infrastructure in **shared** code. Phone already consumes
all of it; only the phone composer's *behavior* is missing.

| Capability | Status on phone | Location |
|---|---|---|
| Ghost bubbles render at stream tail | **Already renders** (gated by `!prompt_queue.is_empty()`) | `platform/wide/views/chat/messages.rs::QueuedGhosts`, mounted inside shared `MessageList`; phone's `thread.rs` renders `MessageList` |
| Turn-boundary pulse | **Already bumps** on phone | `platform/wide/views/chat/events.rs` `turn_started` arm bumps `flush_pulse`; phone subscribes to the same `subscribe_run_events` handler |
| Queue state + ops | **Already present** | `ChatState::{prompt_queue, flush_pulse, enqueue_prompt, drain_all_queued, remove_queued_prompt}` (`state.rs`) |
| Pure flush/drain decisions | **Already host-tested** | `shared_ui_logic::state::{should_flush_on_turn_boundary, should_auto_drain_on_settle}` |
| New-chat resets the queue | **Already resets** | `ChatState::clear()` and `clear_session()` both `prompt_queue.set(Vec::new())` |
| Ghost ✕-delete / tap-to-edit | **Already wired** in shared `QueuedGhosts` | restores `draft_seed` + `pending_attachments`, calls `remove_queued_prompt` |

**iPad needs no code.** `app.rs:422` mounts `PhoneChat` only for `FormFactor::Phone`; every other form
factor (including iPad/Tablet) mounts the wide `ChatView`, which already carries the merged feature.
iPad is full-screen (no Split View) so it never resolves to the phone layout, and force-insert there
works via the ⚡ button on touch (no keyboard needed).

## 3. The only change — `platform/phone/chat/composer.rs`

Replace the direct-send-always behavior with the wide behavior set, **reduced** to phone's surface
(no attachments, no model override, no slash-commands / @-mentions / voice). The file grows from
~110 to ~230 lines — well under the 800-line limit.

### 3.1 Signals

- Keep: `input_text: RwSignal<String>`, `is_sending: RwSignal<bool>`.
- Add: `user_interrupted: RwSignal<bool>` — set by Stop to suppress exactly one auto-drain.

### 3.2 Derived

- `running()` (exists): `matches!(phase, Thinking | Streaming) || active_run_id.is_some()`.
- `has_draft` (new `Memo`): `!input_text.get().trim().is_empty()`.
- `can_force` (new `Memo`): `active_run_id.is_some() && (!prompt_queue.is_empty() || !input_text.trim().is_empty())`.

### 3.3 Closures

- `send` (exists, unchanged): idle send via `ChatApi::send` (attachments empty, model `None`).
- `enqueue` (new):
  ```text
  let text = input_text.trim();
  if text.is_empty() { return; }
  chat.enqueue_prompt(QueuedPrompt { text, attachments: Vec::new() });
  input_text.set(String::new());
  ```
  No client-side prompt-injection guard (see §5).
- `flush_queue` (new): mirror wide, reduced.
  ```text
  let batch = chat.drain_all_queued();
  if batch.is_empty() { return; }
  capture session_key, agent_id, project_root;
  spawn_local: for entry in batch {
      chat.push_user_message(&entry.text);
      match ChatApi::send(&dash, &entry.text, session_key, Vec::new(), agent_id, project_root, None).await {
          Ok(resp) => chat.session_key.set(Some(resp.session_key)),
          Err(e)   => chat.set_send_error(ChatSendError::classify(e)),
      }
  }
  ```
  Attachments are always empty; model override is always `None`. The returned `run_id` of a steered
  send is intentionally ignored — `active_run_id` is owned by the `run_accepted` event, and a steered
  send emits none (`execute.rs` returns `Ok` before the `RunAccepted` emit).
- `force_insert` (new):
  ```text
  if active_run_id.is_none() { send(); return; }   // B8 degrade
  enqueue();                                        // fold current draft (no-op if empty)
  user_interrupted.set(false);                      // do NOT suppress the upcoming settle
  if let Some(run_id) = active_run_id { spawn_local: ChatApi::abort(run_id) }
  ```
- `stop` (modified): set `user_interrupted.set(true)` **before** abort (B6 — Stop keeps ghosts).

### 3.4 Effects

- **Idle auto-drain** (new):
  ```text
  Effect::new(|prev_busy| {
      let is_busy = active_run_id.is_some();
      let was_busy = prev_busy.unwrap_or(false);
      if should_auto_drain_on_settle(was_busy, is_busy, prompt_queue.len(), user_interrupted) {
          flush_queue();
      }
      if was_busy && !is_busy { user_interrupted.set(false); }
      is_busy
  });
  ```
- **Turn-boundary flush** (new):
  ```text
  Effect::new(|prev_pulse| {
      let pulse = chat.flush_pulse.get();
      if prev_pulse.is_some() && Some(pulse) != prev_pulse {
          if should_flush_on_turn_boundary(prompt_queue.len(), active_run_id.is_some()) {
              flush_queue();
          }
      }
      pulse
  });
  ```

### 3.5 Keydown

Enter (without Shift) → `enqueue()` if `running()`, else `send()`. (Shift+Enter inserts a newline,
unchanged.)

### 3.6 Buttons

- **Idle:** `▲ Send` → `send()`.
- **Running** (right of the textarea, in this order):
  - `＋ Queue` → `enqueue()`; `disabled` when `!has_draft`.
  - `⚡ Force` → `force_insert()`; shown only `when can_force`.
  - `◼ Stop` → `stop()`.
- Visibility matrix while running: draft typed → `＋ ⚡ ◼`; no draft + ghosts exist → `⚡ ◼`;
  no draft + no ghosts → `◼`.

Reuse existing phone composer styling (round 38px buttons, `var(--color-*)` tokens). The `⚡` uses
the same lightning glyph and primary tint as the wide button; `＋` the same plus glyph.

## 4. Behavior contract (verify at runtime QA)

- **B1** Idle: type + Enter/Send → message sends, run starts.
- **B2** Running: type + Enter or ＋ → message becomes a ghost bubble at the stream bottom; composer clears.
- **B3** Running: append again → second ghost stacks below the first.
- **B4** Agent crosses a turn boundary with ghosts queued → the whole batch steers into the live run
  (ghosts become real user messages, in order).
- **B5** Run settles naturally (busy→idle) with ghosts queued → batch flushes as a fresh run.
- **B6** Stop with ghosts queued → run aborts, **ghosts remain** (no immediate re-fire).
- **B7** ⚡ Force with a draft and/or ghosts → run interrupts and everything flushes as a new run
  (draft included).
- **B8** ⚡ Force with no active run → behaves as a plain send.
- **B9** Ghost ✕ removes it; tapping a ghost body loads its text back into the composer for editing
  (text only — phone has no attachments).
- **B10** ✎ New chat / cold boot → no stale ghosts (queue reset by `clear_session`).

## 5. Deliberate simplifications (vs the wide composer)

1. **No client-side prompt-injection guard on enqueue.** The phone send path already omits it
   (`composer.rs` docstring: "server remains the prompt-injection authority"). Matching that keeps
   the composer free of new imports and consistent with its own send.
2. **Partial duplication, not a shared hook.** This is the 2nd surface to carry the queue logic
   (rule-of-three not met). A `use_prompt_queue(...)` hook would force a refactor of the just-merged
   ~50 KB wide composer — out of scope and destabilizing. Phone's `flush_queue`/`force_insert`/Effects
   are a *reduced* copy (no attachments mapping, no model override), not verbatim. Accepted trade-off;
   a shared hook is a future refactor if a 3rd surface appears.
3. **No `expand_doctor_command`, no slash/mention/voice** on the phone enqueue path — phone never had
   these on its send path either.

## 6. Testing & verification

- **No new pure logic** is introduced. Every decision reuses already-host-tested helpers
  (`should_flush_on_turn_boundary`, `should_auto_drain_on_settle`) and already-tested `ChatState`
  queue ops (`drain_all_queued_empties_and_preserves_order`, etc. in `state.rs`).
- **Gate:** `just wasm` compiles green; existing `shared-ui-logic` and `aleph-panel` queue tests stay
  green (unchanged). No component-harness test is added for trivial view glue (YAGNI — the view
  selection `if running { enqueue } else { send }` is a one-liner over tested primitives).
- **Runtime QA** (owner): iPhone simulator, full macOS app with a freshly rebuilt `aleph-server`
  binary (panel is embedded at compile time — see `DESKTOP_SHELL.md`). Walk B1–B10. Pay attention to:
  ghosts landing at the stream bottom (not fighting the Todo panel slot), Stop keeping ghosts, and
  ✎/tab-switch leaving no stale ghosts.

## 7. Architectural compliance

- **R4 (Interface = pure I/O):** the composer only marshals input → `ChatApi::send`/`abort` and
  renders state; no business logic, no persistence, no planning.
- **R10 / thin harness:** unaffected (no Core change).
- Zero Core changes; zero new dependencies; zero new RPC.

## 8. Out of scope

- iPad (already inherited via the wide layout).
- Attachments / model override / slash-commands / voice on phone.
- Extracting a shared `use_prompt_queue` hook (future, if a 3rd surface appears).
- push / deploy (owner).
