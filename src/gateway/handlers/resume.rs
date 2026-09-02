//! On-demand resume of an interrupted run.
//!
//! Surfaces:
//! - `agent.resume` (JSON-RPC, Panel / WS clients) — [`handle_resume`]
//! - `POST /v1/admin/resume` (CLI over IPC) — `admin_api::resume`
//!
//! [`crate::gateway::ResumeCoordinator`] scans for interrupted runs exactly
//! once, at boot. That covers the case it was built for (the daemon died and
//! came back) and nothing else: a run interrupted while the daemon kept
//! running, or one whose candidate was skipped because a transient store error
//! ate it, stayed interrupted forever with no way for anyone to say "pick that
//! back up". Boot was the only trigger face, so the answer here is more faces —
//! not more implementations. Both surfaces call [`resume_named_session`], which
//! calls `ResumeCoordinator::resume_session`, which shares
//! `resume_from_markers` with the boot scan. Every judgement (recency filter,
//! crash-loop cap, crash-boundary repair, concurrency permit) is made in
//! exactly one place.
//!
//! Deliberately not a builtin tool. A model cannot resume the run it is
//! currently executing — there is nothing to resume — and the surface that
//! needs this verb is the operator's. A catalog entry costs prompt bytes on
//! every request; one with no caller costs them for nothing.

use crate::sync_primitives::Arc;
use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, PERMISSION_DENIED,
};
use super::super::router::SessionKey;
use super::super::session_store::SessionStore;
use super::super::visibility;
use super::agent::BuildRunError;
use super::parse_params;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::{ResumeRefusal, ResumeReport};

/// The status word for "the reducer refused this session's log".
///
/// A string constant, not an inline literal, because three faces spell it
/// (this handler, the admin route, the CLI's match) and a fourth reads it back
/// (`aleph doctor`'s finding ids share the vocabulary). T3 moves it to
/// `shared/protocol` with the rest of the closed set; it lives here for exactly
/// as long as this handler is its only writer.
pub const LOG_INCONSISTENT: &str = "log_inconsistent";

/// Parameters for `agent.resume`.
#[derive(Debug, Clone, Deserialize)]
pub struct ResumeParams {
    /// The session whose interrupted run should be re-triggered.
    pub session_key: String,
}

/// Every way an on-demand resume can end, decided once and rendered per
/// transport.
#[derive(Debug)]
pub enum ResumeOutcome {
    /// The `session_key` string is not a session key.
    InvalidKey,
    /// The session does not exist, is unreadable, or is not visible to the
    /// caller. One variant for all three on purpose: a caller who cannot see a
    /// session must not be able to learn whether it exists.
    NotFound,
    /// The caller can see the session but is not on its agent's
    /// `allowed_users` list. Carries the agent id.
    ///
    /// Deliberately NOT folded into [`Self::NotFound`]: this arm is only
    /// reachable *after* `session_visible` passed, so the caller has already
    /// proven it may know the session exists and there is no oracle left to
    /// protect. The same ruling `BuildRunError::AgentForbidden` records for
    /// the run-start face, for the same reason — a puzzle is what pushes an
    /// operator to widen the setting.
    AgentForbidden(String),
    /// No coordinator is published — this server has no execution adapter to
    /// re-trigger runs with. Distinct from a clean zero report, which would
    /// read as "there was nothing to resume".
    Unavailable,
    /// The coordinator ran. `status` is the one-word summary; `report` carries
    /// the counters.
    Done {
        status: &'static str,
        report: ResumeReport,
    },
    /// The coordinator was reached but its marker read failed.
    Failed(String),
}

