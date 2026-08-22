# btw probe result — TUI transcript frame scoping

Date: 2026-08-20
Method: static trace of the transcript-frame delivery path from server emission
to the TUI's applied state, followed end to end across the gateway visibility
filters, the shared WS client, and the TUI event handler. The real-machine leg
(brief Step 2, driving an interactive TUI against a live server) was not run —
driving a raw-mode ratatui TUI requires an interactive PTY that was not
available in this environment. No live observation was made; nothing below is
inferred from expectation, only from code actually read.

**tui_frames_are_session_scoped: false (static read only)**

**a_client_receives_frames_for_other_sessions_of_the_same_user: true (static read only)**

Server-side predicate read at: `src/gateway/event_visibility.rs:712` (`EventVisibilityIndex::event_admits_for`)

Observed: No live observation — the machine leg did not run. The conclusion above
is a static read, not something seen on screen.

## Static evidence

The delivery path for a transcript frame (`stream.response_chunk`,
`stream.agent_trace`) runs through four independent gates, traced end to end.
None of them compare the frame's session against "the session_key this specific
TUI connection is currently attached to / rendering." The gate that does exist
is owner/user-scoped, not session-key-scoped.

1. **`src/gateway/handlers/events.rs:91-96`** — `SubscriptionManager::should_receive`:
   returns `true` unconditionally when the connection has registered no topic
   filter (`None => true // No filter means receive all`). The TUI never calls
   `events.subscribe`, so this term is always true for a TUI connection.
   Confirmed by `interfaces/tui/src/tui/app/mod.rs:658-663`, a doc comment on
   `turn_streamed_len` that states plainly: "The TUI subscribes to no topics, so
   the gateway's `should_receive` gives it both" (referring to `ResponseChunk`
   and `AgentTrace` carrying the same text twice).

2. **`src/gateway/event_visibility.rs:342-358`** — `session_identity_of` classifies
   `stream.response_chunk`, `stream.agent_trace`, `stream.tool_start/update/end`,
   `stream.reasoning`, `stream.run_complete`, `stream.run_error` etc. as
   `SessionIdentity::ByRunId(run_id)` (the construction itself is at lines
   355-357, inside this match arm) — these frames carry only a `run_id`, not a
   `session_key`.

3. **`src/gateway/event_visibility.rs:748-755`** — the `ByRunId` arm of
   `event_admits_for` resolves the run's `session_key` via the run→session cache,
   then calls `self.session_admits(&session_key, caller, store)`. This is the
   entire gate for a `ByRunId` frame: there is no comparison anywhere against a
   "session this connection is attached to" — there is no such concept in
   `ConnectionState` at all.

4. **`src/gateway/event_visibility.rs:864` (`session_admits`) and `src/gateway/event_visibility.rs:536-555`
   (`SessionOwnership` / `visible_to`)** — the actual check is
   `owner_and_scope_visible_to(owner_user_id, scope_id, caller)`: does the
   connecting USER own (or, for a shared room, have roster access to) the
   session that owns this run. It is an owner/roster predicate keyed on the
   caller's user id, not an equality check against any particular session_key.
   The module's own doc (`src/gateway/event_visibility.rs:1-10`) states this
   directly: `EventScopeGuard` (the role-based filter) is "default-**allow**...
   including every ordinary session/chat/agent-run event," and this module (the
   4th filter term) closes the gap only at the OWNER level — "so today every
   connected member receives every OTHER user's live run stream" is exactly the
   defect this module fixes, and it fixes it by user, not by exact session.
   The same doc also notes loopback resolves to `Some(OWNER_USER_ID)`, so on a
   single-user box (the default/dev deployment, matching the brief's probe
   setup) every session on the box shares one owner and this gate is a no-op —
   every run's frames pass it.

   Consequence for a background/delegated subagent: `src/agents/subagent_spawner/mod.rs:1200-1209`
   (`fn ephemeral_for(agent_id, request_id) -> SessionKey`, called from the
   actual spawn path at `:488`) builds `SessionKey::Ephemeral { agent_id,
   ephemeral_id }`, with `ephemeral_id` prefixed `SUBAGENT_BG_CHILD_PREFIX =
   "sub-bg-"` (`:1216`) for a background child or `ANON_CHILD_PREFIX = "sub-"`
   (`:1220`) otherwise — a session key distinct from the parent conversation's.
   `src/agents/subagent_tool/recovery.rs:80-125` shows the matching reader side
   (`addresses`/`classify`), confirming the same `Ephemeral` shape and prefix
   convention. Nothing in the traced path denies a child's frames to a TUI
   attached to the parent session, provided the child session's stamped
   `owner_user_id` matches the connection's `caller_user` (true by default for
   any subagent spawned within one user's own run).

