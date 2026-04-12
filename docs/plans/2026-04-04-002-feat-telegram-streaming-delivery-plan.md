---
title: "feat: Telegram edit-based streaming delivery"
type: feat
status: active
date: 2026-04-04
origin: docs/brainstorms/2026-04-04-telegram-streaming-delivery-requirements.md
---

# feat: Telegram Edit-Based Streaming Delivery

## Overview

Enable real-time streaming delivery for the Telegram channel by activating the existing `StreamProtocol::EditBased` infrastructure. LLM responses will appear progressively via `editMessageText` instead of buffering until completion. This is a minimal-activation approach: the streaming state machine and edit path already work; we add the Telegram-specific config, edge-case handling, and UX polish.

## Problem Frame

Users wait seconds to tens of seconds for complete LLM replies before seeing anything. The streaming infrastructure (`StreamingController`, `ReplyEmitter` edit path, `delivery::edit_message`) is fully implemented but Telegram declares `stream_protocol: None` and the global `output_mode` defaults to instant. Activating streaming for Telegram requires a per-channel config override and handling Telegram-specific edge cases (MESSAGE_NOT_MODIFIED, 4096 char limit, typing indicator conflict). (see origin: docs/brainstorms/2026-04-04-telegram-streaming-delivery-requirements.md)

## Requirements Trace

- R1. Set `stream_protocol: StreamProtocol::EditBased` in Telegram capabilities
- R2. `TelegramConfig` gains `streaming: StreamingOptions` with `enabled`, `debounce_ms`, `min_initial_chars`
- R3. Per-channel streaming override in ReplyEmitter construction
- R4. Silently ignore `ApiError::MessageNotModified` in edit_message
- R5. Overflow: split to new message when text exceeds 4096 chars
- R6. Cancel typing indicator after SendInitial succeeds
- R7/R8. Append cursor "▍" during streaming, remove on final edit
- R9. Default debounce 800ms for Telegram (vs 300ms global)
- R10. min_initial_chars = 30

## Scope Boundaries

- No Lane model (reasoning/draft/final separation)
- No streaming voice (TTS)
- No StreamingController core rewrite — only targeted extensions (overflow + message_id replacement)
- No other channel streaming activation
- (see origin for full scope boundaries)

## Context & Research

### Relevant Code and Patterns

- `src/gateway/streaming.rs` — `StreamingController` state machine: `push_chunk()`, `poll_action()`, `finalize()`, `reset()`
- `src/gateway/reply_emitter.rs:1007-1054` — Existing `StreamAction::SendInitial` / `Edit` / `Wait` handling in TextDelta
- `src/gateway/reply_emitter.rs:1192-1227` — `RunComplete` finalization with `EditFinal` / `SendFinal`
- `src/gateway/reply_emitter.rs:278-305` — `ReplyEmitterConfig` struct and `from_output_mode()`
- `src/gateway/interfaces/telegram/delivery.rs:497-570` — `edit_message()` with HTML formatting
- `src/gateway/interfaces/telegram/delivery.rs:43-78` — `classify_error()` with `ApiError` matching
- `src/gateway/interfaces/telegram/config.rs` — `TelegramConfig` with `serde(default)` pattern
- `src/gateway/interfaces/telegram/mod.rs:113-129` — `ChannelCapabilities` declaration
- `src/gateway/inbound_router/executor.rs:86-97` — ReplyEmitter construction with global output_mode
- `src/gateway/session_scheduler.rs:165-177` — Same construction path in session scheduler
- `src/gateway/interfaces/msteams/mod.rs:107` — Reference: MS Teams uses `StreamProtocol::Native`
- `src/gateway/channel.rs:356-370` — `StreamProtocol` enum and `ChannelCapabilities`

### Key teloxide Findings

- teloxide v0.13 — `ApiError::MessageNotModified` is a specific enum variant (not string matching)
- `ApiError::MessageIsTooLong` and `ApiError::EditedMessageIsTooLong` also exist as typed variants
- Current `classify_error()` would misclassify `MessageNotModified` as `Rejected` via the `"Bad Request"` string fallback

