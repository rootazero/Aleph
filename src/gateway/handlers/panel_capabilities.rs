//! RED contract tests for the Panel capability snapshot transport adapter.

#[cfg(test)]
mod tests {
    use aleph_protocol::panel::PanelUiCapabilitySnapshot;
    use serde_json::json;

    use super::snapshot_response;
    use crate::gateway::protocol::JsonRpcRequest;

    #[test]
    fn response_result_is_exactly_the_shared_snapshot_shape() {
        let snapshot = PanelUiCapabilitySnapshot::new(7, Vec::new());
        let request = JsonRpcRequest::with_id("panel.capabilities.snapshot", None, json!(41));

        let response = snapshot_response(request, snapshot.clone());
        let result = response.result.expect("snapshot query must succeed");
        let decoded: PanelUiCapabilitySnapshot =
            serde_json::from_value(result).expect("result must be a PanelUiCapabilitySnapshot");

        assert_eq!(decoded, snapshot);
        assert!(response.error.is_none());
    }

    #[test]
    fn empty_snapshot_is_a_successful_value() {
        let snapshot = PanelUiCapabilitySnapshot::new(0, Vec::new());
        let response = snapshot_response(
            JsonRpcRequest::with_id("panel.capabilities.snapshot", None, json!(42)),
            snapshot,
        );

        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }
}
