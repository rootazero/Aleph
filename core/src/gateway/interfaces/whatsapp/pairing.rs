//! WhatsApp Pairing State Machine
//!
//! Fine-grained lifecycle tracking for WhatsApp's QR-based pairing flow.
//! Maps to the coarse 5-state `ChannelStatus` enum for gateway-level reporting.
//!
//! # State Transitions
//!
//! ```text
//!                    ┌─────────────────────────────────────┐
//!                    │              Failed                  │
//!                    │         (unrecoverable)              │
//!                    └──────────────┬──────────────────────┘
//!                                   │ reset
//!                                   ▼
//!   ┌──────┐ start  ┌──────────────┐ qr_ready ┌───────────┐
//!   │ Idle │──────→ │ Initializing │────────→ │ WaitingQr │
//!   └──┬───┘        └──────────────┘          └─────┬─────┘
//!      ▲                                     scanned│  │expired
//!      │                                            ▼  ▼
//!   ┌──┴──────────┐  reconnect  ┌─────────┐  ┌──────────┐
//!   │Disconnected │────────────→│Scanned  │  │QrExpired │
//!   └─────────────┘             └────┬────┘  └──────────┘
//!         ▲                          │ syncing     │ refresh
//!         │                          ▼             │
//!   ┌─────┴─────┐  sync_done  ┌──────────┐       │
//!   │ Connected │←───────────│ Syncing  │←──────┘
//!   └───────────┘             └──────────┘
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::gateway::channel::ChannelStatus;

/// Fine-grained pairing lifecycle state for WhatsApp.
///
/// WhatsApp pairing involves a multi-step QR scan + key sync flow.
/// This enum captures every meaningful phase so the UI can give
/// precise feedback, while `to_channel_status()` maps down to
/// the gateway's coarse 5-state model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PairingState {
    /// Bridge not started. Initial / reset state.
    Idle,

    /// Bridge process is starting up.
    Initializing,

    /// QR code is ready for scanning.
    WaitingQr {
        /// Base64-encoded QR data or a URI the client can render.
        qr_data: String,
        /// When this QR code expires.
        expires_at: DateTime<Utc>,
    },

    /// QR code expired before the user scanned it.
    QrExpired,

    /// User scanned the QR code, waiting for phone-side confirmation.
    Scanned,

    /// Syncing encryption keys / chat history.
    Syncing {
        /// Progress from 0.0 (just started) to 1.0 (complete).
        progress: f32,
    },

    /// Fully connected and operational.
    Connected {
        /// Name of the linked device (e.g. "Aleph Server").
        device_name: String,
        /// Linked phone number in E.164 format.
        phone_number: String,
    },

    /// Disconnected but potentially reconnectable.
    Disconnected {
        /// Human-readable reason for disconnection.
        reason: String,
    },

    /// Unrecoverable failure; must reset to Idle before retrying.
    Failed {
        /// Human-readable error description.
        error: String,
    },
}

impl PairingState {
    /// Map this fine-grained state to the gateway's coarse `ChannelStatus`.
    pub fn to_channel_status(&self) -> ChannelStatus {
        match self {
            PairingState::Idle => ChannelStatus::Disconnected,
            PairingState::Initializing
            | PairingState::WaitingQr { .. }
            | PairingState::QrExpired
            | PairingState::Scanned
            | PairingState::Syncing { .. } => ChannelStatus::Connecting,
            PairingState::Connected { .. } => ChannelStatus::Connected,
            PairingState::Disconnected { .. } => ChannelStatus::Disconnected,
            PairingState::Failed { .. } => ChannelStatus::Error,
        }
    }

    /// Whether the pairing has completed successfully and the channel is usable.
    pub fn is_connected(&self) -> bool {
        matches!(self, PairingState::Connected { .. })
    }

    /// Human-readable description of the current state.
    pub fn description(&self) -> &str {
        match self {
            PairingState::Idle => "Bridge not started",
            PairingState::Initializing => "Bridge process starting",
            PairingState::WaitingQr { .. } => "QR code ready for scanning",
            PairingState::QrExpired => "QR code expired, waiting for refresh",
            PairingState::Scanned => "QR scanned, waiting for phone confirmation",
            PairingState::Syncing { .. } => "Syncing encryption keys",
            PairingState::Connected { .. } => "Connected",
            PairingState::Disconnected { .. } => "Disconnected",
            PairingState::Failed { .. } => "Pairing failed",
        }
    }
}