impl ResumeOutcome {
    /// The wire body for the `Done` / `Failed` cases, plus the session key the
    /// caller named so a batched caller can correlate.
    #[must_use]
    pub fn to_json(&self, session_key: &str) -> serde_json::Value {
        match self {
            Self::Done { status, report } => json!({
                "status": status,
                "session_key": session_key,
                "scanned": report.scanned,
                "resumed": report.resumed,
                "abandoned": report.abandoned,
                "skipped": report.skipped,
                "busy": report.busy,
                // Omitting this was half of the same defect as `status_of`'s:
                // even a `--json` caller could not tell "handed back to its own
                // scheduler" from "the re-trigger errored".
                "delegated": report.delegated,
                "skipped_unknown_age": report.skipped_unknown_age,
                "contradictions": report.contradictions,
                "refused": report
                    .refused
                    .iter()
                    .map(|(session, refusal)| json!({
                        "session_key": session.to_key_string(),
                        "reason": refusal.reason(),
                        "detail": refusal.detail(),
                    }))
                    .collect::<Vec<_>>(),
            }),
            Self::InvalidKey => json!({ "status": "invalid_session_key" }),
            Self::NotFound => json!({ "status": "not_found" }),
            Self::AgentForbidden(agent_id) => {
                json!({ "status": "agent_forbidden", "agent_id": agent_id })
            }
            Self::Unavailable => json!({ "status": "unavailable" }),
            Self::Failed(e) => json!({ "status": "failed", "error": e }),
        }
    }
}

/// Classify a finished [`ResumeReport`] into the one-word status callers read.
///
/// `scanned == 0` means the session has no run markers at all — it never ran
/// anything. That is an answer, not a failure, and `no_runs` is what
/// distinguishes it from `already_finished` (scanned, and its newest marker was
/// a `RunFinished`).
fn status_of(report: &ResumeReport) -> &'static str {
    // Checked first: `busy` means nothing was even looked at, so every other
    // counter is 0 and would otherwise render as `no_runs` — telling the
    // operator the session has no history at the exact moment it is being
    // resumed.
    if report.busy > 0 {
        "already_resuming"
    } else if matches!(
        report.refused.first(),
        Some((_, ResumeRefusal::LogInconsistent(_)))
    ) {
        // Checked before every verdict-shaped counter. A log the reducer
        // refused produces no `resumed`, no `abandoned` and no `skipped`, so
        // this used to fall through to `not_resumed` — "we tried and nothing
        // happened" — when the truth is "this log contradicts itself and
        // nothing was tried". The operator's next move differs: `not_resumed`
        // says retry, `log_inconsistent` says run `aleph doctor`.
        //
        // `first()`, not "any": this face resumes exactly ONE session, so it
        // has at most one refusal; the boot scan's multi-session report names
        // its refusals per session rather than through this word.
        LOG_INCONSISTENT
    } else if report.delegated > 0 {
        // A cron / heartbeat / team session: `resume_from_markers` deliberately
        // hands recovery back to the scheduler that owns it and closes the
        // dangling marker on the way out (`has_own_scheduler`). It produces
        // neither `resumed` nor `abandoned`, so without this arm it fell
        // through to `not_resumed` — whose own wording sends the operator to
        // look for a warning in the log that was never written, and whose
        // second `resume` then answers `already_finished` because the marker
        // this pass correctly closed is gone. Two disjoint outcomes were
        // fanning into one word.
        //
        // Position among the counters is not load-bearing (this face resumes
        // exactly one session, so exactly one of them can be non-zero); it sits
        // here to read in the same order as `resume_from_markers` classifies.
        "delegated"
    } else if report.resumed > 0 {
        "resumed"
    } else if report.abandoned > 0 {
        "abandoned"
    } else if report.skipped > 0 {
        "already_finished"
    } else if report.scanned > 0 {
        // Scanned and interrupted, but neither resumed nor abandoned:
        // `handle_interrupted` bailed out (boundary repair failed, or the
        // re-trigger errored). Both log a warning; the caller gets an honest
        // "nothing happened" rather than a fabricated success.
        "not_resumed"
    } else {
        "no_runs"
    }
}

