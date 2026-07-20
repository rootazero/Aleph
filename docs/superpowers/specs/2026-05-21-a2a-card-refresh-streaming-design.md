# A2A Card Refresh + Streaming Outbound Delegation — Design

- **Date:** 2026-05-21
- **Status:** Approved (design)
- **Scope:** Two deferred follow-ups from the A2A Outbound Wiring cycle (`f334bd176`)

## Background

The A2A Outbound Wiring cycle resurrected Aleph's dead A2A client stack into two
live builtin tools (`a2a_delegate`, `a2a_agents`). It deferred two items:

1. **Placeholder Card refresh** — `CardRegistry::load_from_config` creates
   placeholder `AgentCard`s (`version: "unknown"`, empty `skills`/`description`)
   for config-declared agents. They are routable by name but the LLM semantic
   matcher and `a2a_agents list` never see real skill data. No task ever
   upgrades a placeholder to a real card.

2. **SSE streaming client** — `src/a2a/adapter/client/sse_stream.rs`
   (`parse_sse_response`) has **zero callers** and is **broken**: `parse_event`
   does `serde_json::from_str::<TaskStatusUpdateEvent>(data)`, but Aleph's own
   server (`a2a/adapter/server/routes.rs`) sends each SSE `data` line as a
   JSON-RPC envelope `{"jsonrpc","id","result":<event>}`. It cannot parse
   Aleph's own stream. `A2AClient` has no `send_message_stream`, so
   `A2ASubAgent::dispatch` uses the flat-120s-timeout sync `send_message`.

## Goals

- Config-declared A2A agents get their real Agent Card (skills, description,
  version) shortly after startup — improving routing quality.
- Outbound delegation consumes the remote agent's SSE stream: live progress
  notifications, idle-timeout liveness detection, early failure detection.
- `sse_stream.rs` becomes correct and reachable — no broken dead code left.

## Non-Goals

- Periodic / scheduled card re-refresh (one-shot at startup only).
- A2A-spec `message/stream`-on-`/a2a` interop (Aleph uses `message/send` on
  `/a2a/stream`; sync fallback covers non-Aleph agents).
- Un-stubbing `CardRegistry::fetch_card` / `resolve_by_intent` (no caller —
  refresh builds `A2AClient` directly).
- Destructive refactoring. Changes are additive and surgical.

## Item 1 — Startup Card Refresh

### New file: `src/a2a/service/card_refresh.rs` (~120 lines)

```text
pub async fn refresh_all_cards(registry: &CardRegistry) -> usize
```
- `registry.list_agents()` → for each agent build a fresh `A2AClient`
  (`with_auth` when `auth_token` is `Some`, else `new`).
- `client.fetch_agent_card()`:
  - `Ok(card)` → `registry.upsert(RegisteredAgent { card, trust_level,
    base_url, auth_token <all preserved>, health: Healthy, last_seen: now })`;
    increment count.
  - `Err(e)` → `tracing::warn!`; leave the placeholder untouched (still
    name-routable).
- Returns the number of cards successfully refreshed.

```text
pub fn spawn_card_refresh(registry: Arc<CardRegistry>, sub_agent: Arc<A2ASubAgent>)
```
- `tokio::spawn`s one `refresh_all_cards` pass; when count > 0 calls
  `sub_agent.refresh_agent_names()` so the sync `can_handle` name cache picks
  up newly-discovered skill names and aliases. Logs the final count.

Design notes:
- Builds `A2AClient` directly rather than via `A2AClientPool` — avoids polluting
  the hot-path pool with clients keyed by soon-to-be-replaced placeholder ids.
- `CardRegistry::upsert` already dedups by `base_url` OR `card.id`, so the
  placeholder entry collapses cleanly into the real one even when the slug id
  differs from the remote card's id.
- Sequential iteration — config agents are few; no need for `join_all`.

### Wiring: `src/bin/aleph-server/commands/start/mod.rs`

