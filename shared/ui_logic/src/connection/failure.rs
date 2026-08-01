//! Typed connection-failure classification.
//!
//! Collapses the opaque `connection_error: String` into a value that drives
//! UI copy, retry policy, and the lite-shell handoff. Pure + host-testable —
//! no wasm, no Leptos. Browsers report almost every WebSocket failure as
//! close code 1006, so classification keys off *which stage* failed plus the
//! `needs_token` verdict and known close reasons ([`AUTH_KICK_REASONS`]),
//! never on the close code alone.

use super::connector::ConnectionError;

/// Why a connection attempt or live connection ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionFailure {
    /// WS never opened / TCP unreachable / DNS failure → "check network/address".
    Unreachable { detail: String },
    /// WS opened but the server went silent / an RPC timed out.
    Timeout { detail: String },
    /// Something answered the upgrade and refused it (gateway origin gate,
    /// TLS gate, connection cap, or a proxy). The server is *up and
    /// reachable* — the remedy is configuration, not connectivity, so this
    /// must never be collapsed into [`Self::Unreachable`].
    Rejected { detail: String },
    /// `connect` RPC reported `needs_token`, or the server closed us with an
    /// auth kick ([`AUTH_KICK_REASONS`]) → re-authorize at the login wall.
    AuthRequired,
    /// A previously-healthy connection dropped → transient, auto-reconnect.
    Dropped { detail: String },
    /// Anything we can't place — surface the raw detail verbatim.
    Unknown { detail: String },
}

/// The point in the connect lifecycle a failure surfaced at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureStage {
    /// Before the WebSocket reached OPEN with no answer at all (open timeout,
    /// TCP/DNS failure).
    BeforeOpen,
    /// Before the WebSocket reached OPEN, but the peer answered and refused.
    Rejected,
    /// After OPEN — a live socket dropped.
    AfterOpen,
    /// During the `connect` handshake RPC (transport-level error).
    Handshake,
    /// An RPC exceeded its timeout without the socket closing.
    RpcTimeout,
}

/// Close reasons the gateway sends when it kicks a socket for an *auth* reason
/// rather than a transport one. Both mean the credential this Panel connected
/// with is already dead server-side, so the reconnect loop must route straight
/// to the login wall instead of spending its backoff budget re-presenting it.
///
/// Every new auth-kick close reason must be added here. Miss one and the kick
/// degrades into an ordinary `Dropped`: the Panel spends a backoff delay and a
/// doomed reconnect re-presenting the dead credential before the handshake
/// walls it, and — because the short-circuit is also where the wall learns the
/// credential was *rejected* rather than absent — the login wall greets the
/// user with first-run copy instead of saying what happened.
const AUTH_KICK_REASONS: [&str; 2] = ["token_rotated", "device_revoked"];

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
    if matches!(close_reason, Some(r) if AUTH_KICK_REASONS.iter().any(|k| r.contains(k))) {
        return ConnectionFailure::AuthRequired;
    }
    let detail = close_reason.unwrap_or_default().to_string();
    match stage {
        FailureStage::BeforeOpen | FailureStage::Handshake => {
            ConnectionFailure::Unreachable { detail }
        }
        FailureStage::Rejected => ConnectionFailure::Rejected { detail },
        FailureStage::RpcTimeout => ConnectionFailure::Timeout { detail },
        FailureStage::AfterOpen => ConnectionFailure::Dropped { detail },
    }
}

/// What an independent probe of the page's own origin found after a WebSocket
/// failed before OPEN.
///
/// Needed because the browser reports "TCP connection refused" and "the server
/// answered the upgrade with 403" *identically* — `error` then `close(1006)`
/// with an empty reason, the HTTP status deliberately withheld from script. So
/// the socket alone can only prove "something happened fast"; whether a server
/// exists at all is a separate question that only a second request can answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginLiveness {
    /// The origin answered an ordinary HTTP request (any status — even 404
    /// proves a server is there). A pre-open socket failure against a live
    /// origin is a genuine refusal.
    Serving,
    /// The probe got no answer: nothing is listening, or the host is gone.
    Silent,
    /// No probe ran (not applicable, or it could not be issued). Never claim
    /// liveness we did not establish.
    Unknown,
}

