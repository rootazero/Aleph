# Channel → Panel SessionUpdated Fanout (multi-surface session sync)

**Date**: 2026-06-10
**Goal item**: 「channel 与 webchat 多端信息同步」(/goal openclaw+hermes+opensquilla)

## Gap analysis verdict

The 2-layer permission / working-directory items from the goal are **already shipped and
exceed all three references** (none of openclaw/hermes/opensquilla implement
per-device-tier workspace scoping; Aleph has the channel `default_workspace` lock +
the `agent.run` `project_root` config-tier gate with regression tests). No work there.

The one genuine gap is in multi-surface sync, and it is a classic
**advertised-but-unwired** break:

- `execution_engine` emits `StreamEvent::SessionUpdated` at session creation and run
  completion, with comments self-describing the purpose: *"clients that refresh their
  session list on SessionUpdated (e.g. the Panel sidebar)"*
  (`execute.rs:199-202`, `simple.rs:93-96`).
- But the event is emitted on the **per-run emitter**. For Panel-originated runs that is
  `GatewayEventEmitter` → event bus → sidebar ✅. For **channel-originated runs**
  (Telegram/Slack/Feishu/…) the emitter is `ReplyEmitter`, which **explicitly drops**
  `SessionUpdated` (`reply_emitter/emitter/streaming.rs:590` — "not routed to channels").
- Net effect: when a user talks to the agent from a channel, the Panel never learns the
  session changed. The sidebar doesn't reorder/refresh, and an open transcript view goes
  stale. The only survivor is the auto-topic side path, which publishes a hand-rolled
  `stream.session_updated` **directly to the bus** (`execute.rs:521`) — proving the
  correct route already exists.

Reference comparison: openclaw broadcasts transcript updates to all subscribed
connections; opensquilla fans out all session events (with a replay buffer). Aleph's
break is not a missing subsystem — it is one mis-routed event.

(Deliberately out of scope, consistent with the prior gateway round's reasoned deferral:
a seq/replay buffer for reconnect catch-up. The slow-consumer close + reconnect +
history-pull model covers it; replay is additive with marginal benefit.)

## Design (connect-first, zero new abstraction)

### 1. Publish `SessionUpdated` on the bus, not the emitter (backend)

Mirror the existing `SessionManager::emit_session_updated` pattern
(`session_manager/ops/emit.rs` — Option-gated bus, `publish_frame`, fire-and-forget):

- `ExecutionEngine::publish_session_updated(&self, session_key, origin_channel)` —
  publishes `GatewayEventFrame::SessionUpdated` to `self.event_bus` (already a field).
- Replace the 4 `emitter.emit(StreamEvent::SessionUpdated …)` sites on the full engine
  (`execute.rs` ×2, `fast_path.rs` ×2) with the helper.
- `SimpleExecutionEngine` (fallback engine) gains `event_bus: Option<Arc<GatewayEventBus>>`
  + `with_event_bus()`; its one emit site migrates too. The `agent_init` fallback
  construction already has `event_bus` in scope — wire it.
- `origin_channel` comes from `request.metadata["channel_id"]`: non-empty for channel
  runs (stamped by `inbound_router/executor.rs:238`), empty/absent for Panel runs.
  Empty maps to `None`.

Wire compatibility: `publish_frame` serializes the frame with
`method = "stream.session_updated"` — the exact envelope the Panel already subscribes to
(`subscribe_topic("stream.session_updated")`, client rewrites `stream.` → `run.`).
Panel-run behaviour is unchanged (same frame reached the bus via the emitter before);
channel runs now reach the bus for the first time.

### 2. `origin_channel` on the frame

`GatewayEventFrame::SessionUpdated` gains
`#[serde(default, skip_serializing_if = "Option::is_none")] origin_channel: Option<String>`.
Backward compatible on the wire; lets the Panel distinguish "my own run finished"
(no origin → ignore) from "another surface touched this session" (origin set → refresh).
`SessionManager::emit_session_updated` passes `None` (topic/title updates carry no origin).

### 3. Entropy: delete the dead `StreamEvent::SessionUpdated` variant

After (1), nothing constructs `StreamEvent::SessionUpdated`. Delete:
- the variant (`event_emitter/types.rs:192`),
- the `from_stream_event` arm (`events/frame.rs:366`),
- the ReplyEmitter ignore-list entry (`reply_emitter/emitter/streaming.rs:590`),
- the openai_api `None` arm (`openai_api/completions/agent.rs:183`).
The regression test (`execution_engine/tests.rs:108`) migrates to asserting the bus
receives the frame (typed channel) during `SimpleExecutionEngine::execute`.

### 4. Live refresh of the open transcript (Panel frontend)

`chat_sidebar.rs` already subscribes to `run.session_updated` (list reload). Extend:
- Extract the history→traces→transcript hydration body of `on_select_session` into a
  reusable `hydrate_session_history(dash, chat, key)` (pure refactor, no behavior change
  for the select path; select keeps its tab-switch/clear/workspace-reset preamble).
- In the `run.session_updated` handler: if `data.origin_channel` is non-empty AND
  `data.session_key` equals the currently open session AND the session has no in-flight
  local run (the existing `running` ref-count map) → re-hydrate the open transcript.
- Panel's own runs publish no origin → never self-refresh (no clobber of live streaming
  state, no flicker).

### 5. Ship the Panel: rebuild `interfaces/webchat/dist` (`just wasm`) and commit.

## Error handling

- Bus absent (`None`, tests/degraded): helper is a no-op — identical to today's dropped
  event, never blocks a run.
- `publish_frame` serialization errors ignored (`let _ =`), mirroring emit.rs.
- Frontend re-hydrate failure logs to console and keeps the stale view (pull model
  remains available via re-select).

## Testing

Per the goal's resource directive, no cargo check/test run after implementation.
Compile-safety is covered by the `just wasm` build for the panel crate (required to
produce dist) and by careful read-through for the core crate; the migrated regression
test pins the new bus-publish behaviour for future runs.
