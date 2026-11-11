//! Capability discovery over the gateway JSON-RPC face.

use super::{INTERNAL_ERROR, INVALID_PARAMS, JsonRpcRequest, JsonRpcResponse, SERVICE_UNAVAILABLE, parse_params};
use crate::gateway::protocol::PERMISSION_DENIED;
use crate::capability::facade::{Scope, Zahir};
use aleph_protocol::capability::{CapabilityDescribeParams, CapabilityDescription, PtyInvokeParams};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Describe the mounted special capability records, including their complete
/// canonical schemas. This is discovery only; it never invokes a handler.
pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: CapabilityDescribeParams = match parse_params(&request) {
        Ok(params) => params,
        Err(error) => return error,
    };
    let namespace = params.namespace.filter(|value| !value.trim().is_empty());
    let scope = Scope {
        namespace,
        ..Scope::default()
    };
    let Some(host) = crate::capability::projection_host::projection_host() else {
        return JsonRpcResponse::error(
            request.id,
            SERVICE_UNAVAILABLE,
            "capability projection host is not mounted",
        );
    };
    let facade = host.facade();
    let snapshot = facade.describe(scope);
    let descriptions = snapshot
        .capabilities
        .into_iter()
        .filter_map(|descriptor| {
            let capture = host.specials().capture(&descriptor.id)?;
            Some(CapabilityDescription {
                // Keep authority-bearing values nested and omit owner/provider
                // generation. Those values are validated internally, never a
                // client claim or a discovery response field.
                descriptor: json!({
                    "id": format!("{}/{}", descriptor.id.namespace, descriptor.id.name),
                    "kind": format!("{:?}", descriptor.kind),
                    "revision": descriptor.revision.0,
                    "schema": {
                        "version": descriptor.schema.version,
                        "fingerprint": hex::encode(descriptor.schema.fingerprint),
                    },
                    "lifetime": format!("{:?}", descriptor.lifetime),
                    "facets": {
                        "callable": descriptor.facets.contains(crate::capability::descriptor::CapabilityFacet::Callable),
                        "external_effect": descriptor.facets.contains(crate::capability::descriptor::CapabilityFacet::ExternalEffect),
                    },
                }),
                schema: capture.schema().clone(),
            })
        })
        .collect::<Vec<_>>();
    match serde_json::to_value(descriptions) {
        Ok(data) => JsonRpcResponse::success(
            request.id,
            json!({ "capabilities": data, "cursor": snapshot.committed_cursor.0 }),
        ),
        Err(error) => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("failed to encode capability descriptions: {error}"),
        ),
    }
}

/// Invoke one source-owned terminal session through the capability facade.
///
/// The gateway method-admin layer supplies the operator-only admission gate;
/// this handler only parses the wire value and delegates policy/effect checks to
/// the mounted facade.
pub async fn handle_terminal_invoke(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: PtyInvokeParams = match parse_params(&request) {
        Ok(params) => params,
        Err(error) => return error,
    };
    if let Err(error) = params.validate() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, error.to_string());
    }
    let Some(host) = crate::capability::projection_host::projection_host() else {
        return JsonRpcResponse::error(
            request.id,
            SERVICE_UNAVAILABLE,
            "capability projection host is not mounted",
        );
    };
    let call_id = format!(
        "capabilities.terminal.invoke:{}",
        request
            .id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "notification".into())
    );
    match host
        .facade()
        .invoke_terminal_gateway(params, call_id, CancellationToken::new(), None)
        .await
    {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(crate::capability::special_dispatch::CapabilityInvocationError::Unavailable)
        | Err(crate::capability::special_dispatch::CapabilityInvocationError::Resolve(
            crate::capability::facade::ResolveError::Unknown,
        ))
        | Err(crate::capability::special_dispatch::CapabilityInvocationError::Resolve(
            crate::capability::facade::ResolveError::NotVisible,
        ))
        | Err(crate::capability::special_dispatch::CapabilityInvocationError::Resolve(
            crate::capability::facade::ResolveError::Revoked,
        )) => JsonRpcResponse::error(request.id, SERVICE_UNAVAILABLE, "terminal capability unavailable"),
        Err(crate::capability::special_dispatch::CapabilityInvocationError::Cancelled) => {
            JsonRpcResponse::error(request.id, SERVICE_UNAVAILABLE, "terminal capability cancelled")
        }
        Err(crate::capability::special_dispatch::CapabilityInvocationError::Policy(error)) => {
            JsonRpcResponse::error(request.id, PERMISSION_DENIED, error.to_string())
        }
        Err(crate::capability::special_dispatch::CapabilityInvocationError::Resolve(error)) => {
            JsonRpcResponse::error(request.id, SERVICE_UNAVAILABLE, format!("terminal capability unavailable: {error:?}"))
        }
        Err(crate::capability::special_dispatch::CapabilityInvocationError::Handler(
            crate::capability::special::CapabilityDispatchError::NotFound(_),
        )) => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "terminal session was not found",
        ),
        Err(crate::capability::special_dispatch::CapabilityInvocationError::Handler(error)) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, error.to_string())
        }
    }
}
