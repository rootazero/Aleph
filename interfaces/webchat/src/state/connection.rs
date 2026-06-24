//! Connection phase — a pure projection over `DashboardState` signals.
//!
//! `DashboardState` exposes four orthogonal connection signals (`is_connected`,
//! `is_reconnecting`, `reconnect_count`, `connection_error`) that historically led
//! every consumer to roll its own ad-hoc boolean. This module collapses them
//! into a single enum so UI copy stays consistent across `ConnectionStatus`,
//! `BootCheckGate`, `ServiceBlockingGate`, and any future surface.

use shared_ui_logic::connection::ConnectionFailure;

/// Maximum reconnect attempts before `DashboardState::reconnect()` gives up.
/// Mirrors the hard-coded `max_attempts` in `context.rs::reconnect()`; kept here
/// so UI gates can show "X/MAX" without duplicating the literal.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Single source of truth for the user-visible connection state.
///
/// Variants are ordered roughly by severity — Connected is the happy path,
/// Failed is terminal until user retries. The boot screen uses Initial,
/// every other state is reachable from a running app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionPhase {
    /// Never tried to connect yet — only reachable from app boot.
    Initial,
    /// First connect attempt in flight (`reconnect_count` == 0).
    Connecting,
    /// Lost a previously-good connection and is retrying. `attempt` is 1-based
    /// for display (the underlying counter is 0-based).
    Reconnecting { attempt: u32, max: u32 },
    /// Connected and healthy.
    Connected,
    /// Reconnect ran out of attempts; user action required.
    Failed { failure: ConnectionFailure },
}

impl ConnectionPhase {
    /// Pure derivation from the four `DashboardState` booleans. Kept free of
    /// signal access so tests can exercise every transition without a Leptos
    /// runtime.
    #[must_use]
    pub fn derive(
        is_connected: bool,
        is_reconnecting: bool,
        reconnect_count: u32,
        connection_error: Option<&str>,
        has_connected_once: bool,
    ) -> Self {
        if is_connected {
            return Self::Connected;
        }
        // Any explicit error wins — surface it so the user gets a Retry path.
        if let Some(reason) = connection_error {
            return Self::Failed {
                failure: ConnectionFailure::Unknown { detail: reason.to_string() },
            };
        }
        if is_reconnecting {
            let attempt = reconnect_count
                .saturating_add(1)
                .min(MAX_RECONNECT_ATTEMPTS);
            return Self::Reconnecting {
                attempt,
                max: MAX_RECONNECT_ATTEMPTS,
            };
        }
        if has_connected_once {
            // Dropped connection but reconnect hasn't kicked in yet — show as
            // a momentary disconnect so the chip doesn't flash "Failed".
            return Self::Reconnecting {
                attempt: 1,
                max: MAX_RECONNECT_ATTEMPTS,
            };
        }
        if reconnect_count > 0 {
            // First-boot but already attempted at least once.
            return Self::Connecting;
        }
        Self::Initial
    }

    /// True when the app shell should be hidden behind the boot gate.
    #[must_use]
    pub const fn is_pre_ready(&self) -> bool {
        matches!(self, Self::Initial | Self::Connecting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_when_never_attempted() {
        let p = ConnectionPhase::derive(false, false, 0, None, false);
        assert_eq!(p, ConnectionPhase::Initial);
    }

    #[test]
    fn connecting_during_first_attempt() {
        // is_reconnecting=true is how reconnect() signals an attempt;
        // first-boot connect() does not set it, so we infer from counter.
        let p = ConnectionPhase::derive(false, false, 1, None, false);
        assert_eq!(p, ConnectionPhase::Connecting);
    }

    #[test]
    fn connected_overrides_other_signals() {
        let p = ConnectionPhase::derive(true, true, 3, Some("stale"), true);
        assert_eq!(p, ConnectionPhase::Connected);
    }

    #[test]
    fn reconnecting_uses_one_based_attempt() {
        let p = ConnectionPhase::derive(false, true, 2, None, true);
        assert_eq!(
            p,
            ConnectionPhase::Reconnecting {
                attempt: 3,
                max: MAX_RECONNECT_ATTEMPTS
            }
        );
    }

    #[test]
    fn reconnecting_clamps_attempt_to_max() {
        let p = ConnectionPhase::derive(false, true, 99, None, true);
        assert_eq!(
            p,
            ConnectionPhase::Reconnecting {
                attempt: MAX_RECONNECT_ATTEMPTS,
                max: MAX_RECONNECT_ATTEMPTS
            }
        );
    }

    #[test]
    fn dropped_connection_shows_reconnecting_attempt_one() {
        // Connection dropped (has_connected_once) but reconnect() not yet
        // running. UI should show "Reconnecting 1/5" rather than "Failed".
        let p = ConnectionPhase::derive(false, false, 0, None, true);
        assert_eq!(
            p,
            ConnectionPhase::Reconnecting {
                attempt: 1,
                max: MAX_RECONNECT_ATTEMPTS
            }
        );
    }

    #[test]
    fn failed_after_max_attempts() {
        use shared_ui_logic::connection::ConnectionFailure;
        let p = ConnectionPhase::derive(false, false, 5, Some("WebSocket closed"), true);
        assert_eq!(
            p,
            ConnectionPhase::Failed {
                failure: ConnectionFailure::Unknown {
                    detail: "WebSocket closed".to_string()
                }
            }
        );
    }

    #[test]
    fn explicit_error_during_boot_surfaces_immediately() {
        use shared_ui_logic::connection::ConnectionFailure;
        // First-boot probe failed — boot gate must show the trouble screen
        // with a Retry button rather than spinning forever. The user has
        // actionable info either way, so we don't gate on attempt count.
        let p = ConnectionPhase::derive(false, false, 1, Some("ECONNREFUSED"), false);
        assert_eq!(
            p,
            ConnectionPhase::Failed {
                failure: ConnectionFailure::Unknown {
                    detail: "ECONNREFUSED".to_string()
                }
            }
        );
    }

    #[test]
    fn failed_wraps_error_string_as_unknown() {
        use shared_ui_logic::connection::ConnectionFailure;
        // Any error string is preserved verbatim into the Unknown detail —
        // distinct content + state from failed_after_max_attempts.
        let p = ConnectionPhase::derive(false, true, 2, Some("custom transport glitch 42"), true);
        assert_eq!(
            p,
            ConnectionPhase::Failed {
                failure: ConnectionFailure::Unknown {
                    detail: "custom transport glitch 42".to_string()
                }
            }
        );
    }

    #[test]
    fn is_pre_ready_only_for_initial_and_connecting() {
        use shared_ui_logic::connection::ConnectionFailure;
        assert!(ConnectionPhase::Initial.is_pre_ready());
        assert!(ConnectionPhase::Connecting.is_pre_ready());
        assert!(!ConnectionPhase::Connected.is_pre_ready());
        assert!(!ConnectionPhase::Reconnecting { attempt: 1, max: 5 }.is_pre_ready());
        assert!(
            !ConnectionPhase::Failed {
                failure: ConnectionFailure::Unknown {
                    detail: "x".to_string()
                }
            }
            .is_pre_ready()
        );
    }
}
