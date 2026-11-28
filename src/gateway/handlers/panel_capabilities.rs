use aleph_protocol::memory_graph::{GRAPH_MUTATE_NOTE_METHOD, NEBULA_TAB_ID};
use aleph_protocol::panel::{
    PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiCapabilityDeclaration,
    PanelUiCapabilityOrigin, PanelUiCapabilitySnapshot, PanelUiContextSnapshot, PanelUiTrust,
    PANEL_BUILTIN_NAMESPACE, PANEL_PROJECT_SELECT_METHOD, PANEL_UI_API_MAJOR,
    PANEL_UI_CAPABILITY_SNAPSHOT_METHOD,
};
use serde::Deserialize;

use crate::gateway::handlers::plugins::get_extension_manager;
use crate::gateway::panel_project_context::PanelRequestContext;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, PERMISSION_DENIED,
};

fn with_builtin_shell_tab(
    mut snapshot: PanelUiCapabilitySnapshot,
    tab_id: &str,
    label: &str,
    icon: &str,
    permissions: &[&str],
) -> PanelUiCapabilitySnapshot {
    let key = PanelTabKey {
        plugin_id: PANEL_BUILTIN_NAMESPACE.into(),
        tab_id: tab_id.into(),
    };
    if snapshot
        .declarations
        .iter()
        .any(|tab| tab.descriptor.key == key)
    {
        return snapshot;
    }
    snapshot.declarations.push(PanelUiCapabilityDeclaration {
        descriptor: PanelTabDescriptor {
            key,
            label: label.into(),
            icon: icon.into(),
            order: 0,
            entry: format!("shell/{tab_id}"),
            api_major: PANEL_UI_API_MAJOR,
            trust: PanelUiTrust::Verified,
            permissions: permissions
                .iter()
                .map(|method| (*method).to_owned())
                .collect(),
            status: PanelTabStatus::Enabled,
            status_detail: None,
        },
        origin: PanelUiCapabilityOrigin::BuiltIn,
        plugin_version: env!("CARGO_PKG_VERSION").into(),
        asset_integrity: None,
        public_base: "shell/".into(),
        source_revision: snapshot.revision,
    });
    snapshot
}

pub(crate) fn project_memory_nebula(result: &mut serde_json::Value, mutation_mounted: bool) {
    project_declaration(result, |snapshot| {
        let permissions: &[&str] = if mutation_mounted {
            &[GRAPH_MUTATE_NOTE_METHOD]
        } else {
            &[]
        };
        with_builtin_shell_tab(
            snapshot,
            NEBULA_TAB_ID,
            "Memory Nebula",
            "network",
            permissions,
        )
    });
}

pub(crate) fn project_voice(result: &mut serde_json::Value) {
    use aleph_protocol::panel_voice::VOICE_TAB_ID;
    project_declaration(result, |snapshot| {
        with_builtin_shell_tab(snapshot, VOICE_TAB_ID, "Voice", "mic", &[])
    });
}

/// Preserve the outer RPC result, including any flattened project selection.
fn project_declaration(
    result: &mut serde_json::Value,
    project: impl FnOnce(PanelUiCapabilitySnapshot) -> PanelUiCapabilitySnapshot,
) {
    let Ok(snapshot) = serde_json::from_value::<PanelUiCapabilitySnapshot>(result.clone()) else {
        return;
    };
    let count = snapshot.declarations.len();
    let projected = project(snapshot);
    if let Some(declaration) = projected.declarations.get(count) {
        if let (Some(declarations), Ok(value)) = (
            result
                .get_mut("declarations")
                .and_then(serde_json::Value::as_array_mut),
            serde_json::to_value(declaration),
        ) {
            declarations.push(value);
        }
    }
}

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
        // The shared accessor has no request id; answer with ours so the
        // caller can correlate the failure instead of timing out.
        Err(response) => {
            return JsonRpcResponse {
                id: request.id,
                ..response
            }
        }
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
        // The shared accessor has no request id; answer with ours so the
        // caller can correlate the failure instead of timing out.
        Err(response) => {
            return JsonRpcResponse {
                id: request.id,
                ..response
            }
        }
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

    #[tokio::test]
    async fn nebula_publication_tracks_mounted_reads() {
        use crate::gateway::handlers::HandlerRegistry;

        let request = JsonRpcRequest::with_id("panel.capabilities.snapshot", None, json!(43));
        let mut registry = HandlerRegistry::new();
        registry.register("panel.capabilities.snapshot", |request| async move {
            JsonRpcResponse::success(request.id, json!({"revision": 19, "declarations": []}))
        });
        for method in ["graph.query", "graph.search"] {
            registry.register(method, |request| async move {
                JsonRpcResponse::success(request.id, json!({}))
            });
        }
        let incomplete = registry.handle(&request).await.result.unwrap();
        assert_eq!(incomplete, json!({"revision": 19, "declarations": []}));

        registry.register("graph.node_detail", |request| async move {
            JsonRpcResponse::success(request.id, json!({}))
        });
        let mounted = registry.handle(&request).await.result.unwrap();
        assert_eq!(mounted["revision"], 19);
        let snapshot: PanelUiCapabilitySnapshot = serde_json::from_value(mounted).unwrap();
        assert_eq!(snapshot.declarations.len(), 1);
        shared_ui_logic::panel::projection::PanelProjection::from_snapshot(snapshot.clone())
            .expect("the published declaration must be admissible by the Panel");
        let declaration = &snapshot.declarations[0];
        assert_eq!(
            declaration.descriptor.key.qualified(),
            "aleph:memory-nebula"
        );
        assert_eq!(declaration.source_revision, 19);
        assert_eq!(
            declaration.descriptor.status,
            aleph_protocol::panel::PanelTabStatus::Enabled
        );
        assert_eq!(
            declaration.origin,
            aleph_protocol::panel::PanelUiCapabilityOrigin::BuiltIn
        );

        let selection = json!({"project_id": "project-a", "generation": 4});
        let mut contextual = json!({
            "revision": 19, "declarations": [], "project_context": selection,
        });
        super::project_memory_nebula(&mut contextual, false);
        assert_eq!(contextual["project_context"], selection);
        assert_eq!(
            contextual["declarations"],
            serde_json::to_value(snapshot.declarations).unwrap()
        );
    }
    use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

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
