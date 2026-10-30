//! RED contract tests for connection-scoped Panel discovery state.

#[cfg(test)]
mod tests {
    use aleph_protocol::panel::PanelUiCapabilitySnapshot;
    use serde_json::json;

    use super::{PanelDiscoveryState, PanelDiscoveryStatus, SnapshotUpdate};

    #[test]
    fn empty_and_non_empty_snapshots_decode_as_successful_values() {
        let empty = decode_snapshot(json!({
            "revision": 0,
            "declarations": []
        }))
        .expect("empty snapshot must decode");
        assert_eq!(empty, PanelUiCapabilitySnapshot::new(0, Vec::new()));

        let snapshot = decode_snapshot(json!({
            "revision": 3,
            "declarations": []
        }))
        .expect("canonical snapshot must decode");
        assert_eq!(snapshot.revision, 3);
    }

    #[test]
    fn malformed_payload_remains_a_decode_error() {
        assert!(decode_snapshot(json!({
            "revision": "three",
            "declarations": []
        }))
        .is_err());
    }

    #[test]
    fn stale_revision_cannot_replace_a_newer_snapshot() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(4);
        assert_eq!(
            state.accept_snapshot(4, PanelUiCapabilitySnapshot::new(8, Vec::new())),
            SnapshotUpdate::Accepted
        );
        assert_eq!(
            state.accept_snapshot(4, PanelUiCapabilitySnapshot::new(7, Vec::new())),
            SnapshotUpdate::StaleRevision
        );
        assert_eq!(state.snapshot().map(|snapshot| snapshot.revision), Some(8));
        assert_eq!(state.status(), PanelDiscoveryStatus::Ready);
    }

    #[test]
    fn an_old_connection_epoch_cannot_write_current_discovery_state() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(9);

        assert_eq!(
            state.accept_snapshot(8, PanelUiCapabilitySnapshot::new(99, Vec::new())),
            SnapshotUpdate::StaleEpoch
        );
        assert!(state.snapshot().is_none());
    }

    #[test]
    fn transport_error_and_disconnect_are_not_successful_empty_snapshots() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(2);
        state.mark_error(2, "socket closed");
        assert_eq!(
            state.status(),
            PanelDiscoveryStatus::Error {
                message: "socket closed".into()
            }
        );

        state.disconnect(2);
        assert_eq!(state.status(), PanelDiscoveryStatus::Disconnected);
        assert!(state.snapshot().is_none());
    }
}
