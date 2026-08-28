//! `gateway.metrics.lanes` / `gateway.metrics.run_concurrency` /
//! `gateway.metrics.subagent_concurrency` — live occupancy gauges for
//! diagnostics.
//!
//! `lanes` returns the snapshot produced by [`LaneManager::snapshot`] as a
//! JSON array, `run_concurrency` returns the engine's run-lifetime
//! `ConcurrencyLimiter` snapshot (Task 4/8, audit 3.4) — "N/M run slots in
//! use" — and `subagent_concurrency` (Round-8, §4.11) returns the
//! `BackgroundAgentTracker` occupancy: live sub-agents per session plus
//! completed / consumed counts so a panel can surface the
//! `consumed / completed` dedup-hygiene ratio without scraping
//! `subagent.list`. All three live on the Query lane (registered in
//! `Lane::override_for`).

use crate::agents::background_tracker::BackgroundAgentTracker;
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::sync_primitives::Arc;

use serde_json::json;

use super::super::lane::LaneManager;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use super::agent::AgentRunManager;

/// Handle `gateway.metrics.lanes`. Returns the live occupancy snapshot of
/// every lane in a fixed order (Query / Execute / Mutate / System).
pub async fn handle_gateway_metrics_lanes(
    request: JsonRpcRequest,
    lane_manager: Arc<LaneManager>,
) -> JsonRpcResponse {
    let lanes = lane_manager.snapshot();
    JsonRpcResponse::success(request.id, json!({ "lanes": lanes }))
}

/// Whether one live session KEY — a wire string, not a row — may be shown to
/// the current caller.
///
/// The authority is the same one the rest of P1/P2 uses,
/// `visibility::session_visible` (⇒ `owner_and_scope_visible_to`: a project
/// room follows its roster, a personal session its owner). What this adds is
/// the resolution step, and one rule that is deliberately the OPPOSITE of
/// `visibility::existing_session_is_visible`: an element that cannot be
/// resolved — malformed key, no row yet, store error — is **dropped**. That
/// predicate answers "may I ADDRESS this key", where a not-yet-created session
/// is the ordinary first message of a new conversation; this one answers "may I
/// be TOLD this key exists", where "I could not work out whose this is" must
/// never mean "everyone's". The cost is that a run claimed a few milliseconds
/// before its session row is written is briefly absent from a member's gauge;
/// the event plane makes the identical trade for the identical reason, and the
/// repair for both lives on the event plane — `execute.rs` re-publishes
/// `stream.running_set_changed` at a fresh seq the moment `ensure_session`
/// makes the row resolvable (`SessionRunRegistry::republish_running_set`). A
/// cold load that lands inside that window sees a short gauge until that frame
/// arrives; it is NOT repaired by a later poll, because
/// `SessionMap::seed_server_running` only applies before any frame has come in.
async fn running_key_is_visible(store: &dyn SessionStore, key: &str) -> bool {
    let Some(parsed) = SessionKey::from_key_string(key) else {
        return false;
    };
    matches!(
        store.get_metadata(&parsed).await,
        Ok(Some(meta)) if visibility::session_visible(&meta)
    )
}

/// Narrow a set of live session keys to the ones the current caller may see.
///
/// Twin of [`crate::gateway::event_visibility::EventVisibilityIndex::project_for`],
/// which narrows the SAME set on the event plane — the two are pinned to agree
/// by `the_gauge_and_the_event_plane_narrow_the_same_set`. The one deliberate
/// asymmetry is the first arm: an unrestricted caller (no `CALLER_USER` scoped
/// — cron, A2A, an in-process test) is unchanged and does not even touch the
/// store, matching every predicate in [`crate::gateway::visibility`]. The event
/// plane has no equivalent arm because there `caller_user: None` means a walled
/// socket, which is a denial rather than a privilege.
async fn visible_running_keys(store: &dyn SessionStore, keys: Vec<String>) -> Vec<String> {
    if visibility::visible_owner_filter().is_none() {
        return keys;
    }
    let mut visible = Vec::with_capacity(keys.len());
    for key in keys {
        if running_key_is_visible(store, &key).await {
            visible.push(key);
        }
    }
    visible
}

