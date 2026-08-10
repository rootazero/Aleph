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

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::router::SessionKey;
use super::super::session_store::SessionStore;
use super::super::visibility;
use super::parse_params;
use crate::gateway::ResumeReport;

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
            }),
            Self::InvalidKey => json!({ "status": "invalid_session_key" }),
            Self::NotFound => json!({ "status": "not_found" }),
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
/// The visibility gate runs before the coordinator is consulted at all, so an
/// invisible session cannot be probed for existence. Note what the gate is
/// worth on each surface: over JSON-RPC the caller's identity is scoped around
/// `process_request`, so `session_visible` compares against a real actor. Over
/// `/v1/admin` there is no such scope, `visible_owner_filter()` is `None` and
/// the gate admits everything — which is the trust model working as designed,
/// not a hole: that route is bearer-authenticated with the operator's shared
/// token, and an operator sees every session anyway. The check stays in the
/// shared body rather than being lifted into the RPC handler precisely so that
/// this reasoning lives at the gate instead of being re-derived per transport.
pub async fn resume_named_session(
    raw_key: &str,
    session_manager: &Arc<dyn SessionStore>,
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
/// # Why the agent-admission gate is not here
///
/// Every other run-start path passes through
/// `handlers::agent::build_run_request`, which asks
/// `caller_identity::caller_may_act_as_agent` (§5.17 round 5). This one does
/// not: it re-triggers an interrupted run under the session's **stored**
/// attribution, so the only question it asks is `session_visible`.
///
/// The residue is narrow and deliberate: after an operator removes someone
/// from an agent's `allowed_users`, that person can still resume a run of
/// their own that was interrupted earlier. They cannot **steer** it —
/// `ResumeParams` carries only a session key, and giving the run new
/// instructions means `chat.send`, which is gated. The work being resumed was
/// authorized when it started, and a revocation already requires a restart to
/// take effect at all (`[agents]` is not a live section).
///
/// **This reasoning expires the moment `agent.resume` accepts input.** If a
/// caller can direct the resumed run, resume becomes a run-start path in
/// substance and needs the same gate — see AGENT_IDENTITY.md §6 ①.
pub async fn handle_resume(
    request: JsonRpcRequest,
    session_manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params: ResumeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let outcome = resume_named_session(&params.session_key, &session_manager).await;
    match &outcome {
        ResumeOutcome::InvalidKey => {
            JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key format")
        }
        ResumeOutcome::NotFound => visibility::not_found_response(request.id),
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
        }
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
    }
}
