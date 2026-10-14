# Task 56 window 5 — `reveal_browser` failure: source-level diagnosis (PARTIAL; defensive hardening only, no proven fix)

Status: **PARTIAL / BLOCKED on one missing fact; root cause still NOT proven.**
Sections "Proven" / "Unexplained" / "Fix assessment" below are the original diagnosis (written when **no** production
source, predicate or harness file had been changed and nothing had been run; evidence was code reading + the window-5
Gateway log). They are kept as the historical record. **Superseded in part by the "Diagnostic follow-up" section
below:** Panel subscription hardening now exists in the working tree. It is a diagnostic follow-up / defensive
hardening, **not a proven fix**.

## Diagnostic follow-up (defensive hardening, not proven fix)
Scope: working-tree changes (dirty, uncommitted) in `interfaces/webchat/src/context.rs` and
`interfaces/webchat/src/app.rs`. Motivation: the window-5 log shows the Panel connection subscribed to some topics but
never `approval.**` / `config.**` / `browser.live.*`, consistent with the subscribe chain stalling or one of its RPCs
never being answered. These changes remove three plausible stall / loss paths **without** claiming that any of them
was the window-5 cause (none was observed; the Panel console and per-RPC results were never captured).
1. `send_tracked` (`context.rs`): the pending-response entry is registered **before** `connector.send`; a failed
   send un-registers it and answers the caller with a local `RpcFailure`. Rationale: a reply that finds no entry is
   dropped silently and the caller (and every `events.subscribe` queued behind it) waits out the 30 s ceiling.
2. Approval pending seed (`setup_approval_subscriptions`, `context.rs`): the `exec.approvals.pending` seed no longer
   runs on the subscription path (own `spawn_local`), and is applied only when `approval_seed_applies` holds
   (same `connection_epoch`, no newer `approval.*` refetch). Rationale: a slow seed must not hold the mount effect's
   later subscriptions, and a late seed must not overwrite a newer list or a dead connection's list.
3. `browser.live.resource` / `browser.live.closed` (`app.rs`): after "Connected to Gateway" each topic is subscribed in
   its own `spawn_local`, independent of the alert / approval subscribe chain; failure only logs a console warning and
   never marks any viewer connected or live.

Verification actually run (by the controller, offline, Panel crate only):
- `cargo test -p aleph-panel --lib --offline` — **1530 / 1530 passed**.
- `cargo check -p aleph-panel --target wasm32-unknown-unknown --offline --lib` — **exit 0**.
- Unit tests for the new helpers live in `context.rs` (`a_request_is_registered_before_it_is_sent`,
  `a_failed_send_unregisters_and_answers_the_caller`, `a_stale_approval_seed_is_discarded`).

**Not verified / not claimed:**
- Not a Native / Tauri / WebView2 / browser proof; no window was run (no sixth window; the Native budget stays
  exhausted); no Gateway, keyring, HOME or selftest was run for this follow-up.
- `interfaces/webchat/dist` was **NOT rebuilt**; the dist / WASM served in window 5 does not contain these changes,
  and no `just wasm` was run after them. The hardening has therefore never executed in a real Panel.
- The window-5 root cause remains unproven; the `reveal_browser` failure is **not** shown to be fixed. The
  late-subscribe / Panel-reload gap (no replay RPC, see Proven §2) is unchanged and still open.
- Not a whole-branch pass; only the Panel lib and the wasm32 type check above were run. #48 / #53 / #56 are not
  claimed complete; overall remains PARTIAL, not production ready.

## Proven
1. `reveal_browser` acks false iff the `workspace-tab-browser` button never appears within 500 ms
   (`desktop/shell/src/main.rs` QA_MONITOR_JS `revealBrowser`). The tab renders only when
   `BrowserWorkspace.model.bound().is_some()` (`interfaces/webchat/src/platform/wide/views/browser/model.rs:581`,
   `components/workspace_panel.rs` `has_browser`). `bound` is set ONLY by `apply_browser_event` on a
   `browser.live.resource` frame (`views/browser/bridge.rs`). So window 5 failed because the Panel model
   never received/accepted a resource frame.