After the A2A tool handle is published (step ~9.5, inside the
`if a2a_config.enabled` block), add:

```text
alephcore::a2a::service::spawn_card_refresh(card_registry.clone(),
                                            a2a_sub_agent.clone());
```

Non-blocking — never delays startup. No-op when the registry is empty.

### Export: `src/a2a/service/mod.rs`

Add `pub mod card_refresh;` and re-export `refresh_all_cards`,
`spawn_card_refresh`.

## Item 2 — Streaming Outbound Delegation

### `src/a2a/adapter/client/http_client.rs`

- New field `stream_idle: Duration` (default 90s) + builder `with_stream_idle()`,
  mirroring the existing `with_timeout()`.
- New method:
  ```text
  pub async fn send_message_stream(&self, task_id, message, session_id)
      -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>>
  ```
  - POSTs JSON-RPC `message/send` (params `{taskId, message, sessionId?}`) to
    `{base_url}/a2a/stream` with header `Accept: text/event-stream` and bearer
    auth when a token is set.
  - **No total `.timeout()`** — for a streaming body that would cap the whole
    stream lifetime. Liveness is governed by the idle-timeout instead.
  - Non-2xx status → `Err` (caller falls back to sync). 2xx →
    `parse_sse_response(response, self.stream_idle)`.

### `src/a2a/adapter/client/sse_stream.rs`

1. **Envelope bug fix.** Replace `parse_event` data handling: parse `data` as
   `serde_json::Value`; if `.error` is present yield `Err(A2AError)`; else take
   `.result` (falling back to the whole value for bare-event spec interop);
   try `kind`-tagged `UpdateEvent`, else disambiguate via the SSE `event:` line
   (`status-update` → `TaskStatusUpdateEvent`, `artifact-update` →
   `TaskArtifactUpdateEvent`). The classifier returns
   `Option<A2AResult<UpdateEvent>>` — `Some(Ok)` event, `Some(Err)` error frame,
   `None` skip (keep-alive / unknown). Existing 7 unit tests still pass
   (bare-event back-compat path).

2. **Idle timeout.** `parse_sse_response` gains an `idle: Duration` param. Each
   byte-chunk read is wrapped in `tokio::time::timeout(idle, …)`. Any bytes —
   including SSE keep-alive comment lines — reset the timer; `idle` of silence
   yields `A2AError::Timeout(idle)` and ends the stream.

3. **Stream fold.**
   ```text
   pub struct FoldedOutcome { summary: String, success: bool,
                              error: Option<String>, final_state: Option<TaskState> }
   pub async fn fold_stream(stream, on_chunk: impl FnMut(&str)) -> FoldedOutcome
   ```
   Folds `UpdateEvent`s: accumulates artifact text parts + the last status
   message; `on_chunk` is fired with each new text fragment for live progress.
   `final_state == Failed` or a stream `Err` → `success = false`. Pure — no
   `builtin_tools` dependency (caller supplies the notify callback).

### `src/a2a/sub_agent.rs`

