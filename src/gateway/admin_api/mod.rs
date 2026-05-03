//! `/v1/admin/*` namespace — IPC entry points for CLI commands while
//! the server holds the singleton lock.
//!
//! Mounted on the gateway router under `/v1/admin`; bearer-auth is
//! enforced upstream by the existing OpenAI-compat router (see
//! `gateway::server::build_router`). Spec C scope covers secrets and
//! agents (memory writes go through the existing `remember` tool).

pub mod agents;
pub mod secrets;

use std::sync::Arc;

use axum::Router;

use crate::config::agent_manager::AgentManager;
use crate::gateway::security::SharedTokenManager;

#[derive(Clone)]
pub struct AdminApiState {
    pub shared_token: Arc<SharedTokenManager>,
    pub agent_manager: Arc<AgentManager>,
}

pub fn router(state: AdminApiState) -> Router {
    Router::new()
        .nest("/secrets", secrets::router())
        .nest("/agents", agents::router())
        .with_state(state)
}