2. There is NO replay or snapshot path for it. `events.subscribe` (`src/gateway/handlers/events.rs::handle_subscribe`)
   only adds the filter; the only browser-viewer RPCs are `browser.viewer.grant.issue|revoke` and
   `browser.viewer.resource.decide` (`src/gateway/handlers/browser_viewer.rs::register`). The Panel model is
   in-memory (`BrowserWorkspace::new()` in `app.rs`). Any frame published before the Panel's socket holds the
   subscription — and any Panel reload — loses the resource permanently. (Explains the driver's "races the
   resource/tab replay" comment and window 4's all-false post-reload DOM.)
3. Window-5 Gateway log (`.analysis/browser-live-runtime/home/logs/aleph-server.log.2026-10-12`, lines 9164-9173):
   - fixture conn 127.0.0.1:59123 subscribed `["browser.live.*"]` at 21:41:58.620;
   - Panel conn 127.0.0.1:59122 subscribed ONLY `alerts.**` (x2), `runtime.agents.changed`, `canvas.updated`,
     `session.artifact` (21:41:58.643-.663) — **never `browser.live.resource`/`browser.live.closed`, and also never
     `approval.**` / `config.**`** (both are in `BASE_TOPICS`, `interfaces/webchat/src/context.rs:430`, and in the
     app.rs mount loop);
   - `agent.run` started 21:41:58.709, i.e. after those subscriptions; the resource was published ~21:41:59.9;
   - conn 59122 closed 21:42:41.868. So the Gateway never had a browser.live subscription for the Panel;
     visibility (`event_visibility.rs` `ByBrowserLiveScope`) was never even consulted for it.
4. Not a visibility problem on this evidence: the Panel is credential-less loopback => `OWNER_USER_ID`
   (`handlers/connect.rs::resolve_connection_identity`); no `browser.live.` rule in `event_scope.rs`.
5. Not stale WASM for the subscription code: `dist/aleph_panel_bg.wasm` contains `browser.live.resource`,
   `browser.live.closed`, `Subscribed to approval`, `config.**`, `workspace-tab-browser`, `browser-live-decision`
   (`app.rs` last changed 10-10, dist built 10-12 17:13).

## Unexplained (the exact blocker)
Why did the running Panel subscribe `alerts.**` but not `approval.**`, `config.**` or `browser.live.*`, although
both the replay (`context.rs` ~1815 `replay_subscriptions(topics_to_replay())`, `replay_set` adds BASE_TOPICS)
and the `app.rs` mount effect (`setup_approval_subscriptions`, then the RESOURCE/CLOSED loop) issue them?
Either those `events.subscribe` RPCs never left the Panel / were refused before `handle_subscribe` logs success
(error paths return before the INFO line), or the Panel chain stalled after the first alert subscribe. Candidate
causes I could not distinguish by reading: `await_gateway_ready` stall, wall/`NeedsToken` path
(`context.rs` ~1791: "do NOT spawn subscriptions"), or a client-side console error. Missing evidence: the Panel
WebView console (`Failed to subscribe to ...`, `Failed to setup approval subscriptions`) and the Gateway's
per-RPC result for conn 59122 between 21:41:58.64 and .66. Neither is captured by the current harness.

## Fix assessment
- A "Panel reconnect/replay" fix cannot be proven: the evidence says the subscription itself was never established.
- A real replay (new gateway RPC listing the caller's active browser-live resources + Panel call after subscribe)
  would close the late-subscribe/reload gap, but it is a new authenticated, visibility-gated RPC in the Gateway
  (security surface, `ByBrowserLiveScope` re-derivation, protocol change) — not a low-risk minimal change, and it
  would still not fix a Panel that never subscribes. Not implemented (still true after the follow-up above, which
  changes only Panel-side subscription ordering / fencing and adds no Gateway RPC).
- No harness/predicate change made or proposed to weaken acceptance (unchanged after the follow-up).

## Needed before the next (single) Native window
1. Capture Panel console + a post-subscribe `events.list` for the Panel connection (diagnostic only), or read
   Gateway per-RPC outcomes for conn 59122, to see why `approval.**`/`config.**`/`browser.live.*` subscribes
   did not land.
2. Stale-bundle is NOT indicated: dist (10-12 17:13) is newer than the Panel sources I checked (10-12 16:09-16:41),
   and the wasm carries the expected strings.