/// Resolve, gate, and resume one named session. Shared by both surfaces.
///
/// # The two gates, in this order
///
/// **Visibility first**, before the coordinator is consulted at all, so an
/// invisible session cannot be probed for existence. **Admission second**,
/// because it answers a different question — see [`handle_resume`]'s doc for
/// why resume needs it and what it costs when it is missing.
///
/// The order is what lets the two refusals have different shapes: the first
/// has an existence secret to keep and answers `NotFound`; the second is only
/// reachable once that secret is already spent, so it answers honestly.
///
/// # What each gate is worth per surface
///
/// Over JSON-RPC the caller's identity is scoped around `process_request`, so
/// both gates compare against a real actor. Over `/v1/admin` there is no such
/// scope: `visible_owner_filter()` is `None` and `current_caller_user()` is
/// `None`, so **both** gates admit everything — which is the trust model
/// working as designed, not a hole. That route is bearer-authenticated with
/// the operator's shared token, and an operator both sees every session and
/// may act as every agent. The checks stay in the shared body rather than
/// being lifted into the RPC handler precisely so that this reasoning lives at
/// the gate instead of being re-derived per transport.
///
/// # Why `agents` is a required parameter and not a process-global
///
/// A second `OnceLock` beside `global_resume_coordinator` would give a third
/// resume face the gate for free — and would give it silence if the wiring
/// ever stopped setting it. A required parameter gives a compile error
/// instead, which is the stronger of the two (the same ruling
/// `build_run_request` records for its non-`Option` `agent` argument).
///
/// `None` means *this server has no registry to ask* — the Simulated-execution
/// build, which has no `AgentRegistry` at all. That is not a hole in the
/// deployment sense: the thing `allowed_users` protects is the agent's
/// `tool_permissions`, and Simulated execution runs no tools. It is passed
/// explicitly at both call sites rather than defaulted, so a new face has to
/// write down which it is.
pub async fn resume_named_session(
    raw_key: &str,
    session_manager: &Arc<dyn SessionStore>,
    agents: Option<&Arc<AgentRegistry>>,
) -> ResumeOutcome {
    let Some(session_key) = SessionKey::from_key_string(raw_key) else {
        return ResumeOutcome::InvalidKey;
    };

    // A store error is `NotFound`, not a 500: a transient failure must not be
    // usable to distinguish an existing session from a missing one.
    let meta = match session_manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) | Err(_) => return ResumeOutcome::NotFound,
    };
    if !visibility::session_visible(&meta) {
        return ResumeOutcome::NotFound;
    }

    // The agent axis. Read off the REGISTRY, not `Config.agents.list`: the
    // registry is the authority on which agent will actually run, and it is
    // where a revocation lands without a restart
    // (`AgentRegistry::set_allowed_users`). Reading config would leave
    // "registered but not in the TOML" as a bypass.
    //
    // The outer `None` — this agent is not registered at all — admits. There
    // is no admission list to enforce, and inventing a refusal for a deleted
    // agent would answer a policy question nobody asked; the resume itself
    // then reports `not_resumed` on its own terms. This is the same arm
    // `agent_admits_user` already takes for an absent list.
    if let Some(registry) = agents {
        if let Some(allowed) = registry.get_allowed_users(&meta.agent_id).await {
            if !crate::gateway::caller_identity::caller_may_act_as_agent(allowed.as_deref()) {
                return ResumeOutcome::AgentForbidden(meta.agent_id.clone());
            }
        }
    }

    let Some(coordinator) = crate::gateway::global_resume_coordinator() else {
        return ResumeOutcome::Unavailable;
    };

    match coordinator.resume_session(&session_key).await {
        Ok(report) => ResumeOutcome::Done {
            status: status_of(&report),
            report,
        },
        Err(e) => ResumeOutcome::Failed(e.to_string()),
    }
}

