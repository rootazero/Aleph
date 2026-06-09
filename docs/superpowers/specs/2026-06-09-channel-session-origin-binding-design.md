# Channel ↔ WebChat Session Origin Binding (多端会话来源连线)

**Date:** 2026-06-09
**Scope:** `①` multi-end info sync — sub-gap **(a) session continuity**
**Status:** design approved (scoped via AskUserQuestion: (a) 会话连续性优先 + ④ 仅文档化扩展点)

## Background / Goal

`/goal` asked for four things on Aleph's channel architecture. Gap-analysis against
`openclaw` + `hermes-agent` plus first-party source reading established:

| Goal point | Status |
|---|---|
| ② channel 2-layer permission (Chat/Config + workdir) | ✅ already merged (`f97b2b94f`) |
| ③ panel remote = channel identity, reuse permission | ✅ already unified (verified 7-hop convergence) |
| ① channel ↔ agent ↔ webchat multi-end info sync | 🔴 **real gap — this spec** |
| ④ reserve space for multi-level permission | ⚠️ doc-only extension note |

Reference pattern (hermes/openclaw): *the session key is the contract; every surface
binds to the same logical conversation*. Aleph already keys sessions in a shared
`SessionStore`, and `sessions.list` already enumerates **all** sessions (no channel
filter), the Panel sidebar already calls it, and `chat.send` already honours an explicit
`session_key`. So **Panel can already list + open + continue a Telegram session**
mechanically.

## The real gap: session origin binding is a half-wire

A session's **origin channel** has a persisted slot — `SessionIdentityMeta.source_channel`
(`session_manager/mod.rs:119`, carried by `SessionMetadata.identity_meta`) — but **both ends
are broken**:

1. **G1 (population bug)** — `ensure_session(&key)` (`execute.rs:127`) takes no channel and
   `get_or_create` writes no identity metadata, so the persisted `source_channel` stays the
   default sentinel `"unknown"` forever. The origin is *never actually recorded*.
2. **G2 (not surfaced)** — `sessions.list`'s `SessionInfo` builder (`db_handlers/query.rs`)
   reads `identity_meta` for topic/status but **omits `source_channel`**, so the Panel can
   never identify which conversations came from which channel.
3. **G3 (dead None)** — `SessionChangedEvent.channel` is hardcoded `None`
   (`file_backend/mod.rs:94`), so live session-list refresh also drops origin.

Net effect: the Panel cannot *distinguish* channel-originated conversations from its own,
even though the data model already reserves a place for it. This is "已造未连" — built but
not wired.

## Design — connect the existing slot (no new abstraction)

Reuse `SessionIdentityMeta.source_channel`; do not introduce a new binding type.

### Single projection source

Add `SessionMetadata::origin_channel() -> Option<String>` that returns `None` for the
synthetic `""`/`"unknown"` sentinel and `Some(channel)` otherwise. Both G2 and G3 call it,
so the "what counts as a real origin" rule lives in exactly one place.

### G1 — record the origin on first message (the bug fix)

- New idempotent inherent method `SessionManager::stamp_source_channel(&key, channel)`
  (mirrors `set_topic`): read metadata JSON → `SessionIdentityMeta::from_json_str` → if the
  current `source_channel` is empty/`"unknown"`, set it to `channel` and `UPDATE`; otherwise
  no-op (never clobber a real origin).
- New trait method `SessionStore::set_source_channel` with a **default no-op** body (so the
  file backend and any other impl compile unchanged), overridden on the SQLite backend
  (`SqliteSessionStore = SessionManager`) to delegate to `stamp_source_channel`.
- `AgentInstance::set_session_source_channel(&key, channel)` wrapper (mirrors
  `ensure_session`) so the engine does not reach into the private store field.
- `execute.rs`: when `is_first_message`, stamp `request.metadata["channel_id"]` (skip empty).
  `channel_id` is `"gui:chat"` for Panel `chat.send` (`agent.rs:202`) and the real channel id
  for inbound channels (`executor.rs:213`).

### G2 — surface it through `sessions.list`

`SessionInfo` gains `channel: Option<String>`, populated with `m.origin_channel()`. Real
consumer: the Panel sidebar already deserialises `SessionInfo` and calls `sessions.list`.

### G3 — populate the live event

`file_backend::emit_session_changed` sets `channel: meta.and_then(|m| m.origin_channel())`
instead of the hardcoded `None`, removing the dead value.

## Verification (backend integration / unit tests, no WASM)

- `SessionMetadata::origin_channel()` — `"unknown"`/empty → `None`; real → `Some`.
- `stamp_source_channel` — fresh session stamps `"telegram"`; second call with `"gui:chat"`
  does **not** clobber (idempotent); reflected via `list_sessions`/`get_metadata`.
- A Telegram-originated session reports `channel:"telegram"`; a Panel session
  `channel:"gui:chat"`; a legacy session with no metadata → `None` (backward compatible —
  default `SessionIdentityMeta` still parses).

## Entropy reduction

- `SessionChangedEvent.channel`'s hardcoded `None` dead value is eliminated.
- `SessionIdentityMeta.source_channel` goes from a perpetually-`"unknown"` dead field to a
  live, surfaced one.

## Phase 2 — cross-surface reply fan-out (sub-gap (b), now implemented)

Built on the origin binding above so a continuation no longer silently forks:

- **Persist the full origin route.** `set_source_channel` also captures the origin
  conversation id (e.g. the Telegram chat id) from inbound `metadata["conversation_id"]`
  into `identity_meta.custom["origin_conversation"]`, read back via
  `SessionMetadata::origin_conversation()`. Idempotent with the channel stamp.
- **`OriginFanoutEmitter`** (`event_emitter/origin_fanout.rs`) — a decorator mirroring the
  proven `InstantBufferingEmitter` pattern. It wraps the run's primary
  `GatewayEventEmitter` and, on `RunComplete`, delivers the final response as a single
  message to the bound origin channel via `ChannelRegistry::send`. All other events pass
  through untouched (no double-streaming of tool/reasoning chrome; inner sequencing intact).
- **Registry injection without constructor churn.** A process-global
  `OnceLock<Arc<ChannelRegistry>>` (mirrors `middleware::request_state`) is set once at
  subsystem boot, so the Panel run path reaches the registry without threading it through
  `AgentRunManager`. `None` in non-gateway contexts → fan-out simply skipped.
- **Wiring + double-delivery safety.** `AgentRunManager::start_run` wraps the emitter only
  when `AgentInstance::origin_route` resolves an external origin **and** the run's surface
  (`metadata["channel_id"]`, `"gui:chat"` for the Panel) differs from it. Inbound channel
  runs never reach this handler (they deliver via `ReplyEmitter`), so a channel's own reply
  is never duplicated; same-surface continuations are skipped by the surface check.

Verification: `origin_conversation` capture is idempotent and survives a same-session
re-stamp; the decorator forwards `RunComplete` to the inner emitter even when channel
delivery is unavailable (best-effort, never blocks the Panel stream).

## Explicitly deferred (extension points)

- **Multi-level permission (④):** `source_channel` + `ChannelPermissionLevel` can grow a
  level-ordered model later. Documented only; no speculative abstraction now (R10 YAGNI —
  adding an ordinal rank with no third-level consumer would itself be dead code).
- **Frontend origin badge:** G2/G3 deliver origin to the wire protocol; rendering a badge in
  the Panel sidebar is a later WASM round (consistent with prior deferral pattern).
- **Live cross-surface streaming:** the Panel watching a *channel-initiated* run live (full
  event mirror, not just the final reply) remains future work; this round mirrors the final
  reply only, which is the high-value, low-risk slice.
