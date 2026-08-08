//! HTTP probe endpoints for liveness (`/health`) and readiness (`/ready`).
//!
//! - `/health` always returns 200 OK with version + instance id + uptime.
//!   Used as a k8s liveness probe / reverse-proxy upstream check.
//! - `/ready` returns 200 once boot phase-2 completes, 503 before that
//!   (and during graceful shutdown). Used as a k8s readiness probe.

use crate::sync_primitives::{Arc, Ordering};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde_json::json;

use super::GatewaySharedState;

/// `GET /health` — always 200 OK while the process is alive. The body
/// gives the version + per-process `instance_id` + uptime so operators can
/// distinguish processes across restarts.
pub async fn handle_health(State(state): State<Arc<GatewaySharedState>>) -> impl IntoResponse {
    let uptime_secs = (chrono::Utc::now().timestamp() - state.started_at_unix).max(0);
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "version": env!("ALEPH_VERSION"),
            "instance_id": state.instance_id,
            "started_at_unix": state.started_at_unix,
            "uptime_secs": uptime_secs,
        })),
    )
}

/// `GET /ready` — 200 OK once boot phase-2 has flipped the ready flag,
/// 503 `SERVICE_UNAVAILABLE` before that or during shutdown. The body
/// always includes the current phase for diagnostic visibility.
pub async fn handle_ready(State(state): State<Arc<GatewaySharedState>>) -> impl IntoResponse {
    let ready = state.ready.load(Ordering::Acquire);
    let (status, phase) = if ready {
        (StatusCode::OK, "complete")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "booting")
    };
    (
        status,
        Json(json!({
            "ready": ready,
            "phase": phase,
            "version": env!("ALEPH_VERSION"),
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::event_scope::EventScopeGuard;
    use crate::gateway::handlers::events::SubscriptionManager;
    use crate::gateway::handlers::HandlerRegistry;
    use crate::gateway::idempotency::IdempotencyGuard;
    use crate::gateway::lane::{LaneConfig, LaneManager};
    use crate::gateway::middleware::MiddlewareChain;
    use crate::gateway::presence::PresenceTracker;
    use crate::gateway::rate_limiter::{RateLimitConfig, RateLimiter};
    use crate::gateway::state_version::StateVersionTracker;
    use crate::sync_primitives::AtomicBool;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::sync::RwLock;

    fn make_shared(ready: bool) -> Arc<GatewaySharedState> {
        let handlers = Arc::new(HandlerRegistry::new());
        let rate_limiter = Arc::new(RateLimiter::new(RateLimitConfig::default()));
        Arc::new(GatewaySharedState {
            handlers: handlers.clone(),
            event_bus: Arc::new(GatewayEventBus::new()),
            connections: Arc::new(RwLock::new(HashMap::new())),
            subscription_manager: Arc::new(SubscriptionManager::new()),
            max_connections: 1000,
            max_connections_per_ip: 64,
            presence: Arc::new(PresenceTracker::new()),
            state_versions: Arc::new(StateVersionTracker::new()),
            rate_limiter: rate_limiter.clone(),
            lane_manager: Arc::new(LaneManager::new(LaneConfig::default())),
            idempotency_guard: Arc::new(IdempotencyGuard::new(Duration::from_secs(300))),
            event_scope_guard: Arc::new(EventScopeGuard::default_rules()),
            audit_log: None,
            trusted_proxy_enabled: false,
            trusted_proxy_ips: Vec::new(),
            allow_insecure_remote: false,
            tls_enabled: false,
            ready: Arc::new(AtomicBool::new(ready)),
            instance_id: "test-instance".to_string(),
            started_at_unix: chrono::Utc::now().timestamp(),
            ping_interval_secs: 30,
            idle_timeout_secs: 90,
            require_idempotency_key: false,
            shared_token_mgr: None,
            device_token_mgr: None,
            security_store: None,
            middleware_chain: MiddlewareChain::new(handlers, rate_limiter),
            origin_policy: Arc::new(crate::gateway::origin_policy::OriginPolicy::loopback_only()),
            node_registry: Arc::new(crate::cluster::NodeRegistry::new()),
            exec_approval_manager: None,
            session_store: None,
            team_store: None,
            event_visibility: Arc::new(
                crate::gateway::event_visibility::EventVisibilityIndex::new(),
            ),
        })
    }

    #[tokio::test]
    async fn health_returns_ok_when_not_ready() {
        let state = make_shared(false);
        let resp = handle_health(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_returns_503_before_flag_flip() {
        let state = make_shared(false);
        let resp = handle_ready(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn ready_returns_ok_after_flag_flip() {
        let state = make_shared(true);
        let resp = handle_ready(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
