use aleph_protocol::panel::{
    PanelUiCapabilitySnapshot, PanelUiContextSnapshot, PANEL_PROJECT_SELECT_METHOD,
    PANEL_UI_CAPABILITY_SNAPSHOT_METHOD,
};
use serde::Deserialize;

use crate::gateway::handlers::plugins::get_extension_manager;
use crate::gateway::panel_project_context::PanelRequestContext;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, PERMISSION_DENIED,
};

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
    if !valid_snapshot_params(&request) {
        return denied(request);
    }
    let manager = match get_extension_manager() {
        Ok(manager) => manager,
        Err(response) => return response,
    };
    snapshot_response(request, manager.panel_ui_capability_snapshot())
}

/// Selection requires explicit, nullable project_id; omission cannot mean clear.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectParams {
    #[serde(deserialize_with = "Option::deserialize")]
    project_id: Option<String>,
    #[serde(rename = "idempotency_key")]
    _idempotency_key: Option<String>,
}

pub(crate) fn is_context_method(method: &str) -> bool {
    matches!(
        method,
        PANEL_PROJECT_SELECT_METHOD | PANEL_UI_CAPABILITY_SNAPSHOT_METHOD
    )
}

fn denied(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, PERMISSION_DENIED, "denied")
}

/// Request-only registries cannot manufacture authenticated device selection.
pub async fn handle_select(request: JsonRpcRequest) -> JsonRpcResponse {
    denied(request)
}

pub(crate) async fn handle_with_context(
    request: JsonRpcRequest,
    context: &PanelRequestContext,
) -> JsonRpcResponse {
    if request.method == PANEL_PROJECT_SELECT_METHOD {
        let Ok(params) = serde_json::from_value::<SelectParams>(
            request.params.clone().unwrap_or(serde_json::Value::Null),
        ) else {
            return denied(request);
        };
        return match context.selector.select(&context.caller, params.project_id) {
            Ok(selection) => JsonRpcResponse::success(request.id, serde_json::json!(selection)),
            Err(_) => denied(request),
        };
    }
    if !valid_snapshot_params(&request) {
        return denied(request);
    }
    let Ok(resolved) = context.selector.resolve(&context.caller) else {
        return denied(request);
    };
    let manager = match get_extension_manager() {
        Ok(manager) => manager,
        Err(response) => return response,
    };
    let snapshot = PanelUiContextSnapshot {
        snapshot: manager.panel_ui_capability_snapshot_for(&resolved.visibility),
        project_context: resolved.selection,
    };
    match serde_json::to_value(snapshot) {
        Ok(result) => JsonRpcResponse::success(request.id, result),
        Err(_) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Panel snapshot serialization failed",
        ),
    }
}

fn valid_snapshot_params(request: &JsonRpcRequest) -> bool {
    match request.params.as_ref() {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::Object(params)) => params.is_empty(),
        _ => false,
    }
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
