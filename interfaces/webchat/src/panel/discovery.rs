//! Connection-scoped discovery state for the canonical Panel capability snapshot.

use aleph_protocol::panel::PanelUiCapabilitySnapshot;
use serde_json::Value;

/// The current outcome of the discovery query for one connection epoch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PanelDiscoveryStatus {
    Disconnected,
    Loading,
    Ready,
    Error { message: String },
}

/// Result of applying a snapshot to the connection-scoped state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotUpdate {
    Accepted,
    StaleEpoch,
    StaleRequest,
    StaleRevision,
}

/// Discovery-only state. It does not project declarations into browser mounts.
#[derive(Clone, Debug)]
pub struct PanelDiscoveryState {
    connection_epoch: u64,
    snapshot: Option<PanelUiCapabilitySnapshot>,
    last_revision: Option<u64>,
    accepted_epoch: Option<u64>,
    request_generation: u64,
    status: PanelDiscoveryStatus,
}

impl PanelDiscoveryState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            connection_epoch: 0,
            snapshot: None,
            last_revision: None,
            accepted_epoch: None,
            request_generation: 0,
            status: PanelDiscoveryStatus::Disconnected,
        }
    }

    /// Start a query for a new socket epoch while retaining the last known snapshot.
    pub fn begin_connection(&mut self, epoch: u64) -> Option<u64> {
        if epoch < self.connection_epoch
            || (epoch == self.connection_epoch
                && matches!(self.status, PanelDiscoveryStatus::Disconnected))
        {
            return None;
        }
        self.connection_epoch = epoch;
        self.request_generation = self.request_generation.wrapping_add(1);
        self.status = PanelDiscoveryStatus::Loading;
        Some(self.request_generation)
    }

    /// Apply a snapshot using the currently active request generation.
    pub fn accept_snapshot(
        &mut self,
        epoch: u64,
        snapshot: PanelUiCapabilitySnapshot,
    ) -> SnapshotUpdate {
        self.accept_snapshot_for_request(epoch, self.request_generation, snapshot)
    }

    /// Apply a snapshot only from the request that currently owns the state.
    pub fn accept_snapshot_for_request(
        &mut self,
        epoch: u64,
        request_generation: u64,
        snapshot: PanelUiCapabilitySnapshot,
    ) -> SnapshotUpdate {
        if epoch != self.connection_epoch
            || matches!(self.status, PanelDiscoveryStatus::Disconnected)
        {
            return SnapshotUpdate::StaleEpoch;
        }
        if request_generation != self.request_generation {
            return SnapshotUpdate::StaleRequest;
        }
        if self.last_revision.is_some_and(|revision| {
            snapshot.revision < revision
                || (snapshot.revision == revision && self.accepted_epoch == Some(epoch))
        }) {
            if self.snapshot.is_some() {
                self.status = PanelDiscoveryStatus::Ready;
            }
            return SnapshotUpdate::StaleRevision;
        }
        self.last_revision = Some(snapshot.revision);
        self.accepted_epoch = Some(epoch);
        self.snapshot = Some(snapshot);
        self.status = PanelDiscoveryStatus::Ready;
        SnapshotUpdate::Accepted
    }

    /// Preserve transport failure for the currently active request.
    pub fn mark_error(&mut self, epoch: u64, message: impl Into<String>) {
        self.mark_error_for_request(epoch, self.request_generation, message);
    }

    /// Preserve transport failure only for the request that currently owns the state.
    pub fn mark_error_for_request(
        &mut self,
        epoch: u64,
        request_generation: u64,
        message: impl Into<String>,
    ) {
        if epoch == self.connection_epoch
            && request_generation == self.request_generation
            && !matches!(self.status, PanelDiscoveryStatus::Disconnected)
        {
            self.status = PanelDiscoveryStatus::Error {
                message: message.into(),
            };
        }
    }

    /// Mark the current socket as disconnected without inventing an empty snapshot.
    pub fn disconnect(&mut self, epoch: u64) {
        if epoch == self.connection_epoch {
            self.request_generation = self.request_generation.wrapping_add(1);
            self.status = PanelDiscoveryStatus::Disconnected;
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> Option<&PanelUiCapabilitySnapshot> {
        self.snapshot.as_ref()
    }

    #[must_use]
    pub fn status(&self) -> &PanelDiscoveryStatus {
        &self.status
    }

    #[must_use]
    pub const fn connection_epoch(&self) -> u64 {
        self.connection_epoch
    }

    #[must_use]
    pub const fn last_revision(&self) -> Option<u64> {
        self.last_revision
    }
}

impl Default for PanelDiscoveryState {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode the shared wire shape without routing it through browser projection.
pub fn decode_snapshot(value: Value) -> Result<PanelUiCapabilitySnapshot, serde_json::Error> {
    serde_json::from_value(value)
}

#[cfg(test)]
mod tests {
    use aleph_protocol::jsonrpc::JsonRpcResponse;
    use aleph_protocol::panel::PanelUiCapabilitySnapshot;
    use serde_json::json;

    use super::{decode_snapshot, PanelDiscoveryState, PanelDiscoveryStatus, SnapshotUpdate};

    #[test]
    fn control_plane_success_envelope_reaches_discovery_decoder() {
        let snapshot = PanelUiCapabilitySnapshot::new(17, Vec::new());
        let response = JsonRpcResponse::success(
            json!(41),
            serde_json::to_value(&snapshot).expect("snapshot serializes"),
        );
        let wire = serde_json::to_value(response).expect("response serializes");
        let response: JsonRpcResponse = serde_json::from_value(wire).expect("response decodes");
        let decoded =
            decode_snapshot(response.result.expect("success result")).expect("snapshot decodes");

        assert_eq!(decoded, snapshot);
    }

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
    fn replacement_and_retraction_are_complete_snapshot_replacements() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(1);
        let declaration = aleph_protocol::panel::PanelUiCapabilityDeclaration {
            descriptor: aleph_protocol::panel::PanelTabDescriptor {
                key: aleph_protocol::panel::PanelTabKey {
                    plugin_id: "community.example".into(),
                    tab_id: "home".into(),
                },
                label: "Community".into(),
                icon: "spark".into(),
                order: 1,
                entry: "index.js".into(),
                api_major: 1,
                trust: aleph_protocol::panel::PanelUiTrust::Trusted,
                permissions: vec!["session.read".into()],
                status: aleph_protocol::panel::PanelTabStatus::Enabled,
                status_detail: None,
            },
            origin: aleph_protocol::panel::PanelUiCapabilityOrigin::Community,
            plugin_version: "1.0.0".into(),
            asset_integrity: Some("sha256-test".into()),
            public_base: "/plugins/community.example/".into(),
            source_revision: 1,
        };
        assert_eq!(
            state.accept_snapshot(1, PanelUiCapabilitySnapshot::new(1, vec![declaration])),
            SnapshotUpdate::Accepted
        );
        assert_eq!(
            state.snapshot().map(|snapshot| snapshot.declarations.len()),
            Some(1)
        );
        assert_eq!(
            state.accept_snapshot(1, PanelUiCapabilitySnapshot::new(2, Vec::new())),
            SnapshotUpdate::Accepted
        );
        assert_eq!(
            state.snapshot().map(|snapshot| snapshot.declarations.len()),
            Some(0)
        );
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
        assert_eq!(state.status(), &PanelDiscoveryStatus::Ready);
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
            &PanelDiscoveryStatus::Error {
                message: "socket closed".into()
            }
        );

        state.disconnect(2);
        assert_eq!(state.status(), &PanelDiscoveryStatus::Disconnected);
        state.mark_error(2, "late failure");
        assert_eq!(state.status(), &PanelDiscoveryStatus::Disconnected);
        assert!(state.snapshot().is_none());
    }

    #[test]
    fn an_older_refresh_cannot_overwrite_a_newer_refresh() {
        let mut state = PanelDiscoveryState::new();
        let old_request = state.begin_connection(1).expect("old request");
        let new_request = state.begin_connection(1).expect("new request");
        assert_ne!(old_request, new_request);
        assert_eq!(
            state.accept_snapshot_for_request(
                1,
                old_request,
                PanelUiCapabilitySnapshot::new(99, Vec::new())
            ),
            SnapshotUpdate::StaleRequest
        );
        state.mark_error_for_request(1, old_request, "late failure");
        assert_eq!(state.status(), &PanelDiscoveryStatus::Loading);
        assert_eq!(
            state.accept_snapshot_for_request(
                1,
                new_request,
                PanelUiCapabilitySnapshot::new(7, Vec::new())
            ),
            SnapshotUpdate::Accepted
        );
        state.mark_error_for_request(1, old_request, "late failure");
        assert_eq!(state.status(), &PanelDiscoveryStatus::Ready);
    }

    #[test]
    fn malformed_payload_does_not_erase_last_good_snapshot() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(1);
        assert_eq!(
            state.accept_snapshot(1, PanelUiCapabilitySnapshot::new(5, Vec::new())),
            SnapshotUpdate::Accepted
        );
        assert!(decode_snapshot(json!({"revision": "bad", "declarations": []})).is_err());
        assert_eq!(state.snapshot().map(|snapshot| snapshot.revision), Some(5));
    }

    #[test]
    fn reconnect_full_snapshot_repairs_a_missed_update() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(1);
        assert_eq!(
            state.accept_snapshot(1, PanelUiCapabilitySnapshot::new(5, Vec::new())),
            SnapshotUpdate::Accepted
        );
        state.begin_connection(2);
        assert_eq!(
            state.accept_snapshot(2, PanelUiCapabilitySnapshot::new(7, Vec::new())),
            SnapshotUpdate::Accepted
        );
        assert_eq!(state.last_revision(), Some(7));
    }

    #[test]
    fn disconnect_preserves_the_last_known_snapshot() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(1);
        assert_eq!(
            state.accept_snapshot(1, PanelUiCapabilitySnapshot::new(5, Vec::new())),
            SnapshotUpdate::Accepted
        );
        state.disconnect(1);
        assert_eq!(state.status(), &PanelDiscoveryStatus::Disconnected);
        assert_eq!(state.snapshot().map(|snapshot| snapshot.revision), Some(5));
    }

    #[test]
    fn a_disconnected_epoch_cannot_be_reopened_without_reconnect() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(1);
        state.disconnect(1);
        state.begin_connection(1);
        assert_eq!(state.status(), &PanelDiscoveryStatus::Disconnected);
    }

    #[test]
    fn a_late_snapshot_after_disconnect_is_rejected() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(1);
        state.disconnect(1);
        assert_eq!(
            state.accept_snapshot(1, PanelUiCapabilitySnapshot::new(9, Vec::new())),
            SnapshotUpdate::StaleEpoch
        );
        assert_eq!(state.status(), &PanelDiscoveryStatus::Disconnected);
        assert!(state.snapshot().is_none());
    }

    #[test]
    fn a_same_revision_snapshot_is_rebased_after_reconnect() {
        let mut state = PanelDiscoveryState::new();
        state.begin_connection(1);
        assert_eq!(
            state.accept_snapshot(1, PanelUiCapabilitySnapshot::new(4, Vec::new())),
            SnapshotUpdate::Accepted
        );
        state.begin_connection(2);
        assert_eq!(
            state.accept_snapshot(2, PanelUiCapabilitySnapshot::new(4, Vec::new())),
            SnapshotUpdate::Accepted
        );
    }
}