/// Handle `gateway.metrics.run_concurrency`. Returns the live run-slot
/// occupancy snapshot from the execution engine's `ConcurrencyLimiter` (Task
/// 4) — "N/M run slots in use", per-agent breakdown, and queue depth — plus
/// `running_sessions`, the authoritative set of session keys with an in-flight
/// run (from the `SessionRunRegistry`). The latter lets a Panel paint
/// per-session running indicators on a fresh load and for runs started by any
/// interface, independent of client-side run-event refcounting.
///
/// **The two arrays that name sessions are filtered to the caller**
/// (`running_sessions` and `busy_queue.per_session`) — this is the RPC half of
/// the `stream.running_set_changed` projection, and the two must move together:
/// this method is the Panel's cold-load seed for the very dots that event
/// keeps live, so filtering one and not the other leaves a member's sidebar
/// either leaking or wrong. Registered `ListFiltered` in `method_visibility`
/// and carved into `method_admin::MEMBER_CARVE_OUTS` for the same reason —
/// admin-gating it (the `gateway.` family default) made the seed and the gauge
/// dead for exactly the population the filtering is for.
///
/// **`run_concurrency.per_agent` is DROPPED for a MEMBER**, and it deliberately
/// asks a DIFFERENT question from the two arrays above: `{agent_id, in_use}`
/// names agent PERSONAS, which are server-global configuration rather than
/// per-user rows, so withholding it is a privilege decision. It therefore asks
/// the privilege predicate — `caller_identity::caller_is_member`, the single
/// source `method_admin`'s gate, `exec_approvals`, `exec_grants` and
/// `config.get` already converged on.
///
/// ⚠️ It asked `visible_owner_filter().is_some()` until 2026-08-29, and the
/// sentence that stood right here claimed "an unrestricted caller (internal /
/// operator — `visible_owner_filter()` is `None`) still gets the whole
/// snapshot". The operator half of that was false.
/// `handlers::connect::resolve_connection_identity` scopes EVERY authorized
/// connection — loopback included — with `CALLER_USER = Some(OWNER_USER_ID)`,
/// so the filter is `Some(..)` for an operator and `per_agent` was dropped for
/// every client on every deployment, the stock single-user desktop included.
/// The field had a producer and a wire slot and no reachable reader, which
/// looks exactly like a field nobody wired. The test below could not see it
/// either: its "unrestricted" arm scoped no caller at all, which is the
/// internal-caller case, never the operator's.
///
/// The two arrays that NAME SESSIONS keep the ownership predicate, and that is
/// not an oversight left behind: a session is a per-user row and P2's product
/// boundary there is owner-only by ruling (see
/// [`crate::gateway::visibility::visible_owner_filter`]'s doc). Privilege and
/// ownership are different questions and this one response answers both — one
/// per field, not one per handler.
///
/// What genuinely IS load and stays process-wide for everyone:
/// `global_in_use` / `global_total` / `per_agent_cap` / `waiting` and
/// `busy_queue.total_waiting`. A member seeing "6/8 slots in use" learns how
/// busy the server is, not whose work it is doing — that is the gauge's entire
/// purpose.
pub async fn handle_gateway_metrics_run_concurrency(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let mut run_concurrency =
        serde_json::to_value(run_manager.concurrency_snapshot()).unwrap_or_else(|_| json!({}));
    // `per_agent` is the third identity array in this response, but it names
    // agent personas rather than sessions — server-global config, so the
    // question is PRIVILEGE and the predicate is the privilege one. See this
    // function's doc for why the ownership filter was the wrong question here
    // and how it made the field unreadable on every deployment.
    if crate::gateway::caller_identity::caller_is_member() {
        if let Some(obj) = run_concurrency.as_object_mut() {
            obj.remove("per_agent");
        }
    }
    let running_sessions =
        visible_running_keys(sessions.as_ref(), run_manager.running_sessions()).await;
    // Backlog waiting *behind* the run slots. Without it the gauge showed a
    // saturated engine but not the queue depth piling up behind it, and a
    // queued message was invisible on every surface (codex renders queued
    // messages in its bottom pane; Aleph showed nothing at all).
    let busy = crate::gateway::busy_queue::snapshot();
    // Same narrowing, same function — the wait lanes are keyed by session too,
    // so an unfiltered `per_session` would re-open on the queue-depth field
    // exactly what `running_sessions` just closed.
    let visible_lanes: std::collections::HashSet<String> = visible_running_keys(
        sessions.as_ref(),
        busy.per_session.iter().map(|(k, _)| k.clone()).collect(),
    )
    .await
    .into_iter()
    .collect();
    let per_session: Vec<_> = busy
        .per_session
        .iter()
        .filter(|(session_key, _)| visible_lanes.contains(session_key))
        .map(|(session_key, depth)| json!({ "session_key": session_key, "depth": depth }))
        .collect();
    JsonRpcResponse::success(
        request.id,
        json!({
            "run_concurrency": run_concurrency,
            "running_sessions": running_sessions,
            "busy_queue": {
                "total_waiting": busy.total_waiting,
                "per_session": per_session,
            },
        }),
    )
}