## Key Technical Decisions

- **Per-channel override via ChannelRegistry query**: executor.rs already has `self.channel_registry` and `ctx.reply_route.channel_id`. After building `ReplyEmitterConfig` from global config, query the channel's `stream_protocol`. If `EditBased`, override `stream_enabled`, `debounce_ms`, `min_initial_chars` from `TelegramConfig.streaming`. No new dependency injection path needed — use existing infrastructure. (see origin: R3 decision)
- **Overflow handled in ReplyEmitter, not StreamingController**: The ReplyEmitter already knows the channel's `max_message_length` via capabilities. When `StreamAction::Edit(text)` exceeds the limit, ReplyEmitter: (1) edits current message with text up to threshold (clean, no cursor), (2) calls `streaming.reset()` to clear state, (3) pushes overflow text back via `streaming.push_chunk(remaining_text)`, (4) sends new message with overflow text + cursor, (5) calls `streaming.record_sent(new_msg_id)`. This avoids modifying `poll_action()` core logic. (see origin: R5)
- **MESSAGE_NOT_MODIFIED as typed match**: Use `ApiError::MessageNotModified` directly instead of string matching. Also catch `ApiError::MessageCantBeEdited` (warn log, message deleted by user) and `ApiError::EditedMessageIsTooLong` (trigger overflow path as fallback) as non-fatal cases during streaming. (R4)
- **Cursor appended in ReplyEmitter after sanitize, before channel_registry call**: The "▍" character is plain Unicode and survives HTML conversion in `MessageFormatter::format()`. Append in the ReplyEmitter's streaming action handlers, not in StreamingController or delivery layer. (R7/R8)
- **Debounce 800ms default for Telegram**: Telegram's edit rate limit is stricter than send. 800ms is validated by OpenClaw production use. Configurable via `TelegramConfig.streaming.debounce_ms`. (R9)

## Open Questions

### Resolved During Planning

- **teloxide MessageNotModified variant**: `ApiError::MessageNotModified` — exact typed variant exists in teloxide v0.13. Match directly in `edit_message()`.
- **ReplyEmitter injection path**: Use existing `self.channel_registry.get()` in executor.rs to query `stream_protocol` from channel capabilities. No architectural change needed.
- **Overflow state machine**: Use `StreamingController::reset()` + `push_chunk(remaining)` + `record_sent(new_id)`. No new methods or `StreamAction::Overflow` variant needed — existing API suffices.
- **Cursor HTML safety**: "▍" (U+258D) is a standard Unicode block element. `MessageFormatter::format()` preserves it without escaping.

### Deferred to Implementation

- Exact HTML char count threshold for overflow (4096 is the Telegram API limit, but HTML tags add overhead — implementation should measure post-conversion length; start with 3800 as safe margin and tune)
- Whether `MessageCantBeEdited` requires different handling than `MessageNotModified` in streaming context (both should be non-fatal, but may need different logging levels)

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```
Streaming Token Flow:

  LLM chunk arrives
       │
       ▼
  ReplyEmitter::emit(TextDelta)
       │
       ├─ buffer.push(chunk)
       ├─ streaming.push_chunk(chunk)
       │
       ▼
  streaming.poll_action()
       │
       ├─ Wait → do nothing
       │
       ├─ SendInitial(text) →
       │     text + "▍"
       │     → channel_registry.send()
       │     → record_sent(msg_id)
       │     → typing_cancel.cancel()    ← R6
       │
       ├─ Edit(text) →
       │     if text.len() > OVERFLOW_THRESHOLD:   ← R5
       │       edit current msg (clean, no cursor)
       │       send new msg (overflow text + "▍")
       │       streaming.replace_message_id(new_id)
       │     else:
       │       text + "▍"
       │       → channel_registry.edit()
       │       → record_edit()
       │
       └─ (on RunComplete) finalize() →
             EditFinal(text) → edit with clean text (no "▍")
             SendFinal(text) → send complete message
```