5. **Client-side, `shared/client/src/connection.rs:221-232`** (`handle_message`) —
   every JSON-RPC notification that parses as a `StreamEvent` is forwarded
   unconditionally into the `event_tx` channel: `serde_json::from_value::<StreamEvent>(params) → event_tx.send(event)`.
   No session_key or run_id filter exists at this layer either; this is the
   shared client used by both the TUI and the CLI.

6. **TUI-side, `interfaces/tui/src/tui/app/events.rs:85-389`** (the full body of
   `handle_gateway_event`) — only `RunAccepted` (line 87-88) *sets*
   `self.current_run`; no other arm in the function checks the incoming frame's
   `run_id` (or any session_key) against `self.current_run` before applying it
   to the transcript. Confirmed for every arm that carries a `run_id`
   server-side per gate #2 above: `AgentTrace` (97-99), `ResponseChunk`
   (186-192), `ToolStart`/`ToolUpdate`/`ToolEnd`, `Reasoning`,
   `RunComplete`/`RunError`, `ReasoningBlock` (309-316), `UncertaintySignal`
   (318-330), `RunRetrying` (332-346), `ModelResolved` (348-370), and
   `ContextGauge` (372-387) — all apply unconditionally. The ONE exception in
   the whole file is `StreamEvent::ClarificationEnded` (`events.rs:288-298`),
   which explicitly compares `session_key == d.session_key` (the comparison
   itself is at line 295) before acting — the brief's cited comment ("Only the
   card for THIS session is retired... a frame for another session must not
   close the one the user is looking at") confirms this is a deliberate,
   narrow exception, not the general rule. Even the sibling `StreamEvent::AskUser`
   handler (`events.rs:259-272`) shows a dialog for whatever session_key the
   frame carries, with no check that it matches the session currently open.

## Why both booleans, per Ruling R1

These two statements read as near-inverses of the same fact but are recorded
separately because a later task (the `/btw` TUI overlay) depends on the second
one being TRUE — the TUI receiving frames for a session it is not attached to
is a *precondition* for that overlay's design, not just an incidental leak.
Both conclusions rest on the same evidence above: `tui_frames_are_session_scoped`
is false because no layer in the traced path drops a frame for failing to match
"the session this connection is attached to" (only an owner/user check exists,
and only at gate #4); `a_client_receives_frames_for_other_sessions_of_the_same_user`
is true for the same reason stated from the client's side — any run whose
session is owned by (or roster-visible to) the connection's authenticated user
reaches this connection and is applied to the open transcript unconditionally.

## What this does NOT establish

- No live observation was made. It is possible some upstream layer not covered
  by this trace (e.g. a per-connection subscription default set elsewhere, or a
  code path added after this read) changes the picture; the trace above is
  believed complete for the topics the brief names (`stream.response_chunk`,
  `stream.agent_trace`) but was not verified by watching bytes on a wire.
- Whether a background subagent's `owner_user_id` in practice always equals the
  parent session's owner was read from the general session-attribution
  mechanism, not traced through the specific subagent-spawn code path that
  stamps `SessionMetadata` for an `Ephemeral` child session.