`dispatch` becomes streaming-first:
- Try `client.send_message_stream(...)`.
  - `Ok(stream)` → `fold_stream(stream, |c| notify_tool_streaming_chunk(
    "a2a_delegate", c))`; build `SubAgentResult` from the `FoldedOutcome`.
  - `Err(e)` → `tracing::info!` ("streaming unavailable, falling back") →
    `dispatch_sync(...)` (today's exact `send_message` body, extracted verbatim).
- The Spec 1 G2 raw-memory delegation hook fires on success in both paths.

Net ~+45 lines. `dispatch_sync` is a pure extraction of existing code.

### `src/a2a/adapter/server/routes.rs`

Add `.keep_alive(KeepAlive::new())` to the two `Sse::new(...)` constructors
(`a2a_stream_handler`, `sse_error`). The single server-side touch: Aleph's
server otherwise goes silent between `Working` and the final event, so without
keep-alive any A2A client must use an idle-timeout longer than the slowest
remote task. With keep-alive comments flowing (~15s), a tight idle-timeout is
correct.

### Timeout model

- **Idle-timeout** (90s of byte silence) — liveness detection.
- **Harness per-tool `max_duration_ms` budget** — total ceiling (already
  enforced by `ScopedToolService`).

No redundant total cap inside `dispatch`.

## Error Handling

- Card refresh failure → keep placeholder, `warn`, never panic, never block
  startup.
- Streaming open failure (non-2xx, e.g. a non-Aleph agent without `/a2a/stream`)
  → transparent sync fallback.
- Mid-stream JSON-RPC error frame or idle timeout → surfaced as a failed
  `SubAgentResult` with a clean message for the model.

## Testing

**Item 1** (`card_refresh.rs` `#[cfg(test)]`):
- empty registry → returns 0.
- unreachable URL → returns 0, placeholder preserved.
- success path against a local axum stub serving
  `/.well-known/agent-card.json` → placeholder replaced with the real card.

**Item 2:**
- `sse_stream.rs`: enveloped status-update / artifact-update parse; JSON-RPC
  error frame → `Err`; keep-alive comment skipped; bare-event back-compat;
  `fold_stream` success / failure / artifact-accumulation outcomes.
- `http_client.rs`: `send_message_stream` against a local `/a2a/stream` stub
  emitting 2 enveloped events → 2 `UpdateEvent`s; 404 → `Err`; idle timeout via
  a short `with_stream_idle` against a stub that opens then stalls.
- `sub_agent.rs`: one `execute_delegation` end-to-end streaming test against a
  local stub; the existing sync `execute_*` tests stay green.

## Files Changed (~9)

| File | Change |
|------|--------|
| `src/a2a/service/card_refresh.rs` | **new** — refresh logic + spawn helper |
| `src/a2a/service/mod.rs` | `pub mod card_refresh` + re-exports |
| `src/bin/aleph-server/commands/start/mod.rs` | `spawn_card_refresh(...)` call |
| `src/a2a/adapter/client/http_client.rs` | `send_message_stream` + `stream_idle` |
| `src/a2a/adapter/client/sse_stream.rs` | envelope fix + idle param + `fold_stream` |
| `src/a2a/sub_agent.rs` | streaming-first `dispatch` + `dispatch_sync` |
| `src/a2a/adapter/server/routes.rs` | `Sse::keep_alive` (2 lines) |
| `docs/reference/MULTI_AGENT_SYSTEM.md` | note streaming delegation + card refresh |

## Anti-Rot

- `sse_stream.rs` goes from broken-and-dead to correct-and-live — no new dead
  code introduced.
- `dispatch_sync` is an extraction, not a duplicate; both paths share the G2
  hook.
- `sub_agent.rs` grows to ~850 lines (≈460 code / ≈390 tests). Slightly over
  the 800 soft cap but mostly inline tests; splitting the file is the kind of
  churn this cycle explicitly avoids. The stream-fold logic lives in
  `sse_stream.rs` precisely to keep `sub_agent.rs` growth minimal.

## Risks

- Aleph's server currently streams only `Working` + final `Completed`/`Failed`
  (no progressive artifacts) — for Aleph-to-Aleph delegation the streaming
  payoff is mainly idle-timeout liveness + early failure. Richer payoff appears
  with external A2A agents that emit progressive artifacts. The `keep_alive`
  fix makes the liveness story correct regardless.
- Non-Aleph spec agents have no `/a2a/stream` route — the sync fallback handles
  them; their cards still refresh fine in Item 1.

## Out of Scope (carried forward)

- Periodic card re-refresh + `health` monitoring loop.
- `CardRegistry::fetch_card` / `resolve_by_intent` trait stubs (no caller).
- Spec `message/stream`-on-`/a2a` streaming interop attempt.