## Implementation Units

- [ ] **Unit 1: StreamingOptions in TelegramConfig**

  **Goal:** Add per-channel streaming configuration to Telegram.

  **Requirements:** R1, R2

  **Dependencies:** None

  **Files:**
  - Modify: `src/gateway/interfaces/telegram/config.rs`
  - Modify: `src/gateway/interfaces/telegram/mod.rs`
  - Test: `src/gateway/interfaces/telegram/config.rs` (inline `#[cfg(test)]`)

  **Approach:**
  - Add `StreamingOptions` struct: `enabled: bool` (default true), `debounce_ms: u64` (default 800), `min_initial_chars: usize` (default 30)
  - Add `#[serde(default)]` field `streaming: StreamingOptions` to `TelegramConfig`
  - In `TelegramChannel::capabilities()`, change `stream_protocol: Default::default()` to `StreamProtocol::EditBased`

  **Patterns to follow:**
  - `CoalescingConfig` in config.rs — same `#[serde(default)]` pattern with `Default` impl
  - `TelegramConfig` existing field conventions

  **Test scenarios:**
  - Happy path: Deserialize config without `streaming` field → defaults applied (enabled=true, debounce=800, min_chars=30)
  - Happy path: Deserialize config with explicit `streaming` values → values respected
  - Happy path: `TelegramChannel::capabilities().stream_protocol == StreamProtocol::EditBased`

  **Verification:** `cargo test -p alephcore --lib` passes. Config roundtrips through serde correctly.

- [ ] **Unit 2: ReplyEmitterConfig debounce fields + per-channel override**

  **Goal:** Allow per-channel streaming parameters and wire the override in executor/session_scheduler.

  **Requirements:** R3, R9, R10

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `src/gateway/reply_emitter.rs` (ReplyEmitterConfig struct + with_config)
  - Modify: `src/gateway/inbound_router/executor.rs`
  - Modify: `src/gateway/session_scheduler.rs`
  - Modify: `src/gateway/channel.rs` (add accessor method on ChannelCapabilities or ChannelRegistry)
  - Test: `src/gateway/reply_emitter.rs` (inline `#[cfg(test)]`)

  **Approach:**
  - Add `debounce_ms: u64` (default 300) and `min_initial_chars: usize` (default 30) to `ReplyEmitterConfig`
  - In `ReplyEmitter::with_config()`, use these fields for `StreamingConfig` instead of hardcoded 300/30
  - In executor.rs (~line 86-97): after building `reply_config` from global output_mode, query `self.channel_registry.get(&ctx.reply_route.channel_id)` → read `capabilities().stream_protocol`. If `EditBased`, set `reply_config.stream_enabled = true`, `reply_config.debounce_ms` and `reply_config.min_initial_chars` from the channel's streaming config. The channel handle is already available (see feishu detection at line 105-112 for the pattern)
  - Apply same logic in session_scheduler.rs at BOTH construction sites: `process_queue` (~line 165-177) and free function `run_enriched_message` (~line 388-399)

  **Patterns to follow:**
  - executor.rs:105-112 — existing pattern for querying channel type from registry
  - `ReplyEmitterConfig::from_output_mode()` — extend, don't replace

  **Test scenarios:**
  - Happy path: ReplyEmitterConfig with debounce_ms=800 → StreamingController uses 800ms interval
  - Happy path: ReplyEmitterConfig with stream_enabled=true → StreamingController enabled
  - Edge case: Channel not found in registry → fall back to global output_mode (no panic)
  - Edge case: Channel has stream_protocol=None → no override, use global config

  **Verification:** `cargo test -p alephcore --lib` passes. ReplyEmitter constructed with correct debounce when channel is EditBased.

