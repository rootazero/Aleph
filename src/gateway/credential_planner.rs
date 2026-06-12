//! Gateway connection-surface planner — read-only diagnostic.
//!
//! Ported (and trimmed) from openclaw `src/gateway/credential-planner.ts`.
//! The auth dimensions (mode/token/challenge/session expiry) died with the
//! LAN-trust revert; what remains answers "what does the gateway expose and
//! to whom" — bind address, origin policy, and mutation hardening — without
//! diffing TOML by hand.
//!
//! Pure function. No I/O, no mutation, no enforcement.
//!
//! Wired into the `gateway.credentials` admin RPC; lives on the Query lane.

use serde::{Deserialize, Serialize};

use super::config::GatewayServerConfig;

/// Read-only snapshot of the gateway's connection surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialPlan {
    /// Whether `gateway.require_idempotency_key` is on (mutation hardening).
    pub require_idempotency_key: bool,
    /// Number of additional allowed WS origins (same-origin is always OK).
    pub allowed_origins_count: usize,
    /// Whether the `allow_any_origin` escape hatch is on (reverse-proxy
    /// deployments; disables the cross-origin/DNS-rebinding guard).
    pub allow_any_origin: bool,
    /// `host:port` the server is bound to. Useful for "why can't I reach
    /// the gateway from the LAN?" debugging — surfaces `127.0.0.1` vs
    /// `0.0.0.0` without exposing the underlying `SocketAddr` type.
    pub bind_address: String,
    /// WS ping interval in seconds (half-open TCP detection).
    pub ping_interval_secs: u64,
    /// Idle-disconnect timeout in seconds.
    pub idle_timeout_secs: u64,
}

/// Snapshot the connection surface from the live config.
#[must_use]
pub fn build_credential_plan(cfg: &GatewayServerConfig) -> CredentialPlan {
    CredentialPlan {
        require_idempotency_key: cfg.require_idempotency_key,
        allowed_origins_count: cfg.allowed_origins.len(),
        allow_any_origin: cfg.allow_any_origin,
        bind_address: format!("{}:{}", cfg.host, cfg.port),
        ping_interval_secs: cfg.ping_interval_secs,
        idle_timeout_secs: cfg.idle_timeout_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_origin_overrides() {
        let cfg = GatewayServerConfig::default();
        let plan = build_credential_plan(&cfg);
        assert!(!plan.require_idempotency_key);
        assert_eq!(plan.allowed_origins_count, 0);
        assert!(!plan.allow_any_origin);
        assert_eq!(plan.bind_address, "127.0.0.1:18790");
    }

    #[test]
    fn hardening_and_origin_knobs_round_trip() {
        let cfg = GatewayServerConfig {
            require_idempotency_key: true,
            allowed_origins: vec!["https://a.local".into(), "https://b.local".into()],
            allow_any_origin: true,
            ..GatewayServerConfig::default()
        };
        let plan = build_credential_plan(&cfg);
        assert!(plan.require_idempotency_key);
        assert_eq!(plan.allowed_origins_count, 2);
        assert!(plan.allow_any_origin);
    }

    #[test]
    fn plan_serializes_as_compact_json() {
        let cfg = GatewayServerConfig::default();
        let plan = build_credential_plan(&cfg);
        let json = serde_json::to_string(&plan).expect("serializes");
        // Spot-check stable keys — full schema is asserted by serde derive.
        assert!(json.contains("\"allow_any_origin\":false"));
        assert!(json.contains("\"bind_address\":\"127.0.0.1:18790\""));
    }
}