/// Returns a short tag for the state variant (for transition error messages).
pub(crate) fn state_tag(state: &PairingState) -> &'static str {
    match state {
        PairingState::Idle => "Idle",
        PairingState::Initializing => "Initializing",
        PairingState::WaitingQr { .. } => "WaitingQr",
        PairingState::QrExpired => "QrExpired",
        PairingState::Scanned => "Scanned",
        PairingState::Syncing { .. } => "Syncing",
        PairingState::Connected { .. } => "Connected",
        PairingState::Disconnected { .. } => "Disconnected",
        PairingState::Failed { .. } => "Failed",
    }
}

/// Validate that a transition from `from` to `to` is legal.
///
/// # Valid transitions
///
/// | From           | Allowed targets                                    |
/// |----------------|----------------------------------------------------|
/// | Idle           | Initializing                                       |
/// | Initializing   | WaitingQr, Connected (reconnect), Failed            |
/// | WaitingQr      | Scanned, QrExpired, Failed                         |
/// | QrExpired      | WaitingQr, Failed                                  |
/// | Scanned        | Syncing, Connected (before HistorySync), Failed    |
/// | Syncing        | Syncing (progress update), Connected, Failed       |
/// | Connected      | Disconnected                                       |
/// | Disconnected   | Initializing, Connected (auto-reconnect), Idle     |
/// | Failed         | Idle                                               |
pub fn validate_transition(from: &PairingState, to: &PairingState) -> Result<(), String> {
    let valid = match from {
        PairingState::Idle => matches!(to, PairingState::Initializing),

        PairingState::Initializing => {
            matches!(
                to,
                PairingState::WaitingQr { .. }
                    | PairingState::Connected { .. } // Direct reconnection (existing session)
                    | PairingState::Failed { .. }
            )
        }

        PairingState::WaitingQr { .. } => matches!(
            to,
            PairingState::Scanned | PairingState::QrExpired | PairingState::Failed { .. }
        ),

        PairingState::QrExpired => {
            matches!(to, PairingState::WaitingQr { .. } | PairingState::Failed { .. })
        }

        PairingState::Scanned => {
            matches!(
                to,
                PairingState::Syncing { .. }
                    | PairingState::Connected { .. } // Connected may fire before HistorySync
                    | PairingState::Failed { .. }
            )
        }

        PairingState::Syncing { .. } => matches!(
            to,
            PairingState::Syncing { .. } | PairingState::Connected { .. } | PairingState::Failed { .. }
        ),

        PairingState::Connected { .. } => matches!(to, PairingState::Disconnected { .. }),

        PairingState::Disconnected { .. } => {
            matches!(
                to,
                PairingState::Initializing
                    | PairingState::Connected { .. } // whatsmeow auto-reconnect
                    | PairingState::Idle
            )
        }

        PairingState::Failed { .. } => matches!(to, PairingState::Idle),
    };

    if valid {
        Ok(())
    } else {
        Err(format!(
            "Invalid state transition: {} -> {}",
            state_tag(from),
            state_tag(to),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    // ── Helpers ──────────────────────────────────────────────────

    fn qr_state() -> PairingState {
        PairingState::WaitingQr {
            qr_data: "test-qr-data".to_string(),
            expires_at: Utc::now() + Duration::seconds(120),
        }
    }

    fn syncing(progress: f32) -> PairingState {
        PairingState::Syncing { progress }
    }

    fn connected() -> PairingState {
        PairingState::Connected {
            device_name: "Aleph Server".to_string(),
            phone_number: "+1234567890".to_string(),
        }
    }

    fn disconnected() -> PairingState {
        PairingState::Disconnected {
            reason: "network timeout".to_string(),
        }
    }

    fn failed() -> PairingState {
        PairingState::Failed {
            error: "bridge crashed".to_string(),
        }
    }

    // ── Channel status mapping ──────────────────────────────────

    #[test]
    fn test_channel_status_idle() {
        assert_eq!(PairingState::Idle.to_channel_status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_channel_status_initializing() {
        assert_eq!(
            PairingState::Initializing.to_channel_status(),
            ChannelStatus::Connecting
        );
    }

    #[test]
    fn test_channel_status_waiting_qr() {
        assert_eq!(qr_state().to_channel_status(), ChannelStatus::Connecting);
    }

    #[test]
    fn test_channel_status_qr_expired() {
        assert_eq!(
            PairingState::QrExpired.to_channel_status(),
            ChannelStatus::Connecting
        );
    }

    #[test]
    fn test_channel_status_scanned() {
        assert_eq!(
            PairingState::Scanned.to_channel_status(),
            ChannelStatus::Connecting
        );
    }

    #[test]
    fn test_channel_status_syncing() {
        assert_eq!(syncing(0.5).to_channel_status(), ChannelStatus::Connecting);
    }

    #[test]
    fn test_channel_status_connected() {
        assert_eq!(connected().to_channel_status(), ChannelStatus::Connected);
    }

    #[test]
    fn test_channel_status_disconnected() {
        assert_eq!(
            disconnected().to_channel_status(),
            ChannelStatus::Disconnected
        );
    }

    #[test]
    fn test_channel_status_failed() {
        assert_eq!(failed().to_channel_status(), ChannelStatus::Error);
    }

    // ── is_connected ────────────────────────────────────────────

    #[test]
    fn test_is_connected_true() {
        assert!(connected().is_connected());
    }

    #[test]
    fn test_is_connected_false_for_all_other_states() {
        let non_connected = vec![
            PairingState::Idle,
            PairingState::Initializing,
            qr_state(),
            PairingState::QrExpired,
            PairingState::Scanned,
            syncing(0.5),
            disconnected(),
            failed(),
        ];
        for state in non_connected {
            assert!(
                !state.is_connected(),
                "Expected is_connected() == false for {:?}",
                state
            );
        }
    }

    // ── description ─────────────────────────────────────────────

    #[test]
    fn test_description_not_empty() {
        let all_states = vec![
            PairingState::Idle,
            PairingState::Initializing,
            qr_state(),
            PairingState::QrExpired,
            PairingState::Scanned,
            syncing(0.0),
            connected(),
            disconnected(),
            failed(),
        ];
        for state in all_states {
            assert!(
                !state.description().is_empty(),
                "description() was empty for {:?}",
                state
            );
        }
    }

    // ── Happy-path transitions ──────────────────────────────────

    #[test]
    fn test_happy_path_full_cycle() {
        // Idle -> Initializing -> WaitingQr -> Scanned -> Syncing -> Connected
        let transitions: Vec<(PairingState, PairingState)> = vec![
            (PairingState::Idle, PairingState::Initializing),
            (PairingState::Initializing, qr_state()),
            (qr_state(), PairingState::Scanned),
            (PairingState::Scanned, syncing(0.0)),
            (syncing(0.0), syncing(0.5)),
            (syncing(0.5), syncing(1.0)),
            (syncing(1.0), connected()),
        ];

        for (i, (from, to)) in transitions.iter().enumerate() {
            assert!(
                validate_transition(from, to).is_ok(),
                "Happy-path step {} failed: {:?} -> {:?}",
                i,
                from,
                to,
            );
        }
    }

    // ── QR expired refresh cycle ────────────────────────────────

    #[test]
    fn test_qr_expired_refresh_cycle() {
        // WaitingQr -> QrExpired -> WaitingQr (refresh) -> Scanned
        assert!(validate_transition(&qr_state(), &PairingState::QrExpired).is_ok());
        assert!(validate_transition(&PairingState::QrExpired, &qr_state()).is_ok());
        assert!(validate_transition(&qr_state(), &PairingState::Scanned).is_ok());
    }

    // ── Reconnection path ───────────────────────────────────────

    #[test]
    fn test_reconnection_path() {
        // Connected -> Disconnected -> Initializing -> WaitingQr -> ... -> Connected
        assert!(validate_transition(&connected(), &disconnected()).is_ok());
        assert!(validate_transition(&disconnected(), &PairingState::Initializing).is_ok());
        assert!(validate_transition(&PairingState::Initializing, &qr_state()).is_ok());
    }

    #[test]
    fn test_disconnected_to_idle() {
        // User explicitly decides to stop reconnecting
        assert!(validate_transition(&disconnected(), &PairingState::Idle).is_ok());
    }

    // ── Failure recovery path ───────────────────────────────────

    #[test]
    fn test_failed_to_idle_only() {
        assert!(validate_transition(&failed(), &PairingState::Idle).is_ok());
        // Failed cannot go directly to Initializing
        assert!(validate_transition(&failed(), &PairingState::Initializing).is_err());
    }

    #[test]
    fn test_failure_from_every_fallible_state() {
        let fallible = vec![
            PairingState::Initializing,
            qr_state(),
            PairingState::QrExpired,
            PairingState::Scanned,
            syncing(0.5),
        ];
        for state in fallible {
            assert!(
                validate_transition(&state, &failed()).is_ok(),
                "Expected {:?} -> Failed to be valid",
                state,
            );
        }
    }

    // ── Invalid transitions ─────────────────────────────────────

    #[test]
    fn test_idle_cannot_jump_to_connected() {
        assert!(validate_transition(&PairingState::Idle, &connected()).is_err());
    }

    #[test]
    fn test_idle_cannot_go_to_waiting_qr() {
        assert!(validate_transition(&PairingState::Idle, &qr_state()).is_err());
    }

    #[test]
    fn test_connected_cannot_go_to_idle() {
        assert!(validate_transition(&connected(), &PairingState::Idle).is_err());
    }

    #[test]
    fn test_connected_cannot_go_to_failed() {
        assert!(validate_transition(&connected(), &failed()).is_err());
    }

    #[test]
    fn test_connected_cannot_go_to_initializing() {
        assert!(validate_transition(&connected(), &PairingState::Initializing).is_err());
    }

    #[test]
    fn test_scanned_can_go_to_connected() {
        // Connected may fire before HistorySync in whatsmeow
        assert!(validate_transition(&PairingState::Scanned, &connected()).is_ok());
    }

    #[test]
    fn test_initializing_cannot_go_to_scanned() {
        // Must go through WaitingQr first
        assert!(validate_transition(&PairingState::Initializing, &PairingState::Scanned).is_err());
    }

    #[test]
    fn test_waiting_qr_cannot_go_to_syncing() {
        // Must go through Scanned first
        assert!(validate_transition(&qr_state(), &syncing(0.0)).is_err());
    }

    #[test]
    fn test_qr_expired_cannot_go_to_scanned() {
        // Must refresh QR first (go back to WaitingQr)
        assert!(validate_transition(&PairingState::QrExpired, &PairingState::Scanned).is_err());
    }

    #[test]
    fn test_failed_cannot_go_to_connected() {
        assert!(validate_transition(&failed(), &connected()).is_err());
    }

    #[test]
    fn test_invalid_transition_error_message() {
        let err = validate_transition(&PairingState::Idle, &connected()).unwrap_err();
        assert!(err.contains("Invalid state transition"));
        assert!(err.contains("Idle"));
        assert!(err.contains("Connected"));
    }

    // ── Reconnection and auto-reconnect paths ────────────────────

    #[test]
    fn test_direct_reconnection() {
        // Initializing -> Connected (existing session, no QR needed)
        assert!(validate_transition(&PairingState::Initializing, &connected()).is_ok());
    }

    #[test]
    fn test_auto_reconnection() {
        // Disconnected -> Connected (whatsmeow auto-reconnect)
        assert!(validate_transition(&disconnected(), &connected()).is_ok());
    }

    // ── Syncing progress updates ────────────────────────────────

    #[test]
    fn test_syncing_progress_update() {
        assert!(validate_transition(&syncing(0.0), &syncing(0.3)).is_ok());
        assert!(validate_transition(&syncing(0.3), &syncing(0.7)).is_ok());
        assert!(validate_transition(&syncing(0.7), &syncing(1.0)).is_ok());
    }

    // ── Serialization roundtrip ─────────────────────────────────

    #[test]
    fn test_serde_idle_roundtrip() {
        let state = PairingState::Idle;
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"state\":\"idle\""));
        let restored: PairingState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn test_serde_waiting_qr_roundtrip() {
        let state = qr_state();
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"state\":\"waiting_qr\""));
        assert!(json.contains("\"qr_data\""));
        assert!(json.contains("\"expires_at\""));
        let restored: PairingState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn test_serde_syncing_roundtrip() {
        let state = syncing(0.75);
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"state\":\"syncing\""));
        assert!(json.contains("\"progress\""));
        let restored: PairingState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn test_serde_connected_roundtrip() {
        let state = connected();
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"state\":\"connected\""));
        assert!(json.contains("\"device_name\""));
        assert!(json.contains("\"phone_number\""));
        let restored: PairingState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn test_serde_disconnected_roundtrip() {
        let state = disconnected();
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"state\":\"disconnected\""));
        assert!(json.contains("\"reason\""));
        let restored: PairingState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn test_serde_failed_roundtrip() {
        let state = failed();
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"state\":\"failed\""));
        assert!(json.contains("\"error\""));
        let restored: PairingState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn test_serde_all_unit_variants() {
        for (state, expected_tag) in [
            (PairingState::Idle, "idle"),
            (PairingState::Initializing, "initializing"),
            (PairingState::QrExpired, "qr_expired"),
            (PairingState::Scanned, "scanned"),
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let expected = format!("\"state\":\"{}\"", expected_tag);
            assert!(
                json.contains(&expected),
                "Expected {} in JSON: {}",
                expected,
                json
            );
            let restored: PairingState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, restored);
        }
    }
}
