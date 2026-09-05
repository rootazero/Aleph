//! `POST /v1/admin/resume` — on-demand resume from the CLI over IPC.
//!
//! The CLI half of `agent.resume`. Both land in
//! [`crate::gateway::handlers::resume::resume_named_session`], so the gate, the
//! status vocabulary and the counters are decided once; this module is
//! transport only.
//!
//! `aleph-server resume` is deliberately **not** a `LockOrIpc` command with a
//! local fallback. Resuming a run means re-entering the harness with the
//! session's provider, tools and workspace — there is no meaningful "do it
//! locally without the server" path, and a fallback that pretended otherwise
//! would either do nothing or start a second runtime beside the singleton. When
//! no server is running the honest answer is to say so.

use aleph_protocol::resume::ResumeReceipt;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use crate::gateway::admin_api::AdminApiState;
use crate::gateway::handlers::resume::{resume_named_session, ResumeOutcome};

pub fn router() -> Router<AdminApiState> {
    Router::new().route("/", post(resume_session))
}

#[derive(Debug, Deserialize)]
pub struct ResumeRequest {
    pub session_key: String,
}

async fn resume_session(
    State(state): State<AdminApiState>,
    Json(body): Json<ResumeRequest>,
) -> Result<Json<ResumeReceipt>, (StatusCode, String)> {
    // `None` — no registry, on purpose rather than by omission. This route
    // carries no `CALLER_USER` (the task-locals are scoped around the WS
    // `process_request`, not around axum), so `caller_may_act_as_agent` would
    // be vacuously true even with a registry in hand: the gate is a no-op
    // here by construction. Threading one in would look like an enforcement
    // point that never enforces. The route's real gate is the bearer token in
    // `admin_auth_middleware`, and an operator may act as every agent —
    // the same reasoning `resume_named_session` records for `session_visible`
    // on this surface.
    let outcome = resume_named_session(&body.session_key, &state.session_store, None).await;
    match &outcome {
        ResumeOutcome::InvalidKey => Err((
            StatusCode::BAD_REQUEST,
            format!("invalid session_key: {}", body.session_key),
        )),
        ResumeOutcome::NotFound => Err((
            StatusCode::NOT_FOUND,
            format!("no such session: {}", body.session_key),
        )),
        // Unreachable from this route today — the gate above is a no-op here
        // for the reason given at the call — but rendered rather than folded
        // into another arm. Folding it would mean this transport silently
        // reported a permission verdict as a 404 the day it *did* become
        // reachable (a future scoped admin identity), and that shape is a
        // puzzle, not a refusal.
        ResumeOutcome::AgentForbidden(agent_id) => Err((
            StatusCode::FORBIDDEN,
            format!("not authorized to run as agent '{agent_id}'"),
        )),
        ResumeOutcome::Unavailable => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "resume is unavailable: this server has no run executor wired".to_string(),
        )),
        ResumeOutcome::Failed(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.clone())),
        // One constructor with the JSON-RPC face — see
        // [`crate::gateway::handlers::resume::ResumeOutcome::receipt`]. The
        // struct this route used to declare carried four of the nine counters,
        // so `delegated`, `busy`, `refused`, `skipped_unknown_age` and
        // `contradictions` were unreachable from the only surface an operator
        // actually calls.
        ResumeOutcome::Done { .. } => Ok(Json(outcome.receipt(&body.session_key))),
    }
}
