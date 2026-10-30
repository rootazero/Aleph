use aleph_protocol::panel::{PanelUiCapabilitySnapshot, PANEL_UI_CAPABILITY_SNAPSHOT_METHOD};

use crate::gateway::handlers::plugins::get_extension_manager;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// Return the canonical Panel capability snapshot in the existing JSON-RPC envelope.
pub fn snapshot_response(
    request: JsonRpcRequest,
    snapshot: PanelUiCapabilitySnapshot,
) -> JsonRpcResponse {
    match serde_json::to_value(snapshot) {
        Ok(result) => JsonRpcResponse::success(request.id, result),
        Err(error) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize {PANEL_UI_CAPABILITY_SNAPSHOT_METHOD}: {error}"),
        ),
    }
}

/// Read the canonical snapshot once and expose it through the existing `/ws` RPC plane.
pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    let manager = match get_extension_manager() {
        Ok(manager) => manager,
        Err(response) => return response,
    };
    snapshot_response(request, manager.panel_ui_capability_snapshot())
}

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