/// Round-8 (§4.11) — Handle `gateway.metrics.subagent_concurrency`. Returns
/// the live background-sub-agent occupancy snapshot from
/// [`BackgroundAgentTracker::subagent_snapshot`]: live sub-agents per
/// session (sorted by session key), the presence-only subtotal (sync
/// fan-out seats that are not enumerated by the `subagent` tool but DO
/// count against the parent's Interrupt-demote budget), and the
/// `completed_total` / `consumed_total` pair so a panel can surface the
/// `consumed / completed` dedup-hygiene ratio.
///
/// Process-wide by default; pass `params = {"scope": "agent:<id>:peer:user"}`
/// to limit to one session.
pub async fn handle_gateway_metrics_subagent_concurrency(
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    let scope = request
        .params
        .as_ref()
        .and_then(|p| p.get("scope"))
        .and_then(|v| v.as_str());
    let snap = BackgroundAgentTracker::global().subagent_snapshot(scope);
    JsonRpcResponse::success(request.id, json!({ "subagent_concurrency": snap }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::lane::LaneConfig;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use serde_json::json;
    use tempfile::TempDir;

    fn empty_session_store() -> (Arc<dyn SessionStore>, TempDir) {
        let temp = TempDir::new().unwrap();
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        (Arc::new(store), temp)
    }

    async fn owned_session(store: &Arc<dyn SessionStore>, conv: &str, owner: &str) -> String {
        let key = SessionKey::main(conv);
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal(owner)),
            store.get_or_create(&key),
        )
        .await
        .unwrap();
        key.to_key_string()
    }

    #[tokio::test]
    async fn returns_one_entry_per_lane_in_fixed_order() {
        let manager = Arc::new(LaneManager::new(LaneConfig::default()));
        let req = JsonRpcRequest::with_id("gateway.metrics.lanes", None, json!(1));
        let resp = handle_gateway_metrics_lanes(req, manager).await;

        assert!(resp.is_success());
        let result = resp.result.unwrap();
        let lanes = result["lanes"].as_array().expect("lanes must be array");
        assert_eq!(lanes.len(), 4);
        assert_eq!(lanes[0]["lane"], "Query");
        assert_eq!(lanes[1]["lane"], "Execute");
        assert_eq!(lanes[2]["lane"], "Mutate");
        assert_eq!(lanes[3]["lane"], "System");

        // Single-pool lanes omit the desktop split (None → JSON null).
        assert!(lanes[0]["desktop_total"].is_null());
        assert!(lanes[0]["desktop_available"].is_null());

        // Channel-class-split lanes carry both pool sizes.
        assert!(lanes[1]["desktop_total"].as_u64().is_some());
        assert!(lanes[1]["shared_total"].as_u64().is_some());
    }

    /// Minimal `ToolRegistry` double: this test never looks up or executes a
    /// tool, so an empty registry satisfies `ExecutionEngine<P, R>`'s generic
    /// bound without pulling in the real (heavy) `BuiltinToolRegistry`.
    /// Mirrors `execution_engine::tests::EmptyToolRegistry`.
    struct EmptyToolRegistry;

    impl crate::executor::ToolRegistry for EmptyToolRegistry {
        fn get_tool(&self, _name: &str) -> Option<&crate::tool_metadata::UnifiedTool> {
            None
        }

        fn execute_tool(
            &self,
            _tool_name: &str,
            _arguments: serde_json::Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = crate::error::Result<serde_json::Value>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async { Err(crate::error::AlephError::tool("no tools in test registry")) })
        }
    }

    /// Wires a real `ExecutionEngine` (default config: `max_runs_global = 8`)
    /// through the exact same `Arc<dyn ExecutionAdapter>` → `AgentRunManager`
    /// path production boot uses, so tests using it prove the whole access
    /// chain — not just a hand-built double.
    fn real_run_manager() -> Arc<AgentRunManager> {
        use crate::gateway::agent_instance::AgentRegistry;
        use crate::gateway::event_bus::GatewayEventBus;
        use crate::gateway::execution_adapter::ExecutionAdapter;
        use crate::gateway::execution_engine::{ExecutionEngine, ExecutionEngineConfig};
        use crate::gateway::router::AgentRouter;

        let engine = ExecutionEngine::new(
            ExecutionEngineConfig::default(),
            Arc::new(crate::thinker::SingleProviderRegistry::new(
                crate::providers::create_mock_provider(),
            )),
            Arc::new(EmptyToolRegistry),
            Vec::new(),
            None,
        );
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(engine);
        Arc::new(AgentRunManager::new(
            Arc::new(AgentRouter::new()),
            Arc::new(GatewayEventBus::new()),
            Arc::new(AgentRegistry::new()),
            execution_adapter,
        ))
    }

    #[tokio::test]
    async fn run_concurrency_reports_default_global_total() {
        let run_manager = real_run_manager();
        let (sessions, _temp) = empty_session_store();
        let req = JsonRpcRequest::with_id("gateway.metrics.run_concurrency", None, json!(1));
        let resp = handle_gateway_metrics_run_concurrency(req, run_manager, sessions).await;

        assert!(resp.is_success());
        let result = resp.result.unwrap();
        assert_eq!(result["run_concurrency"]["global_total"], 8);
        assert_eq!(result["run_concurrency"]["global_in_use"], 0);
        // Second-optimization fields: per-agent sub-cap, idle queue depth, and
        // an empty per-agent breakdown when nothing is running.
        assert_eq!(result["run_concurrency"]["per_agent_cap"], 3);
        assert_eq!(result["run_concurrency"]["waiting"], 0);
        assert!(result["run_concurrency"]["per_agent"]
            .as_array()
            .expect("per_agent is an array")
            .is_empty());
        // Sibling field sourced from the SessionRunRegistry — empty at rest.
        assert!(result["running_sessions"]
            .as_array()
            .expect("running_sessions is an array")
            .is_empty());
    }

    /// Round-8 — `gateway.metrics.subagent_concurrency` reads the
    /// process-global `BackgroundAgentTracker` directly. We seed a couple of
    /// entries (with unique ids so the process-global map stays isolated
    /// across cargo-test invocations) and assert the snapshot reflects them.
    #[tokio::test]
    async fn subagent_concurrency_reports_tracker_occupancy() {
        use crate::agents::background_tracker::{CompletedOutcome, SpawnMeta};
        use tokio_util::sync::CancellationToken;

        let tracker = crate::agents::background_tracker::BackgroundAgentTracker::global();
        let live_id = format!("gmc-live-{}", uuid::Uuid::new_v4());
        let done_id = format!("gmc-done-{}", uuid::Uuid::new_v4());
        tracker.register_with_meta(
            live_id.clone(),
            CancellationToken::new(),
            "live task".into(),
            SpawnMeta {
                root_session: "agent:main:peer:user".into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        tracker.register_with_meta(
            done_id.clone(),
            CancellationToken::new(),
            "done task".into(),
            SpawnMeta {
                root_session: "agent:main:peer:user".into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        tracker.mark_completed(&done_id, CompletedOutcome::ok_text("x"));
        tracker.mark_consumed(&done_id);

        // Process-wide view.
        let req = JsonRpcRequest::with_id("gateway.metrics.subagent_concurrency", None, json!(1));
        let resp = handle_gateway_metrics_subagent_concurrency(req).await;
        assert!(resp.is_success());
        let result = resp.result.expect("ok");
        let snap = &result["subagent_concurrency"];
        // `running_total` reflects the process-global set, which other
        // tests in this run may have populated; the exact number is
        // unstable. Instead we assert OUR id is in `running_per_session`.
        let per_session = snap["running_per_session"]
            .as_array()
            .expect("running_per_session is an array");
        let own_session = per_session
            .iter()
            .find(|r| r["session"] == "agent:main:peer:user")
            .expect("our seeded session must appear");
        assert!(
            own_session["count"].as_u64().unwrap() >= 1,
            "seeded live id must count toward the session's running tally"
        );
        // Completed / consumed are also process-global, so we just check
        // our id was seen by the dedup counter:
        assert!(snap["consumed_total"].as_u64().unwrap() >= 1);

        // Scoped view: ask for the seeded session and assert the count
        // matches.
        let req_scoped = JsonRpcRequest::with_id(
            "gateway.metrics.subagent_concurrency",
            Some(json!({ "scope": "agent:main:peer:user" })),
            json!(2),
        );
        let resp_scoped = handle_gateway_metrics_subagent_concurrency(req_scoped).await;
        assert!(resp_scoped.is_success());
        let snap_scoped = resp_scoped.result.unwrap();
        let own_session_scoped = snap_scoped["subagent_concurrency"]["running_per_session"]
            .as_array()
            .expect("array")
            .iter()
            .find(|r| r["session"] == "agent:main:peer:user")
            .expect("our session in scoped view");
        assert_eq!(
            own_session_scoped["count"], own_session["count"],
            "scoped view must report the same per-session count for the seeded session"
        );
    }

    // ── Caller narrowing (the RPC half of the running-set projection) ────

    /// The gauge stops enumerating other people's live sessions. Asserted on
    /// the RETURNED SET, not on the predicate having been consulted.
    #[tokio::test]
    async fn the_gauge_narrows_the_running_set_to_the_calling_member() {
        let (sessions, _temp) = empty_session_store();
        let alice = owned_session(&sessions, "gm-narrow-alice", "u-alice").await;
        let bob = owned_session(&sessions, "gm-narrow-bob", "u-bob").await;

        let seen = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                visible_running_keys(sessions.as_ref(), vec![alice.clone(), bob.clone()]).await
            })
            .await;
        assert_eq!(seen, vec![alice.clone()]);

        let seen_by_bob = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                visible_running_keys(sessions.as_ref(), vec![alice, bob.clone()]).await
            })
            .await;
        assert_eq!(seen_by_bob, vec![bob]);
    }

    /// Zero-change guarantee for the unrestricted caller (CLI / cron / an
    /// in-process test): the set comes back verbatim, INCLUDING keys with no
    /// row behind them — the store is not consulted at all, so a run claimed
    /// milliseconds before its session row exists is not dropped from the
    /// operator's gauge the way it is from a member's.
    #[tokio::test]
    async fn an_unrestricted_caller_gets_the_whole_running_set_verbatim() {
        let (sessions, _temp) = empty_session_store();
        let alice = owned_session(&sessions, "gm-unrestricted", "u-alice").await;
        let never_created = SessionKey::main("gm-unrestricted-ghost").to_key_string();

        let keys = vec![alice, never_created, "not even a key".to_string()];
        assert_eq!(
            visible_running_keys(sessions.as_ref(), keys.clone()).await,
            keys,
            "no CALLER_USER scoped ⇒ unchanged, exactly like every predicate in \
             `visibility`"
        );
    }

    /// The reason this handler and `event_visibility` are one change: they are
    /// two producers of ONE set (this is the Panel's cold-load seed for the
    /// very dots `stream.running_set_changed` then keeps live), so a caller
    /// must get the same answer from both or their sidebar flips on refresh.
    /// Pinning them against each other is what makes "fixing either alone
    /// leaves the other broken" a test failure rather than a comment.
    #[tokio::test]
    async fn the_gauge_and_the_event_plane_narrow_the_same_set() {
        use crate::gateway::event_visibility::EventVisibilityIndex;

        let (sessions, _temp) = empty_session_store();
        let alice = owned_session(&sessions, "gm-agree-alice", "u-alice").await;
        let bob = owned_session(&sessions, "gm-agree-bob", "u-bob").await;
        let ghost = SessionKey::main("gm-agree-ghost").to_key_string();
        let all = vec![alice.clone(), bob, ghost, "junk".to_string()];

        let via_rpc = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                visible_running_keys(sessions.as_ref(), all.clone()).await
            })
            .await;

        let payload = json!({ "seq": 1, "running": all });
        let projected = EventVisibilityIndex::new()
            .project_for(
                "stream.running_set_changed",
                Some(&payload),
                Some("u-alice"),
                &sessions,
            )
            .await
            .expect("the event plane projects this frame");
        let via_event: Vec<String> = projected["running"]
            .as_array()
            .expect("running array")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();

        assert_eq!(via_rpc, vec![alice]);
        assert_eq!(
            via_rpc, via_event,
            "the RPC gauge and the event projection must narrow the same set the \
             same way — they feed the same red dot"
        );
    }

    /// End-to-end through the real handler, and specifically over
    /// `busy_queue.per_session`: the wait lanes are keyed by session too, so
    /// leaving them unfiltered would re-open on the queue-depth field exactly
    /// what `running_sessions` closes — and would make this method's
    /// `ListFiltered` registration a lie.
    #[tokio::test]
    async fn the_handler_filters_the_busy_queue_lanes_as_well() {
        let (sessions, _temp) = empty_session_store();
        let alice = owned_session(&sessions, "gm-lane-alice", "u-alice").await;
        let bob = owned_session(&sessions, "gm-lane-bob", "u-bob").await;

        // Hold both tickets for the duration: dropping a guard leaves its lane.
        let _alice_ticket = crate::gateway::busy_queue::register(&alice, 8, "run-a")
            .expect("lane accepts alice's message");
        let _bob_ticket = crate::gateway::busy_queue::register(&bob, 8, "run-b")
            .expect("lane accepts bob's message");

        let req = JsonRpcRequest::with_id("gateway.metrics.run_concurrency", None, json!(1));
        let resp = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_gateway_metrics_run_concurrency(req, real_run_manager(), sessions.clone())
                    .await
            })
            .await;

        assert!(resp.is_success());
        let result = resp.result.expect("ok");
        let lanes: Vec<String> = result["busy_queue"]["per_session"]
            .as_array()
            .expect("per_session is an array")
            .iter()
            .map(|e| e["session_key"].as_str().unwrap().to_string())
            .collect();
        assert!(
            lanes.contains(&alice),
            "alice must still see her own queued message"
        );
        assert!(
            !lanes.contains(&bob),
            "bob's queued message must not be enumerated to alice"
        );
    }

    /// The third identity array in this response. `per_agent` is
    /// `[{agent_id, in_use}]` — which agent personas hold a live run *right
    /// now* — so leaving it verbatim hands a member the same live-activity
    /// correlation the two session arrays were narrowed to remove. It is
    /// dropped for a MEMBER, not summarised and not excused in a comment.
    ///
    /// ⚠️ **The operator arm is the one that matters, and it is the one this
    /// test did not have.** Until 2026-08-29 the "unrestricted" arm below
    /// scoped no caller at all — which is the INTERNAL caller (cron, A2A, an
    /// in-process test), never a connection. Every real connection, loopback
    /// included, carries `CALLER_USER = Some(OWNER_USER_ID)`, so the
    /// then-current ownership predicate dropped `per_agent` for every client on
    /// every deployment while this test stayed green. A fixture that cannot
    /// reach the case is not coverage of it, so all three arms are here now and
    /// each names the task-locals a real connection of that kind carries.
    #[tokio::test]
    async fn the_per_agent_breakdown_is_withheld_from_a_member_and_only_a_member() {
        use crate::gateway::caller_identity::CALLER_ROLE;
        use crate::gateway::security::store::OWNER_USER_ID;

        let (sessions, _temp) = empty_session_store();

        async fn snapshot(
            sessions: Arc<dyn SessionStore>,
            id: i64,
            user: Option<&str>,
            role: Option<&str>,
        ) -> serde_json::Value {
            CALLER_USER
                .scope(
                    user.map(str::to_string),
                    CALLER_ROLE.scope(role.map(str::to_string), async {
                        handle_gateway_metrics_run_concurrency(
                            JsonRpcRequest::with_id(
                                "gateway.metrics.run_concurrency",
                                None,
                                json!(id),
                            ),
                            real_run_manager(),
                            sessions,
                        )
                        .await
                    }),
                )
                .await
                .result
                .expect("ok")
        }

        // (a) Internal caller: no identity scoped at all.
        let internal = snapshot(sessions.clone(), 1, None, None).await;
        assert!(
            internal["run_concurrency"]["per_agent"].is_array(),
            "the premise: an unscoped internal caller receives the whole snapshot"
        );

        // (b) An OPERATOR connection — loopback or an admin-bound device. This
        //     is what `resolve_connection_identity` actually stamps, and it is
        //     the arm whose absence hid the defect.
        let operator = snapshot(sessions.clone(), 2, Some(OWNER_USER_ID), Some("operator")).await;
        assert!(
            operator["run_concurrency"]["per_agent"].is_array(),
            "an operator carries CALLER_USER = Some(OWNER_USER_ID); the field \
             this response exists to serve must survive that, or no shipped \
             surface can ever read it"
        );

        // (c) A MEMBER connection.
        let member = snapshot(sessions.clone(), 3, Some("u-alice"), Some("member")).await;
        assert!(
            member["run_concurrency"]
                .as_object()
                .expect("run_concurrency is an object")
                .get("per_agent")
                .is_none(),
            "a member must not learn which agent personas are running right now"
        );
        // The LOAD numbers — the reason the carve-out exists at all — survive.
        assert_eq!(member["run_concurrency"]["global_total"], 8);
        assert_eq!(member["run_concurrency"]["global_in_use"], 0);
        assert_eq!(member["run_concurrency"]["waiting"], 0);
    }
}
