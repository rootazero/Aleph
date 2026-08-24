//! Kanban data plane: project-scoped listing for goals and loops.
//!
//! `teams.list` already carries `scope_id` per row (`teams::types::TeamSummary`,
//! landed round-5) and is filtered through `ScopedTeamStore`/`teams::scoped::
//! team_visible` — goals and loops had NO gateway RPC surface at all before
//! this module. `GoalTool`'s and `LoopManageTool`'s own `list` action answers
//! the MODEL: a rendered text summary, scoped to the calling session unless
//! the caller is an operator. This module is the structured, Panel-facing
//! counterpart the project Kanban tab reads (design doc
//! `2026-08-24-multiuser-teamchat-p3` §B1).
//!
//! Both stores are process globals (`goal::global()` / `looping::global()`),
//! so — unlike `teams.*`, which needs a `TeamStore` handle threaded through
//! boot — these handlers read them directly and register in the main
//! [`super::HandlerRegistry::new`] table, the same way `dreaming.run_now`
//! reads the process-global dream daemon.
//!
//! # Visibility
//!
//! [`super::super::visibility::stamped_owner_visible`]'s doc names
//! `goal::Goal::scope_id` and `looping::LoopState::scope_id` as scope facts
//! it DELIBERATELY leaves unread: a project room does not, by default, own
//! its members' background work (ruled 2026-08-07). That doc also predicts
//! the shape a widening would need if one were ever ruled: "the shape is ONE
//! scope-aware sibling in this module taking `(owner_user_id, scope_id)` and
//! delegating to the same `crate::projects::roster::is_member` call
//! `project_visible` makes". The multiuser-teamchat-p3 design (§B1) makes
//! that ruling — but ONLY for an EXPLICIT `scope_id` ask; the unfiltered
//! list keeps the pre-existing owner-only default:
//!
//! - `scope_id: None` → [`super::super::visibility::stamped_owner_visible`]
//!   per row, unchanged from every other background-work surface.
//! - `scope_id: Some("project:p-x")` →
//!   [`super::super::visibility::project_visible`] gates the QUERIED scope
//!   once (non-member ⇒ empty set, never an error — no existence oracle),
//!   then rows are retained by an exact `scope_id` match. A `scope_id` that
//!   does not parse to a `Project` scope (personal / org / garbage) is not
//!   Kanban-addressable and fails closed to an empty set — it must never
//!   leak whether it happens to equal some row's raw stamp.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::visibility;
use crate::goal::{Goal, GoalStatus};
use crate::looping::LoopState;
use crate::scope::ScopeId;

/// Shared params shape for both `goal.list` and `loop.list` — an entirely
/// optional filter, so a bare `{}` (or no `params` at all) means "no scope
/// filter, current status quo".
#[derive(Debug, Default, Deserialize)]
struct ScopeFilterParams {
    #[serde(default)]
    scope_id: Option<String>,
}

/// Parse `request.params` into an optional scope filter, treating an absent
/// or `null` params object as the all-defaults case rather than
/// `INVALID_PARAMS` — every field here is optional, so a caller with nothing
/// to filter on has nothing to send. `serde_json::from_value` does not treat
/// `Value::Null` as "use the Default" for a struct type, so that case is
/// handled explicitly rather than relying on it.
#[allow(clippy::result_large_err)]
fn parse_scope_filter(request: &JsonRpcRequest) -> Result<Option<String>, JsonRpcResponse> {
    match &request.params {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => serde_json::from_value::<ScopeFilterParams>(v.clone())
            .map(|p| p.scope_id)
            .map_err(|e| {
                JsonRpcResponse::error(
                    request.id.clone(),
                    INVALID_PARAMS,
                    format!("Invalid params: {e}"),
                )
            }),
    }
}

/// Whether `scope_id` (the QUERIED scope, not a row's own stamp) admits the
/// current caller. `None` (no filter) is not routed here — see the module
/// doc for that path.
fn scope_query_visible(scope_id: &str) -> bool {
    matches!(ScopeId::parse(scope_id), Some(ScopeId::Project(p)) if visibility::project_visible(&p))
}

