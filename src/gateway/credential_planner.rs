//! Gateway credential surface planner — read-only diagnostic.
//!
//! Ported (and trimmed) from openclaw `src/gateway/credential-planner.ts`.
//! This module's *only* job is to expose **what the gateway sees** so ops
//! can diagnose "why is the token I just exported not taking effect"
//! without diffing TOML by hand.
//!
//! Pure function. No I/O beyond the env-reader closure the caller passes
//! in (so tests can isolate). No mutation. No enforcement.
//!
//! Wired into the `gateway.credentials` admin RPC; lives on the Query lane.

use serde::{Deserialize, Serialize};

use super::config::{AuthMode, GatewayServerConfig};

/// Environment variable Aleph honours for a bearer-token override at boot.
///
/// Mirrors the documented contract in `docs/reference/GATEWAY.md`; checked
/// by the credential planner only — actual loading happens inside
/// `aleph-server::start` (see [`SharedTokenManager`](super::security::SharedTokenManager)).
pub const ENV_GATEWAY_TOKEN: &str = "ALEPH_GATEWAY_TOKEN";

/// Same idea for the panel session secret (used to sign cookies).
pub const ENV_SESSION_SECRET: &str = "ALEPH_SESSION_SECRET";

/// Read-only snapshot of the gateway's authentication surface.
///
/// Field naming mirrors the openclaw planner so cross-project ops tooling
/// can consume both with the same dashboard schema where the field exists
/// in both worlds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialPlan {
    /// `"token"` or `"none"` — see [`AuthMode`].
    pub auth_mode: String,
    /// True when `ALEPH_GATEWAY_TOKEN` was set at planner-build time.
    pub has_env_token: bool,
    /// True when `ALEPH_SESSION_SECRET` was set at planner-build time.
    pub has_env_session_secret: bool,
    /// Whether the running configuration would require a bearer token from
    /// incoming `connect` calls. Mirrors [`AuthConfig::is_auth_required`].
    pub auth_required: bool,
    /// Whether `gateway.require_challenge` is on (replay-resistant connect).
    pub require_challenge: bool,
    /// Whether `gateway.require_idempotency_key` is on (mutation hardening).
    pub require_idempotency_key: bool,
    /// Number of additional allowed WS origins (same-origin is always OK).
    pub allowed_origins_count: usize,
    /// HTTP session cookie expiry in hours (panel sign-in).
    pub session_expiry_hours: u64,
    /// Device pairing token expiry in hours.
    pub token_expiry_hours: u64,
    /// `host:port` the server is bound to. Useful for "why can't I reach
    /// the gateway from the LAN?" debugging — surfaces `127.0.0.1` vs
    /// `0.0.0.0` without exposing the underlying `SocketAddr` type.
    pub bind_address: String,
    /// WS ping interval in seconds (half-open TCP detection).
    pub ping_interval_secs: u64,
    /// Idle-disconnect timeout in seconds.
    pub idle_timeout_secs: u64,
}

/// Snapshot the credential surface using `env` as the environment reader.
///
/// Pure: `env` is the only side-effect surface, and it is called at most
/// once per env var name. Pass `|name| std::env::var(name).ok()` in prod.
pub fn build_credential_plan<F>(cfg: &GatewayServerConfig, env: F) -> CredentialPlan
where
    F: Fn(&str) -> Option<String>,
{
    let has_env_token = env(ENV_GATEWAY_TOKEN)
        .is_some_and(|v| !v.trim().is_empty());
    let has_env_session_secret = env(ENV_SESSION_SECRET)
        .is_some_and(|v| !v.trim().is_empty());
    CredentialPlan {
        auth_mode: auth_mode_label(&cfg.auth.mode),
        has_env_token,
        has_env_session_secret,
        auth_required: cfg.auth.is_auth_required(),
        require_challenge: cfg.require_challenge,
        require_idempotency_key: cfg.require_idempotency_key,
        allowed_origins_count: cfg.auth.allowed_origins.len(),
        session_expiry_hours: cfg.auth.session_expiry_hours,
        token_expiry_hours: cfg.auth.token_expiry_hours,
        bind_address: format!("{}:{}", cfg.host, cfg.port),
        ping_interval_secs: cfg.ping_interval_secs,
        idle_timeout_secs: cfg.idle_timeout_secs,
    }
}

fn auth_mode_label(mode: &AuthMode) -> String {
    match mode {
        AuthMode::Token => "token".to_string(),
        AuthMode::None => "none".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::config::AuthConfig;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_have_none_auth_no_env_overrides() {
        let cfg = GatewayServerConfig::default();
        let plan = build_credential_plan(&cfg, no_env);
        assert_eq!(plan.auth_mode, "none");
        assert!(!plan.auth_required);
        assert!(!plan.has_env_token);
        assert!(!plan.has_env_session_secret);
        assert!(!plan.require_challenge);
        assert!(!plan.require_idempotency_key);
        assert_eq!(plan.allowed_origins_count, 0);
        assert_eq!(plan.bind_address, "127.0.0.1:18790");
    }

    #[test]
    fn env_token_is_detected() {
        let cfg = GatewayServerConfig::default();
        let plan = build_credential_plan(&cfg, |name| match name {
            ENV_GATEWAY_TOKEN => Some("secret-xyz".to_string()),
            _ => None,
        });
        assert!(plan.has_env_token);
        assert!(!plan.has_env_session_secret);
    }

    #[test]
    fn empty_env_token_is_treated_as_unset() {
        let cfg = GatewayServerConfig::default();
        let plan = build_credential_plan(&cfg, |name| match name {
            ENV_GATEWAY_TOKEN => Some("   ".to_string()),
            _ => None,
        });
        assert!(!plan.has_env_token);
    }

    #[test]
    fn auth_none_drops_required_flag() {
        let cfg = GatewayServerConfig {
            auth: AuthConfig {
                mode: AuthMode::None,
                ..AuthConfig::default()
            },
            ..GatewayServerConfig::default()
        };
        let plan = build_credential_plan(&cfg, no_env);
        assert_eq!(plan.auth_mode, "none");
        assert!(!plan.auth_required);
    }

    #[test]
    fn hardening_flags_round_trip() {
        let cfg = GatewayServerConfig {
            require_challenge: true,
            require_idempotency_key: true,
            auth: AuthConfig {
                allowed_origins: vec!["https://a.local".into(), "https://b.local".into()],
                ..AuthConfig::default()
            },
            ..GatewayServerConfig::default()
        };
        let plan = build_credential_plan(&cfg, no_env);
        assert!(plan.require_challenge);
        assert!(plan.require_idempotency_key);
        assert_eq!(plan.allowed_origins_count, 2);
    }

    #[test]
    fn plan_serializes_as_compact_json() {
        let cfg = GatewayServerConfig::default();
        let plan = build_credential_plan(&cfg, no_env);
        let json = serde_json::to_string(&plan).expect("serializes");
        // Spot-check a stable key — full schema is asserted by serde derive.
        assert!(json.contains("\"auth_mode\":\"none\""));
        assert!(json.contains("\"bind_address\":\"127.0.0.1:18790\""));
    }
}