/// Which stage a failed `connect()` belongs to. Exists so the distinction the
/// connector worked to observe — "refused" vs "no answer" — survives the trip
/// to the UI instead of being flattened back into one message.
///
/// A refusal is only reported as [`FailureStage::Rejected`] when the origin is
/// confirmed [`OriginLiveness::Serving`]; otherwise it degrades to
/// `BeforeOpen`, because the copy for `Rejected` tells the operator their
/// server is up and the problem is configuration — a claim that is worse than
/// useless when nothing is running.
#[must_use]
pub const fn stage_for_connect_error(
    err: &ConnectionError,
    origin: OriginLiveness,
) -> FailureStage {
    match (err, origin) {
        (ConnectionError::FailedBeforeOpen(_), OriginLiveness::Serving) => FailureStage::Rejected,
        _ => FailureStage::BeforeOpen,
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
            Self::Rejected { .. } => "rejected",
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
    fn every_auth_kick_reason_is_auth_required() {
        // Driven off the constant so a newly-added kick reason cannot ship
        // unclassified. The failure mode is silent — the Panel still reaches
        // the wall eventually, just slower and with the wrong copy — so
        // nothing else would catch it.
        for reason in AUTH_KICK_REASONS {
            assert_eq!(
                classify(FailureStage::AfterOpen, Some(reason), false),
                ConnectionFailure::AuthRequired,
                "{reason} must route straight to the login wall"
            );
        }
    }

    #[test]
    fn an_ordinary_drop_is_not_an_auth_kick() {
        // Guards the `contains` match from widening into "any close reason".
        assert_eq!(
            classify(FailureStage::AfterOpen, Some("going away"), false),
            ConnectionFailure::Dropped {
                detail: "going away".to_string()
            }
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
    fn a_refused_handshake_is_not_reported_as_unreachable() {
        // The bug this variant exists for. A gateway that answers the upgrade
        // with 403 (`origin not allowed`) / 426 / 503 is *running and
        // reachable* — reporting it as `Unreachable` sends the operator to
        // check DNS and firewalls while the actual answer is one config key.
        let refused = classify(
            FailureStage::Rejected,
            Some("WebSocket closed before open: code=1006 reason="),
            false,
        );
        assert!(
            !matches!(refused, ConnectionFailure::Unreachable { .. }),
            "a server that actively refused the upgrade must not read as unreachable"
        );
        assert!(
            !matches!(refused, ConnectionFailure::Timeout { .. }),
            "an immediate refusal must not read as a timeout"
        );
        assert_eq!(
            refused,
            ConnectionFailure::Rejected {
                detail: "WebSocket closed before open: code=1006 reason=".to_string()
            }
        );
    }

    #[test]
    fn a_refused_handshake_still_routes_auth_kicks_to_the_wall() {
        // Rejection is classified *after* the auth-kick check, so a gateway
        // that refuses the upgrade citing a dead credential still walls.
        for reason in AUTH_KICK_REASONS {
            assert_eq!(
                classify(FailureStage::Rejected, Some(reason), false),
                ConnectionFailure::AuthRequired
            );
        }
    }

    #[test]
    fn rejection_keeps_retrying() {
        // The browser cannot read the upgrade's status code, so we cannot tell
        // a permanent 403 from a transient 503 (connection limit). Retrying is
        // the conservative choice: this variant changes the *copy*, not the
        // retry policy.
        assert!(ConnectionFailure::Rejected {
            detail: String::new()
        }
        .should_retry());
    }

    #[test]
    fn connect_errors_map_to_the_stage_that_describes_them() {
        // The mapping that was missing: a refusal and a silent socket are two
        // different failures and must not collapse onto one stage.
        assert_eq!(
            stage_for_connect_error(
                &ConnectionError::FailedBeforeOpen("code=1006".into()),
                OriginLiveness::Serving
            ),
            FailureStage::Rejected
        );
        assert_eq!(
            stage_for_connect_error(
                &ConnectionError::ConnectFailed("WebSocket open timed out".into()),
                OriginLiveness::Unknown
            ),
            FailureStage::BeforeOpen
        );
    }

    #[test]
    fn a_dead_port_is_unreachable_not_a_refusal() {
        // The browser reports "TCP connection refused" and "HTTP 403 on the
        // upgrade" identically: error + close(1006) with no reason. Only an
        // independent probe of the origin separates them. If the origin serves
        // nothing, the honest verdict is Unreachable — claiming "the server is
        // running but refused you" would send the operator to edit
        // allowed_origins when the real fix is to start the server.
        assert_eq!(
            stage_for_connect_error(
                &ConnectionError::FailedBeforeOpen("WebSocket error before open".into()),
                OriginLiveness::Silent
            ),
            FailureStage::BeforeOpen
        );
        assert_eq!(
            classify(
                stage_for_connect_error(
                    &ConnectionError::FailedBeforeOpen("WebSocket error before open".into()),
                    OriginLiveness::Silent
                ),
                Some("WebSocket error before open"),
                false
            ),
            ConnectionFailure::Unreachable {
                detail: "WebSocket error before open".to_string()
            }
        );
    }

    #[test]
    fn an_unprobed_refusal_does_not_claim_the_server_is_running() {
        // Fail safe: if the probe could not be run at all we must not assert
        // liveness we never established.
        assert_eq!(
            stage_for_connect_error(
                &ConnectionError::FailedBeforeOpen("x".into()),
                OriginLiveness::Unknown
            ),
            FailureStage::BeforeOpen
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
        assert_eq!(
            ConnectionFailure::Rejected {
                detail: String::new()
            }
            .i18n_key(),
            "rejected"
        );
    }
}