/// One goal row projected for the Kanban board.
#[derive(Debug, Clone, Serialize)]
struct GoalSummary {
    session_id: String,
    objective: String,
    status: GoalStatus,
    scope_id: Option<String>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

impl From<&Goal> for GoalSummary {
    fn from(g: &Goal) -> Self {
        Self {
            session_id: g.session_id.clone(),
            objective: g.objective.clone(),
            status: g.status,
            scope_id: g.scope_id.clone(),
            created_at_ms: g.created_at_ms,
            updated_at_ms: g.updated_at_ms,
        }
    }
}

/// Handle `goal.list` — structured, scope-filterable goal listing for the
/// Kanban tab. See the module doc for the visibility rule. A goal subsystem
/// that has not initialized its process-global store yet (early boot, or a
/// test that never called `goal::init_global`) reads as "no goals" rather
/// than an error, matching `goal::mod`'s own "dormant" framing.
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let scope_id = match parse_scope_filter(&request) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let Some(store) = crate::goal::global() else {
        return JsonRpcResponse::success(request.id, json!({ "goals": Vec::<GoalSummary>::new() }));
    };

    let mut goals = match store.list_all() {
        Ok(g) => g,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to list goals: {e}"),
            )
        }
    };

    match scope_id.as_deref() {
        None => goals.retain(|g| visibility::stamped_owner_visible(g.owner_user_id.as_deref())),
        Some(sid) => {
            if scope_query_visible(sid) {
                goals.retain(|g| g.scope_id.as_deref() == Some(sid));
            } else {
                goals.clear();
            }
        }
    }

    let summaries: Vec<GoalSummary> = goals.iter().map(GoalSummary::from).collect();
    JsonRpcResponse::success(request.id, json!({ "goals": summaries }))
}

/// One loop row projected for the Kanban board. `status` is the canonical
/// snake_case string (`LoopStatus::as_str`) rather than the enum itself —
/// unlike `GoalStatus`, `LoopStatus` does not derive `Serialize` (it is
/// process-memory only, never persisted as JSON).
#[derive(Debug, Clone, Serialize)]
struct LoopSummary {
    session_id: String,
    prompt: String,
    status: &'static str,
    scope_id: Option<String>,
    created_at_ms: u64,
}

impl From<&LoopState> for LoopSummary {
    fn from(l: &LoopState) -> Self {
        Self {
            session_id: l.session_id.clone(),
            prompt: l.prompt.clone(),
            status: l.status.as_str(),
            scope_id: l.scope_id.clone(),
            created_at_ms: l.created_at_ms,
        }
    }
}