- [ ] **Unit 3: MESSAGE_NOT_MODIFIED handling in edit_message**

  **Goal:** Silently ignore "message not modified" errors during streaming edits.

  **Requirements:** R4

  **Dependencies:** None (can be done in parallel with Units 1-2)

  **Files:**
  - Modify: `src/gateway/interfaces/telegram/delivery.rs`
  - Test: `src/gateway/interfaces/telegram/delivery.rs` (inline `#[cfg(test)]`)

  **Approach:**
  - In `edit_message()`, wrap the `request.await` result. Before propagating errors, match on `RequestError::Api(ApiError::MessageNotModified)` → return `Ok(())` with `tracing::debug!` log
  - Also handle `ApiError::MessageCantBeEdited` the same way (message may have been deleted mid-stream)
  - Do NOT change `classify_error()` — this is specific to edit operations, not general send

  **Patterns to follow:**
  - `send_reaction()` in delivery.rs — already uses "swallow non-critical errors" pattern

  **Test scenarios:**
  - Happy path: Successful edit → Ok(())
  - Edge case: MessageNotModified error → Ok(()) with debug log (not error)
  - Edge case: MessageCantBeEdited error → Ok(()) with warn log
  - Error path: Other ApiError (e.g. ChatNotFound) → still propagated as ChannelError

  **Verification:** `cargo test -p alephcore --lib` passes. No `MESSAGE_NOT_MODIFIED` errors in logs during streaming.

- [ ] **Unit 4: Cursor symbol and typing cancellation in ReplyEmitter**

  **Goal:** Add streaming cursor "▍" to intermediate edits and cancel typing after first message.

  **Requirements:** R6, R7, R8

  **Dependencies:** Unit 2 (stream_enabled must be wired)

  **Files:**
  - Modify: `src/gateway/reply_emitter.rs`
  - Test: `src/gateway/reply_emitter.rs` (inline `#[cfg(test)]`)

  **Approach:**
  - Define `const STREAMING_CURSOR: &str = "▍";` in reply_emitter.rs
  - In `StreamAction::SendInitial` handler (~line 1014): after `sanitize_llm_output`, append `STREAMING_CURSOR` to text before creating `OutboundMessage`. After `record_sent()`, add `self.typing_cancel.cancel()`.
  - In `StreamAction::Edit` handler (~line 1033): after `sanitize_llm_output`, append `STREAMING_CURSOR` before `channel_registry.edit()`
  - In `StreamAction::EditFinal` handler (~line 1204): do NOT append cursor — send clean text
  - In `StreamAction::SendFinal` handler (~line 1196): do NOT append cursor — send clean text

  **Patterns to follow:**
  - Existing `sanitize_llm_output()` usage in the same handlers
  - `self.typing_cancel.cancel()` is already used in RunComplete handler

  **Test scenarios:**
  - Happy path: SendInitial text has "▍" appended
  - Happy path: Edit text has "▍" appended
  - Happy path: EditFinal text does NOT have "▍"
  - Happy path: SendFinal text does NOT have "▍"
  - Happy path: typing_cancel is cancelled after SendInitial succeeds
  - Edge case: SendInitial fails → typing NOT cancelled (keep typing active for retry)

  **Verification:** `cargo test -p alephcore --lib` passes. Streaming messages show cursor during generation, clean text on completion.