/// Handle `agent.resume`.
///
/// # Why the agent-admission gate IS here
///
/// Every run-start path passes through
/// `handlers::agent::build_run_request`, which asks
/// `caller_identity::caller_may_act_as_agent` (§5.17 round 5). `agent.resume`
/// is member-open (`method_admin.rs` pins it in `MEMBER_CARVE_OUTS`), so until
/// 2026-08-10 it was the one way to put an agent back to work without being
/// asked that question.
///
/// It stood on two legs, and **the load-bearing one was a bug**:
///
/// 1. *"A revocation needs a restart to take effect anyway"* — so the residue
///    was only reachable between a restart and the interrupted run ageing out.
///    That sentence stopped being true the same day `AgentRegistry::
///    set_allowed_users` landed: a revocation now binds on the next turn, and
///    the residue moved to **immediately after the revocation**. Nothing on
///    this file changed when that happened, and no test went red — the leg was
///    a fact about another module, cited from here.
/// 2. *"The resumed run cannot be steered"* — `ResumeParams` carries only a
///    session key, and giving a run new instructions means `chat.send`, which
///    is gated. Still true.
///
/// Leg 2 alone does not cover it. What `allowed_users` protects is the agent
/// axis of `tool_permissions`, and a resumed run does not replay a decided
/// transcript — it re-enters the harness and keeps **calling tools** under
/// that agent's permissions. "The work was authorized when it started" is
/// true of the work already done and says nothing about the work the next turn
/// invents. So the question resume asks is the one `method_admin.rs`'s own
/// carve-out comment already claimed it asks: *the same authorization question
/// as starting one.*
///
/// The gate lives in [`resume_named_session`], not here, so both faces derive
/// it once — see that function for the ordering, the per-surface worth of each
/// gate, and why the registry arrives as a parameter.
///
/// # What is deliberately still not asked
///
/// The run re-enters under the session's **stored** attribution, never the
/// caller's. Resume is not a way to run something as yourself; it is a way to
/// say "pick that back up", and this gate only decides whether you may say it.
pub async fn handle_resume(
    request: JsonRpcRequest,
    session_manager: Arc<dyn SessionStore>,
    agents: Option<Arc<AgentRegistry>>,
) -> JsonRpcResponse {
    let params: ResumeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let outcome =
        resume_named_session(&params.session_key, &session_manager, agents.as_ref()).await;
    match &outcome {
        ResumeOutcome::InvalidKey => {
            JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key format")
        }
        ResumeOutcome::NotFound => visibility::not_found_response(request.id),
        // The wording comes from `BuildRunError::AgentForbidden`'s `Display`
        // so the two faces of one refusal cannot drift into two sentences.
        ResumeOutcome::AgentForbidden(agent_id) => JsonRpcResponse::error(
            request.id,
            PERMISSION_DENIED,
            BuildRunError::AgentForbidden(agent_id.clone()).to_string(),
        ),
        ResumeOutcome::Unavailable => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Resume is unavailable: this server has no run executor wired.".to_string(),
        ),
        ResumeOutcome::Failed(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Resume failed: {e}"))
        }
        ResumeOutcome::Done { .. } => {
            JsonRpcResponse::success(request.id, outcome.to_json(&params.session_key))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(scanned: usize, resumed: usize, abandoned: usize, skipped: usize) -> ResumeReport {
        ResumeReport {
            scanned,
            resumed,
            abandoned,
            skipped,
            busy: 0,
            delegated: 0,
            refused: Vec::new(),
            skipped_unknown_age: 0,
            contradictions: 0,
        }
    }

    fn refused_report(refusal: ResumeRefusal) -> ResumeReport {
        ResumeReport {
            scanned: 1,
            refused: vec![(
                crate::routing::session_key::SessionKey::ephemeral("status-of"),
                refusal,
            )],
            ..report(0, 0, 0, 0)
        }
    }

    /// The refusal must be readable as itself, not as "we tried and nothing
    /// happened". Goes red if the `log_inconsistent` arm is moved below
    /// `scanned > 0` (it would then read `not_resumed`) or below `skipped`.
    #[test]
    fn a_refused_log_is_log_inconsistent_not_not_resumed() {
        let refused = refused_report(ResumeRefusal::LogInconsistent(
            crate::session::reduction::LogContradiction::OutOfOrderSlice { at_seq: 41 },
        ));
        assert_eq!(status_of(&refused), LOG_INCONSISTENT);

        // The arm it used to fall into still means what it says.
        assert_eq!(status_of(&report(1, 0, 0, 0)), "not_resumed");
    }

    /// Only the log-contradiction refusal gets the word. A missing agent or a
    /// failed re-trigger IS "we tried and nothing happened", and telling the
    /// operator to run `doctor` for it would send them to the wrong place.
    #[test]
    fn the_other_refusals_do_not_claim_an_inconsistent_log() {
        for refusal in [
            ResumeRefusal::AgentMissing,
            ResumeRefusal::BoundaryRepairFailed("append failed".into()),
            ResumeRefusal::RetriggerFailed("adapter said no".into()),
        ] {
            assert_eq!(
                status_of(&refused_report(refusal.clone())),
                "not_resumed",
                "{refusal:?} must not read as an inconsistent log"
            );
        }
    }

    /// A refusal outranks `busy`? No — `busy` still wins, because a busy pass
    /// looked at nothing at all and therefore cannot have refused anything.
    /// Pinned so the two never swap.
    #[test]
    fn a_busy_session_outranks_every_refusal() {
        let mut busy = refused_report(ResumeRefusal::LogInconsistent(
            crate::session::reduction::LogContradiction::NonMarkerInMarkerSlice { seq: 3 },
        ));
        busy.busy = 1;
        assert_eq!(status_of(&busy), "already_resuming");
    }

    /// A cron / heartbeat / team session is handed back to the scheduler that
    /// owns it (`has_own_scheduler`), which increments only `delegated` — a
    /// counter `status_of` did not read, so this fell through to `not_resumed`
    /// ("could not re-trigger it, check the server log") for an outcome that is
    /// correct and logs no warning. This helper could not even express the
    /// input, which is why no test could fail.
    #[test]
    fn a_session_handed_back_to_its_own_scheduler_is_delegated_not_not_resumed() {
        let delegated = ResumeReport {
            scanned: 1,
            delegated: 1,
            ..report(0, 0, 0, 0)
        };
        assert_eq!(status_of(&delegated), "delegated");

        // The arm it used to fall into must still mean what it says: scanned,
        // interrupted, and nothing happened for a reason that WAS logged.
        assert_eq!(status_of(&report(1, 0, 0, 0)), "not_resumed");
    }

    /// Every counter the report carries must reach the wire, or a `--json`
    /// caller cannot re-derive the status word. Derived from the struct: adding
    /// a counter without rendering it reddens here.
    #[test]
    fn to_json_renders_every_counter_the_report_carries() {
        let r = ResumeReport {
            scanned: 1,
            resumed: 2,
            abandoned: 3,
            skipped: 4,
            busy: 5,
            delegated: 6,
            skipped_unknown_age: 7,
            contradictions: 8,
            refused: vec![(
                crate::routing::session_key::SessionKey::ephemeral("wire"),
                ResumeRefusal::AgentMissing,
            )],
        };
        let body = ResumeOutcome::Done {
            status: "delegated",
            report: r,
        }
        .to_json("agent:main:cron:daily");
        let obj = body.as_object().expect("object");
        // `status` + `session_key` + one key per counter. The count is derived
        // from a destructure so a new field cannot be forgotten silently.
        let ResumeReport {
            scanned,
            resumed,
            abandoned,
            skipped,
            busy,
            delegated,
            skipped_unknown_age,
            contradictions,
            refused,
        } = ResumeReport::default();
        let fields = [
            scanned,
            resumed,
            abandoned,
            skipped,
            busy,
            delegated,
            skipped_unknown_age,
            contradictions,
        ]
        .len()
            // `refused` is a list, not a counter, so it cannot join the array
            // above — but the destructure is exhaustive (no `..`), so a new
            // field still fails to compile here rather than going unrendered.
            + [refused.len()].len();
        assert_eq!(
            obj.len(),
            fields + 2,
            "a field on `ResumeReport` is not reaching the wire: {obj:?}"
        );
        assert_eq!(obj["delegated"], serde_json::json!(6));
        assert_eq!(obj["skipped_unknown_age"], serde_json::json!(7));
        assert_eq!(obj["contradictions"], serde_json::json!(8));
        assert_eq!(
            obj["refused"][0]["reason"],
            serde_json::json!("agent_missing"),
            "a refusal must reach the wire as its own word: {obj:?}"
        );
    }

    #[test]
    fn a_session_that_never_ran_is_no_runs_not_already_finished() {
        // The distinction the caller acts on: `no_runs` means "you named a
        // session with no history", `already_finished` means "its last run
        // completed". Collapsing them would make a typo'd key look like a
        // successful no-op.
        assert_eq!(status_of(&report(0, 0, 0, 0)), "no_runs");
        assert_eq!(status_of(&report(1, 0, 0, 1)), "already_finished");
    }

    #[test]
    fn a_scanned_candidate_that_did_nothing_is_not_reported_as_success() {
        // Interrupted, neither resumed nor abandoned — the boundary repair or
        // the re-trigger failed. Reporting "resumed" here would be a lie the
        // operator only discovers by waiting for output that never comes.
        assert_eq!(status_of(&report(1, 0, 0, 0)), "not_resumed");
    }

    /// A concurrent resume leaves every other counter at 0, so without its own
    /// arm it would render as `no_runs` — "this session has no history", said
    /// about a session that is being resumed right now.
    #[test]
    fn a_concurrent_resume_reports_already_resuming_not_no_runs() {
        let busy = ResumeReport {
            busy: 1,
            ..Default::default()
        };
        assert_eq!(status_of(&busy), "already_resuming");
    }

    #[test]
    fn resumed_and_abandoned_outrank_the_quiet_outcomes() {
        assert_eq!(status_of(&report(1, 1, 0, 0)), "resumed");
        assert_eq!(status_of(&report(1, 0, 1, 0)), "abandoned");
    }

    #[test]
    fn non_done_outcomes_render_their_own_status() {
        assert_eq!(
            ResumeOutcome::Unavailable.to_json("s")["status"],
            "unavailable"
        );
        assert_eq!(ResumeOutcome::NotFound.to_json("s")["status"], "not_found");
        assert_eq!(
            ResumeOutcome::InvalidKey.to_json("s")["status"],
            "invalid_session_key"
        );
        let forbidden = ResumeOutcome::AgentForbidden("ops".to_string()).to_json("s");
        assert_eq!(forbidden["status"], "agent_forbidden");
        assert_eq!(
            forbidden["agent_id"], "ops",
            "a refusal that does not name the agent leaves the operator guessing \
             which `allowed_users` list to look at"
        );
    }

    // ─── The agent-admission gate ────────────────────────────────────────────
    //
    // These go through `resume_named_session` itself, under the same task-local
    // nesting a real dispatch applies (`server::handler::
    // dispatch_with_caller_context`), because the gate's whole subject is what
    // those task-locals hold — calling `caller_may_act_as_agent` directly would
    // test `agent_admits_user`, which already has its own tests, and would stay
    // green if this file stopped calling it.

    use crate::gateway::agent_instance::{AgentInstanceConfig, AgentRegistry};
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::gateway::session_store::SessionStore;
    use crate::scope::{with_scope, ScopeAttribution};
    use tempfile::TempDir;

    /// Mirrors `isolation_acceptance::as_caller` — the P1 scope attribution
    /// wrapping the P0 identity, both seeded from one caller id, exactly as
    /// `dispatch_with_caller_context` nests them.
    async fn as_caller<F, T>(user: &str, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        with_scope(
            Some(ScopeAttribution::personal(user)),
            CALLER_USER.scope(Some(user.to_string()), fut),
        )
        .await
    }

    /// A session owned by `owner`, on an agent named after the test, plus a
    /// registry holding that agent with `allowed` as its admission list.
    ///
    /// The agent id carries a uuid so two tests in one process cannot collide
    /// on the shared session-key namespace.
    async fn fixture(
        owner: &str,
        allowed: Option<Vec<String>>,
    ) -> (TempDir, Arc<dyn SessionStore>, Arc<AgentRegistry>, String) {
        let temp = TempDir::new().unwrap();
        let sessions: Arc<dyn SessionStore> = Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("sessions.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let agent_id = format!("ops-{}", uuid::Uuid::new_v4().simple());
        let key = SessionKey::main(agent_id.clone());
        as_caller(owner, async {
            sessions.get_or_create(&key).await.unwrap();
        })
        .await;

        let registry = Arc::new(AgentRegistry::new());
        registry
            .register_config(
                AgentInstanceConfig {
                    agent_id: agent_id.clone(),
                    allowed_users: allowed,
                    ..Default::default()
                },
                sessions.clone(),
            )
            .await;
        (temp, sessions, registry, key.to_key_string())
    }

    /// The residue this gate was added for, stated as the scenario that
    /// produces it: an operator removes Alice from `ops`'s `allowed_users`,
    /// and Alice reaches for the run of her own that was interrupted earlier.
    ///
    /// She still passes `session_visible` — it is her session — which is
    /// precisely why the visibility gate could never have covered this. Before
    /// 2026-08-10 the answer here was a resumed run under an agent whose
    /// permissions she no longer holds.
    #[tokio::test]
    async fn a_revoked_user_cannot_resume_their_own_interrupted_run() {
        let (_t, sessions, registry, key) =
            fixture("u-alice", Some(vec!["u-bob".to_string()])).await;

        let outcome = as_caller(
            "u-alice",
            resume_named_session(&key, &sessions, Some(&registry)),
        )
        .await;

        assert!(
            matches!(outcome, ResumeOutcome::AgentForbidden(_)),
            "a revoked caller must be refused, not resumed; got {outcome:?}"
        );
    }

    /// …and the same call by someone still on the list goes through. Without
    /// this the test above would also pass if the gate refused everybody.
    ///
    /// `Unavailable` is what "both gates admitted, the coordinator was asked"
    /// looks like in a test process: `set_global_resume_coordinator` has
    /// exactly one caller and it is in the `aleph-server` binary, so no test in
    /// this crate can publish one.
    #[tokio::test]
    async fn a_still_admitted_user_reaches_the_coordinator() {
        let (_t, sessions, registry, key) =
            fixture("u-alice", Some(vec!["u-alice".to_string()])).await;

        let outcome = as_caller(
            "u-alice",
            resume_named_session(&key, &sessions, Some(&registry)),
        )
        .await;

        assert!(
            matches!(outcome, ResumeOutcome::Unavailable),
            "an admitted caller must get past the gate; got {outcome:?}"
        );
    }

    /// An agent the registry does not know has no admission list to enforce,
    /// so it admits — the same arm `agent_admits_user` takes for an absent
    /// list. Refusing here would invent a policy about deleted agents and
    /// would answer "you are not allowed" to what is really "that agent is
    /// gone".
    #[tokio::test]
    async fn an_unregistered_agent_has_no_list_to_enforce() {
        let (_t, sessions, _registry, key) = fixture("u-alice", None).await;
        let empty = Arc::new(AgentRegistry::new());

        let outcome = as_caller(
            "u-alice",
            resume_named_session(&key, &sessions, Some(&empty)),
        )
        .await;

        assert!(
            matches!(outcome, ResumeOutcome::Unavailable),
            "an unknown agent must not be turned into a refusal; got {outcome:?}"
        );
    }

    /// `agents: None` is "this server has no registry to ask", not "refuse".
    /// Pinned because the honest-looking alternative — fail closed when the
    /// gate cannot run — would take resume away from the Simulated build
    /// entirely, and the thing the list protects (an agent's
    /// `tool_permissions`) does not exist there to protect.
    #[tokio::test]
    async fn no_registry_means_the_gate_cannot_run_not_that_it_refuses() {
        let (_t, sessions, _registry, key) =
            fixture("u-alice", Some(vec!["u-bob".to_string()])).await;

        let outcome = as_caller("u-alice", resume_named_session(&key, &sessions, None)).await;

        assert!(
            matches!(outcome, ResumeOutcome::Unavailable),
            "a missing registry must not read as a refusal; got {outcome:?}"
        );
    }

    /// The `/v1/admin` surface, reproduced: no `CALLER_USER` scope at all.
    /// Both gates admit — that is the trust model, and this pins that the
    /// admission gate did not accidentally become the one predicate in this
    /// file that fails closed on an unscoped process (which would take cron,
    /// heartbeat and the CLI's `aleph-server resume` down with it).
    #[tokio::test]
    async fn an_unscoped_process_is_admitted_by_both_gates() {
        let (_t, sessions, registry, key) =
            fixture("u-alice", Some(vec!["u-bob".to_string()])).await;

        // Deliberately NOT wrapped in `as_caller`.
        let outcome = resume_named_session(&key, &sessions, Some(&registry)).await;

        assert!(
            matches!(outcome, ResumeOutcome::Unavailable),
            "an unscoped caller must be admitted; got {outcome:?}"
        );
    }

    /// One wording, two faces. The RPC handler renders its refusal through
    /// `BuildRunError::AgentForbidden`'s `Display` so `agent.resume` and
    /// `chat.send` cannot answer the same verdict with two sentences.
    #[test]
    fn the_refusal_wording_has_one_source() {
        let from_run_start = BuildRunError::AgentForbidden("ops".to_string()).to_string();
        assert!(
            from_run_start.contains("allowed_users"),
            "the shared sentence must name the setting the operator has to edit"
        );
    }
}