/// Handle `loop.list` — the loop twin of [`handle_list`] (goals). Same
/// params, same visibility rule, same "uninitialized registry reads as
/// empty" convention.
pub async fn handle_loop_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let scope_id = match parse_scope_filter(&request) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let Some(registry) = crate::looping::global() else {
        return JsonRpcResponse::success(request.id, json!({ "loops": Vec::<LoopSummary>::new() }));
    };

    let mut loops = registry.list_all();

    match scope_id.as_deref() {
        None => loops.retain(|l| visibility::stamped_owner_visible(l.owner_user_id.as_deref())),
        Some(sid) => {
            if scope_query_visible(sid) {
                loops.retain(|l| l.scope_id.as_deref() == Some(sid));
            } else {
                loops.clear();
            }
        }
    }

    let summaries: Vec<LoopSummary> = loops.iter().map(LoopSummary::from).collect();
    JsonRpcResponse::success(request.id, json!({ "loops": summaries }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::{Goal, GoalStore};
    use crate::looping::LoopRegistry;
    use crate::scope::{ScopeAttribution, ScopeId};
    use crate::sync_primitives::Arc;

    fn req(method: &str, params: Option<serde_json::Value>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id: Some(json!(1)),
        }
    }

    fn goal_with_scope(session_id: &str, scope: Option<(&str, &str)>) -> Goal {
        let mut g = Goal::new(session_id, "objective", 0, 1_000);
        if let Some((owner, project)) = scope {
            let attr = ScopeAttribution {
                owner_user_id: owner.to_string(),
                scope: ScopeId::Project(project.to_string()),
            };
            g = g.with_owner_scope(Some(&attr));
        }
        g
    }

    /// Returns the ACTUAL process-global [`GoalStore`] — `set_global_for_test`
    /// is `OnceLock`-once, so if another test elsewhere in this binary already
    /// installed the global first, a freshly-created store here would never
    /// become reachable via `crate::goal::global()`, and every row a caller
    /// `.put()`s into it would be invisible to `handle_list` (which only ever
    /// reads THAT global). Mirrors `gateway::handlers::users::tests::
    /// deactivate_pauses_owned_goals_and_loops`'s `unwrap_or_else` idiom:
    /// check the real global first, only create+install as the fallback.
    ///
    /// A winning install's tempdir must outlive this function — another test
    /// elsewhere in the binary, running after this one returns, may still be
    /// reading through the very store this call installed — so it is
    /// deliberately leaked with `mem::forget` rather than handed back for a
    /// caller to drop (which would delete the backing sqlite file out from
    /// under a still-live global).
    fn install_goal_store() -> Arc<GoalStore> {
        crate::goal::global().unwrap_or_else(|| {
            let dir = tempfile::tempdir().unwrap();
            crate::goal::set_global_for_test(Arc::new(
                GoalStore::open(&dir.path().join("g.db")).unwrap(),
            ));
            std::mem::forget(dir); // keep the sqlite file alive for the process
            crate::goal::global().expect("goal global set")
        })
    }

    #[tokio::test]
    async fn no_scope_filter_defaults_to_owner_only_status_quo() {
        let store = install_goal_store();
        store.put(&goal_with_scope("s-mine", None)).unwrap();

        // Unrestricted caller (no CALLER_USER task-local scoped) sees
        // everything — `stamped_owner_visible`'s `None` arm. Assert
        // PRESENCE, not an exact count: `goal::global()` is a process-wide
        // OnceLock this test shares with every other test in the same
        // `--lib` binary that touches it (see `install_goal_store`'s doc),
        // so an unrestricted list can legitimately carry rows this test
        // never created.
        let resp = handle_list(req("goal.list", None)).await;
        let body = resp.result.expect("success");
        let goals = body["goals"].as_array().unwrap();
        assert!(
            goals.iter().any(|g| g["session_id"] == "s-mine"),
            "expected our own goal to be visible to an unrestricted caller: {goals:?}"
        );
    }

    /// C7 review, Important 1: the presence-only assertion above proves an
    /// UNRESTRICTED caller sees their own row — it cannot prove a RESTRICTED
    /// caller is refused someone else's, because `stamped_owner_visible`'s
    /// `None => true` arm never runs for a scoped caller. This is the
    /// negative twin, following `group_chat.rs::
    /// list_hides_another_users_sessions_but_keeps_your_own`'s pattern: two
    /// distinctly-owned rows, one restricted caller, assert both directions.
    #[tokio::test]
    async fn no_scope_filter_denies_another_owners_goal_to_a_restricted_caller() {
        let store = install_goal_store();
        let alice = crate::scope::ScopeAttribution::personal("u-alice-kanban-nofilter");
        let bob = crate::scope::ScopeAttribution::personal("u-bob-kanban-nofilter");
        store
            .put(
                &Goal::new("s-alice-kanban-nofilter", "objective", 0, 1_000)
                    .with_owner_scope(Some(&alice)),
            )
            .unwrap();
        store
            .put(
                &Goal::new("s-bob-kanban-nofilter", "objective", 0, 1_000)
                    .with_owner_scope(Some(&bob)),
            )
            .unwrap();

        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-bob-kanban-nofilter".to_string()), async {
                handle_list(req("goal.list", None)).await
            })
            .await;

        let body = resp.result.expect("success");
        let ids: Vec<String> = body["goals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["session_id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            ids.contains(&"s-bob-kanban-nofilter".to_string()),
            "bob must see his own goal: {ids:?}"
        );
        assert!(
            !ids.contains(&"s-alice-kanban-nofilter".to_string()),
            "bob must NOT see alice's goal via the unfiltered list: {ids:?}"
        );
    }

    #[tokio::test]
    async fn a_project_scope_query_from_a_non_member_gets_an_empty_set_not_an_error() {
        let store = install_goal_store();
        store
            .put(&goal_with_scope("s-room", Some(("u-alice", "p-x"))))
            .unwrap();

        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-mallory".to_string()), async {
                handle_list(req("goal.list", Some(json!({ "scope_id": "project:p-x" })))).await
            })
            .await;

        assert!(resp.error.is_none(), "non-member is refused, not errored");
        let body = resp.result.expect("success");
        assert_eq!(body["goals"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn a_project_scope_query_from_a_member_sees_the_rooms_goal() {
        let _roster_guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = install_goal_store();
        store
            .put(&goal_with_scope("s-room2", Some(("u-alice", "p-y"))))
            .unwrap();

        crate::projects::roster::publish(crate::projects::roster::RosterSnapshot::from_pairs([
            ("p-y".to_string(), "u-alice".to_string()),
            ("p-y".to_string(), "u-bob".to_string()),
        ]));

        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_list(req("goal.list", Some(json!({ "scope_id": "project:p-y" })))).await
            })
            .await;

        let body = resp.result.expect("success");
        let goals = body["goals"].as_array().unwrap();
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0]["session_id"], "s-room2");
        assert_eq!(goals[0]["scope_id"], "project:p-y");
    }

    /// The looping twin of [`install_goal_store`] — same `OnceLock`-once
    /// race, same fix: fetch the ACTUAL process-global registry (installing
    /// one only if none exists yet) and hand callers that, never an orphaned
    /// local registry `crate::looping::global()` would never read back.
    fn install_loop_registry() -> Arc<LoopRegistry> {
        crate::looping::global().unwrap_or_else(|| {
            let reg = Arc::new(LoopRegistry::default());
            crate::looping::init_global(reg);
            crate::looping::global().expect("loop registry installed")
        })
    }

    #[tokio::test]
    async fn loop_list_mirrors_the_same_scope_rule() {
        let _roster_guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let reg = install_loop_registry();
        let mut st = LoopState::new(
            "s-loop-room",
            "watch it",
            crate::looping::Cadence::Fixed {
                interval_ms: 300_000,
            },
            1_000,
        );
        let attr = ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: ScopeId::Project("p-z".to_string()),
        };
        st = st.with_owner_scope(Some(&attr));
        reg.put(st);

        crate::projects::roster::publish(crate::projects::roster::RosterSnapshot::from_pairs([(
            "p-z".to_string(),
            "u-alice".to_string(),
        )]));

        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_loop_list(req("loop.list", Some(json!({ "scope_id": "project:p-z" })))).await
            })
            .await;

        let body = resp.result.expect("success");
        let loops = body["loops"].as_array().unwrap();
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0]["session_id"], "s-loop-room");
        assert_eq!(loops[0]["status"], "active");
    }

    /// C7 review, Important 1: the loop twin of
    /// `no_scope_filter_denies_another_owners_goal_to_a_restricted_caller`.
    #[tokio::test]
    async fn no_scope_filter_denies_another_owners_loop_to_a_restricted_caller() {
        let reg = install_loop_registry();
        let alice = ScopeAttribution::personal("u-alice-kanban-loop-nofilter");
        let bob = ScopeAttribution::personal("u-bob-kanban-loop-nofilter");

        let mut alice_st = LoopState::new(
            "s-alice-kanban-loop-nofilter",
            "watch it",
            crate::looping::Cadence::Fixed {
                interval_ms: 300_000,
            },
            1_000,
        );
        alice_st = alice_st.with_owner_scope(Some(&alice));
        reg.put(alice_st);

        let mut bob_st = LoopState::new(
            "s-bob-kanban-loop-nofilter",
            "watch it too",
            crate::looping::Cadence::Fixed {
                interval_ms: 300_000,
            },
            1_000,
        );
        bob_st = bob_st.with_owner_scope(Some(&bob));
        reg.put(bob_st);

        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-bob-kanban-loop-nofilter".to_string()), async {
                handle_loop_list(req("loop.list", None)).await
            })
            .await;

        let body = resp.result.expect("success");
        let ids: Vec<String> = body["loops"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["session_id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            ids.contains(&"s-bob-kanban-loop-nofilter".to_string()),
            "bob must see his own loop: {ids:?}"
        );
        assert!(
            !ids.contains(&"s-alice-kanban-loop-nofilter".to_string()),
            "bob must NOT see alice's loop via the unfiltered list: {ids:?}"
        );
    }

    #[tokio::test]
    async fn an_unparseable_scope_id_fails_closed_to_empty_not_an_error() {
        let store = install_goal_store();
        store.put(&goal_with_scope("s-garbage", None)).unwrap();

        let resp = handle_list(req(
            "goal.list",
            Some(json!({ "scope_id": "not-a-real-scope" })),
        ))
        .await;
        assert!(resp.error.is_none());
        let body = resp.result.expect("success");
        assert_eq!(body["goals"].as_array().unwrap().len(), 0);
    }
}