- [ ] **Unit 5: Overflow handling for messages exceeding 4096 chars**

  **Goal:** Split long streaming messages into multiple Telegram messages without losing content.

  **Requirements:** R5

  **Dependencies:** Units 2, 3, 4

  **Files:**
  - Modify: `src/gateway/streaming.rs` (no new methods — uses existing `reset()`, `push_chunk()`, `record_sent()`)
  - Modify: `src/gateway/reply_emitter.rs` (overflow detection in Edit handler)
  - Test: `src/gateway/streaming.rs` (inline `#[cfg(test)]`)
  - Test: `src/gateway/reply_emitter.rs` (inline `#[cfg(test)]`)

  **Approach:**
  - In ReplyEmitter's `StreamAction::Edit` handler, before sending the edit, check if `text.chars().count() > self.overflow_threshold` (a new field on ReplyEmitter, initialized as `channel.capabilities().max_message_length.saturating_sub(300)` during construction alongside the per-channel streaming override in Unit 2). Use `chars().count()` not `len()` — Telegram's limit is characters, not bytes (consistent with `streaming.rs:113`). If overflow:
    1. Edit current message with the text up to the threshold (clean, no cursor) — this finalizes the current message
    2. Call `streaming.reset()` to clear buffer and message_id
    3. Push overflow text back: `streaming.push_chunk(remaining_text)` — so the controller tracks the new message's content
    4. Send a new message with the remaining text + cursor
    5. Call `streaming.record_sent(new_msg_id)` to track the new message
  - Same overflow check in `StreamAction::EditFinal` — if the final text overflows, split into edit (finalize current) + send (new message, clean, no cursor). No state reset needed since streaming is ending.

  **Patterns to follow:**
  - `StreamingController::reset()` — already exists for clearing state
  - `chunking::split_html_safe()` in delivery.rs — for reference on where to split

  **Test scenarios:**
  - Happy path: Text under 4096 → normal edit, no overflow
  - Happy path: Text exceeds threshold → current message finalized, new message sent with remainder
  - Edge case: Overflow happens on EditFinal → split into edit + send, both without cursor
  - Edge case: Text barely exceeds threshold (4097 chars) → still triggers overflow correctly
  - Edge case: Multiple overflows in one stream (>8192 chars) → each overflow creates a new message
  - Integration: Overflow preserves cursor "▍" on continuation message during streaming, removes on final

  **Verification:** `cargo test -p alephcore --lib` passes. Long messages split cleanly without content loss.

## System-Wide Impact

- **Interaction graph:** ReplyEmitter's TextDelta handler now calls `channel_registry.edit()` and `typing_cancel.cancel()` during streaming — previously only called on RunComplete. No new callbacks or middleware.
- **Error propagation:** MessageNotModified errors are swallowed in `edit_message()` only — all other error paths unchanged. Cooldown system not affected (edit_message doesn't use ErrorCooldown; streaming edit failures are non-critical UX, not delivery failures).
- **State lifecycle:** StreamingController gains `replace_message_id()` but buffer lifecycle unchanged. The controller's `reset()` method already handles state clearing for overflow scenarios.
- **API surface parity:** Other channels with `EditBased` support (none currently, but Discord could declare it in future) would automatically benefit from the per-channel override in executor.rs.
- **Unchanged invariants:** `Channel::send()`, `Channel::edit()` trait signatures unchanged. `StreamingController::poll_action()` and `finalize()` logic unchanged. Global `output_mode` still works for channels that don't declare a stream_protocol.

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Telegram edit rate limiting (429) at 800ms debounce | 800ms is OpenClaw-validated; configurable via `debounce_ms` for tuning |
| HTML overhead causes overflow threshold miscalculation | Start with 3800 char margin (4096 - ~300 for tags); tune based on testing |
| Cursor "▍" rendering differs across Telegram clients | Standard Unicode block element; fallback: make cursor configurable in StreamingOptions |
| Concurrent streaming + voice mode conflict | Existing guard: `!self.should_voice().await` check in TextDelta handler already prevents streaming when voice is active |

## Sources & References

- **Origin document:** [docs/brainstorms/2026-04-04-telegram-streaming-delivery-requirements.md](docs/brainstorms/2026-04-04-telegram-streaming-delivery-requirements.md)
- Related code: `src/gateway/streaming.rs`, `src/gateway/reply_emitter.rs`, `src/gateway/interfaces/telegram/delivery.rs`
- teloxide v0.13 `ApiError` enum — `MessageNotModified`, `MessageCantBeEdited`, `EditedMessageIsTooLong`
- OpenClaw reference: `extensions/telegram/polling-session.ts` — 800ms debounce validation
