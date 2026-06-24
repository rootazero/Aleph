//! Typed connection-failure classification.
//!
//! Collapses the opaque `connection_error: String` into a value that drives
//! UI copy, retry policy, and the lite-shell handoff. Pure + host-testable —
//! no wasm, no Leptos. Browsers report almost every WebSocket failure as
//! close code 1006, so classification keys off *which stage* failed plus the
//! `needs_token` verdict and known close reasons (e.g. `token_rotated`),
//! never on the close code alone.

/// Why a connection attempt or live connection ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionFailure {
    /// WS never opened / TCP unreachable / DNS failure → "check network/address".
    Unreachable { detail: String },
    /// WS opened but the server went silent / an RPC timed out.
    Timeout { detail: String },
    /// `connect` RPC reported `needs_token`, or the server closed us with
    /// `token_rotated` → re-enter the Gateway token (login wall).
    AuthRequired,
    /// A previously-healthy connection dropped → transient, auto-reconnect.
    Dropped { detail: String },
    /// Anything we can't place — surface the raw detail verbatim.
    Unknown { detail: String },
}

/// The point in the connect lifecycle a failure surfaced at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureStage {
    /// Before the WebSocket reached OPEN (connect()/open timeout).
    BeforeOpen,
    /// After OPEN — a live socket dropped.
    AfterOpen,
    /// During the `connect` handshake RPC (transport-level error).
    Handshake,
    /// An RPC exceeded its timeout without the socket closing.
    RpcTimeout,
}

/// Pure classification. `close_reason` is the WS close reason (or transport
/// error string) when available; `needs_token` is the handshake verdict.
#[must_use]
pub fn classify(
    stage: FailureStage,
    close_reason: Option<&str>,
    needs_token: bool,
) -> ConnectionFailure {
    if needs_token {
        return ConnectionFailure::AuthRequired;
    }
    if matches!(close_reason, Some(r) if r.contains("token_rotated")) {
        return ConnectionFailure::AuthRequired;
    }
    let detail = close_reason.unwrap_or_default().to_string();
    match stage {
        FailureStage::BeforeOpen | FailureStage::Handshake => {
            ConnectionFailure::Unreachable { detail }
        }
        FailureStage::RpcTimeout => ConnectionFailure::Timeout { detail },
        FailureStage::AfterOpen => ConnectionFailure::Dropped { detail },
    }
}

impl ConnectionFailure {
    /// Whether the reconnect loop should keep retrying this failure.
    /// `AuthRequired` is terminal-for-now: retrying the same bad token is wasted.
    #[must_use]
    pub const fn should_retry(&self) -> bool {
        !matches!(self, Self::AuthRequired)
    }

    /// Stable suffix used to build the i18n key for this failure's copy.
    #[must_use]
    pub const fn i18n_key(&self) -> &'static str {
        match self {
            Self::Unreachable { .. } => "unreachable",
            Self::Timeout { .. } => "timeout",
            Self::AuthRequired => "auth_required",
            Self::Dropped { .. } => "dropped",
            Self::Unknown { .. } => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_token_is_auth_required_regardless_of_stage() {
        assert_eq!(
            classify(FailureStage::Handshake, None, true),
            ConnectionFailure::AuthRequired
        );
        assert_eq!(
            classify(FailureStage::AfterOpen, Some("whatever"), true),
            ConnectionFailure::AuthRequired
        );
    }

    #[test]
    fn token_rotated_close_is_auth_required() {
        assert_eq!(
            classify(FailureStage::AfterOpen, Some("token_rotated"), false),
            ConnectionFailure::AuthRequired
        );
    }

    #[test]
    fn before_open_failure_is_unreachable() {
        assert_eq!(
            classify(FailureStage::BeforeOpen, None, false),
            ConnectionFailure::Unreachable {
                detail: String::new()
            }
        );
    }

    #[test]
    fn rpc_timeout_stage_is_timeout() {
        assert_eq!(
            classify(FailureStage::RpcTimeout, None, false),
            ConnectionFailure::Timeout {
                detail: String::new()
            }
        );
    }

    #[test]
    fn after_open_drop_is_dropped() {
        assert_eq!(
            classify(
                FailureStage::AfterOpen,
                Some("WebSocket closed: code=1006 reason="),
                false
            ),
            ConnectionFailure::Dropped {
                detail: "WebSocket closed: code=1006 reason=".to_string()
            }
        );
    }

    #[test]
    fn auth_required_does_not_retry_others_do() {
        assert!(!ConnectionFailure::AuthRequired.should_retry());
        assert!(ConnectionFailure::Unreachable {
            detail: String::new()
        }
        .should_retry());
        assert!(ConnectionFailure::Timeout {
            detail: String::new()
        }
        .should_retry());
        assert!(ConnectionFailure::Dropped {
            detail: String::new()
        }
        .should_retry());
    }

    #[test]
    fn i18n_keys_are_stable() {
        assert_eq!(
            ConnectionFailure::Unreachable {
                detail: String::new()
            }
            .i18n_key(),
            "unreachable"
        );
        assert_eq!(
            ConnectionFailure::Timeout {
                detail: String::new()
            }
            .i18n_key(),
            "timeout"
        );
        assert_eq!(ConnectionFailure::AuthRequired.i18n_key(), "auth_required");
        assert_eq!(
            ConnectionFailure::Dropped {
                detail: String::new()
            }
            .i18n_key(),
            "dropped"
        );
        assert_eq!(
            ConnectionFailure::Unknown {
                detail: String::new()
            }
            .i18n_key(),
            "unknown"
        );
    }
}
