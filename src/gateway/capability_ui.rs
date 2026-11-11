//! Gateway transport for the focused capability UI surface.
//!
//! The gateway owns only JSON-RPC and trusted caller/session checks. The
//! surface, pending acknowledgement, and input queue remain in Core.

use crate::capability::facade::Scope;
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use crate::capability::policy_runtime::CapabilityCallContext;
use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport,
};
use crate::capability::special::SpecialRegistration;
use crate::capability::special_dispatch::{CapabilityInvocationError, CapabilityInvocationOrigin};
use crate::capability::ui::{UiError, UI_NAMESPACE};
use crate::capability::ui_surface::UiSurface;
use crate::gateway::caller_identity::{
    current_caller_conn_id, current_caller_user, current_caller_verified_user,
};
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, PERMISSION_DENIED,
    SERVICE_UNAVAILABLE,
};
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility::session_visible_to;
use crate::sync_primitives::{Arc, AtomicU64, Ordering};
use aleph_protocol::capability::{
    UiAttachParams, UiAttachment, UiDetachParams, UiInputEvent, UiInvokeParams, UiOperation,
    UiRenderAck, UiSubmitInput,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

tokio::task_local! {
    static UI_REQUEST_CANCEL: CancellationToken;
}

pub(crate) async fn with_ui_request_cancel<F: std::future::Future>(
    cancel: CancellationToken,
    future: F,
) -> F::Output {
    UI_REQUEST_CANCEL.scope(cancel, future).await
}

fn ui_request_cancel() -> Result<CancellationToken, UiError> {
    UI_REQUEST_CANCEL
        .try_with(Clone::clone)
        .map_err(|_| UiError::Unavailable)
}

static NEXT_MOUNT_GENERATION: AtomicU64 = AtomicU64::new(1);

const UI_DISPLAY_TOPIC: &str = "capabilities.ui.display";

/// Functional UI frames require the current mount as well as audit entitlement.
pub(crate) fn display_mount_admits(data: Option<&Value>, connection: &str) -> bool {
    let Some(data) = data else { return false };
    if data.get("connection_id").and_then(Value::as_str) != Some(connection) {
        return false;
    }
    let Some(surface_id) = data.get("surface_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(generation) = data.get("mount_generation").and_then(Value::as_u64) else {
        return false;
    };
    let Some(host) = crate::capability::projection_host::projection_host() else {
        return false;
    };
    if host.is_closing() {
        return false;
    }
    host.specials()
        .ui_surface(&surface_capability_id(surface_id))
        .is_some_and(|surface| {
            surface
                .validate_attachment(connection, surface_id, generation)
                .is_ok()
        })
}

/// Register a focused surface for one authenticated gateway connection.
pub async fn handle_attach(
    request: JsonRpcRequest,
    session_store: Arc<dyn SessionStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: UiAttachParams = match parse_params(&request) {
        Ok(params) => params,
        Err(error) => return error,
    };
    let Some(connection_id) = current_caller_conn_id() else {
        return JsonRpcResponse::error(
            request.id,
            PERMISSION_DENIED,
            "authenticated connection required",
        );
    };
    let Some(principal) = trusted_principal() else {
        return JsonRpcResponse::error(
            request.id,
            PERMISSION_DENIED,
            "authenticated principal required",
        );
    };
    let Some(session_key) =
        crate::gateway::router::SessionKey::from_key_string(&params.session_key)
    else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "invalid session key");
    };
    let metadata = match session_store.get_metadata(&session_key).await {
        Ok(Some(metadata)) => metadata,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                SERVICE_UNAVAILABLE,
                "session is unavailable",
            );
        }
        Err(error) => {
            return JsonRpcResponse::error(
                request.id,
                SERVICE_UNAVAILABLE,
                format!("session lookup failed: {error}"),
            );
        }
    };
    if !session_visible_to(&metadata, &principal) {
        return JsonRpcResponse::error(request.id, PERMISSION_DENIED, "session is not visible");
    }
    let Some(host) = crate::capability::projection_host::projection_host() else {
        return JsonRpcResponse::error(
            request.id,
            SERVICE_UNAVAILABLE,
            "capability projection host is not mounted",
        );
    };

    let surface_id = format!("surface-{}", uuid::Uuid::new_v4().simple());
    let mount_generation = NEXT_MOUNT_GENERATION.fetch_add(1, Ordering::Relaxed);
    let attachment = UiAttachment {
        surface_id: surface_id.clone(),
        mount_generation,
    };
    let owner = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some(principal.clone()),
        Some(params.session_key.clone()),
        None,
        Some(connection_id.clone()),
        request_call_id(&request, "capabilities.ui.attach"),
    );
    let audience = crate::capability::event_audience::EventAudience::from_call(
        &owner,
        Some(format!("ui/{surface_id}/{mount_generation}")),
    );
    let send_bus = Arc::clone(&event_bus);
    let send_connection = connection_id.clone();
    let send = Arc::new(
        move |message: aleph_protocol::capability::UiDisplayMessage| {
            send_bus
                .publish_json(&TopicEvent::new(
                    UI_DISPLAY_TOPIC,
                    json!({
                        "connection_id": send_connection,
                        "audience": audience,
                        "surface_id": message.surface_id,
                        "mount_generation": message.mount_generation,
                        "request_id": message.request_id,
                        "text": message.text,
                    }),
                ))
                .map(|_| ())
                .map_err(|_| UiError::Unavailable)
        },
    );
    let surface = Arc::new(UiSurface::new(attachment.clone(), owner, send));
    let capability_id = surface_capability_id(&surface_id);
    let visibility = VisibilityScope {
        principal: Some(principal.clone()),
        ..VisibilityScope::default()
    };
    let facade = host.facade();
    if facade
        .bind_terminal_session(capability_id.clone(), visibility.clone())
        .is_err()
    {
        return JsonRpcResponse::error(
            request.id,
            SERVICE_UNAVAILABLE,
            "UI surface is unavailable",
        );
    }

    let mut descriptor = crate::capability::ui::descriptor();
    descriptor.id = capability_id.clone();
    descriptor.lifetime = LifetimeScope::Session;
    descriptor.visibility = visibility;
    let provider_id = ProviderId::new(format!("builtin/ui/{surface_id}"))
        .expect("generated UI provider id is non-empty");
    let provider = ProviderHandle::new(ProviderLifecycle::ready(
        provider_id,
        ProviderTransport::InProcess,
    ));
    let registration = SpecialRegistration::new(
        descriptor,
        crate::capability::ui::schema(),
        surface.clone(),
        provider,
    )
    .with_ui_surface(surface);
    if let Err(error) = host.specials().register_with_handle(registration) {
        facade.withdraw_terminal_session(&capability_id);
        return JsonRpcResponse::error(
            request.id,
            SERVICE_UNAVAILABLE,
            format!("UI surface registration failed: {error}"),
        );
    }
    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(attachment).unwrap_or(Value::Null),
    )
}

/// Invoke the policy-admitted UI capability for one attached surface.
pub async fn handle_invoke(mut request: JsonRpcRequest) -> JsonRpcResponse {
    let cancel = match ui_request_cancel() {
        Ok(cancel) => cancel,
        Err(error) => return ui_error_response(request.id, error),
    };
    // Transport metadata is consumed here, not added to the shared UI wire.
    // Its value and namespace were already checked by connection dispatch.
    if let Some(Value::Object(params)) = request.params.as_mut() {
        params.remove("idempotency_key");
    }
    let params: UiInvokeParams = match parse_params(&request) {
        Ok(params) => params,
        Err(error) => return error,
    };
    let Some(connection_id) = current_caller_conn_id() else {
        return JsonRpcResponse::error(
            request.id,
            PERMISSION_DENIED,
            "authenticated connection required",
        );
    };
    let Some(principal) = trusted_principal() else {
        return JsonRpcResponse::error(
            request.id,
            PERMISSION_DENIED,
            "authenticated principal required",
        );
    };
    let Some(host) = crate::capability::projection_host::projection_host() else {
        return JsonRpcResponse::error(
            request.id,
            SERVICE_UNAVAILABLE,
            "capability projection host is not mounted",
        );
    };
    let context = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some(principal),
        None,
        None,
        Some(connection_id),
        request_call_id(&request, "capabilities.ui.invoke"),
    );
    match host
        .facade()
        .invoke_ui_gateway(
            params,
            context,
            CapabilityInvocationOrigin::GatewayDomain,
            cancel,
        )
        .await
    {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(error) => invocation_error_response(request.id, error),
    }
}

pub async fn handle_ack(request: JsonRpcRequest) -> JsonRpcResponse {
    let ack: UiRenderAck = match parse_params(&request) {
        Ok(ack) => ack,
        Err(error) => return error,
    };
    let Some(connection_id) = current_caller_conn_id() else {
        return JsonRpcResponse::error(
            request.id,
            PERMISSION_DENIED,
            "authenticated connection required",
        );
    };
    match surface_for_request(&ack.surface_id, &connection_id, ack.mount_generation) {
        Ok(surface) => match surface.ack(&connection_id, ack) {
            Ok(()) => JsonRpcResponse::success(request.id, json!({ "acknowledged": true })),
            Err(error) => ui_error_response(request.id, error),
        },
        Err(error) => ui_error_response(request.id, error),
    }
}

pub async fn handle_submit_input(request: JsonRpcRequest) -> JsonRpcResponse {
    let input: UiSubmitInput = match parse_params(&request) {
        Ok(input) => input,
        Err(error) => return error,
    };
    let Some(connection_id) = current_caller_conn_id() else {
        return JsonRpcResponse::error(
            request.id,
            PERMISSION_DENIED,
            "authenticated connection required",
        );
    };
    match surface_for_request(&input.surface_id, &connection_id, input.mount_generation) {
        Ok(surface) => match surface.submit_input(&connection_id, input) {
            Ok(()) => JsonRpcResponse::success(request.id, json!({ "accepted": true })),
            Err(error) => ui_error_response(request.id, error),
        },
        Err(error) => ui_error_response(request.id, error),
    }
}

pub async fn handle_detach(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UiDetachParams = match parse_params(&request) {
        Ok(params) => params,
        Err(error) => return error,
    };
    if params.surface_id.is_empty() || params.mount_generation == 0 {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "invalid surface attachment");
    }
    let Some(connection_id) = current_caller_conn_id() else {
        return JsonRpcResponse::error(
            request.id,
            PERMISSION_DENIED,
            "authenticated connection required",
        );
    };
    let Some(host) = crate::capability::projection_host::projection_host() else {
        return JsonRpcResponse::error(
            request.id,
            SERVICE_UNAVAILABLE,
            "capability projection host is not mounted",
        );
    };
    let capability_id = surface_capability_id(&params.surface_id);
    let Some(surface) = host.specials().ui_surface(&capability_id) else {
        return JsonRpcResponse::error(
            request.id,
            SERVICE_UNAVAILABLE,
            "UI surface is unavailable",
        );
    };
    if let Err(error) =
        surface.validate_attachment(&connection_id, &params.surface_id, params.mount_generation)
    {
        return ui_error_response(request.id, error);
    }
    if let Err(error) = surface.close().await {
        return ui_error_response(request.id, error);
    }
    let _ = host.specials().unregister(&capability_id);
    host.facade().withdraw_terminal_session(&capability_id);
    JsonRpcResponse::success(request.id, json!({ "detached": true }))
}

/// Close and withdraw every UI surface owned by a disconnected connection.
pub async fn close_connection_surfaces(connection_id: &str) {
    let Some(host) = crate::capability::projection_host::projection_host() else {
        return;
    };
    let scope = Scope {
        kind: Some(crate::capability::descriptor::CapabilityKind::Ui),
        ..Scope::default()
    };
    let records = host.specials().describe(&scope);
    for record in records {
        let id = record.descriptor().id.clone();
        let Some(surface) = host.specials().ui_surface(&id) else {
            continue;
        };
        if surface.owner_connection() != connection_id {
            continue;
        }
        if let Err(error) = surface.close().await {
            tracing::warn!(%error, %connection_id, "failed to close disconnected UI surface");
        }
        let _ = host.specials().unregister(&id);
        host.facade().withdraw_terminal_session(&id);
    }
}

/// The caller supplies the actual source host, never a replacement predicate.
/// Production uses its installed host; tests can own and close an isolated one.
pub(crate) fn display_mount_is_current(
    data: &Value,
    connection_id: &str,
    host: &crate::capability::projection_host::ProjectionHost,
) -> bool {
    let Some(surface_id) = data.get("surface_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(generation) = data.get("mount_generation").and_then(Value::as_u64) else {
        return false;
    };
    surface_for_request_at_host(host, surface_id, connection_id, generation).is_ok()
}

fn surface_for_request(
    surface_id: &str,
    connection_id: &str,
    mount_generation: u64,
) -> Result<Arc<UiSurface>, UiError> {
    let Some(host) = crate::capability::projection_host::projection_host() else {
        return Err(UiError::Unavailable);
    };
    surface_for_request_at_host(&host, surface_id, connection_id, mount_generation)
}

fn surface_for_request_at_host(
    host: &crate::capability::projection_host::ProjectionHost,
    surface_id: &str,
    connection_id: &str,
    mount_generation: u64,
) -> Result<Arc<UiSurface>, UiError> {
    let Some(surface) = host
        .specials()
        .ui_surface(&surface_capability_id(surface_id))
    else {
        return Err(UiError::Unavailable);
    };
    surface.validate_attachment(connection_id, surface_id, mount_generation)?;
    Ok(surface)
}

fn surface_capability_id(surface_id: &str) -> crate::capability::descriptor::CapabilityId {
    crate::capability::descriptor::CapabilityId {
        namespace: UI_NAMESPACE.into(),
        name: surface_id.into(),
    }
}

fn trusted_principal() -> Option<String> {
    current_caller_verified_user().or_else(current_caller_user)
}

fn request_call_id(request: &JsonRpcRequest, method: &str) -> String {
    format!(
        "{method}:{}",
        request
            .id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "notification".into())
    )
}

fn parse_params<T: serde::de::DeserializeOwned>(
    request: &JsonRpcRequest,
) -> Result<T, JsonRpcResponse> {
    match &request.params {
        Some(params) => serde_json::from_value(params.clone()).map_err(|error| {
            JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("Invalid params: {error}"),
            )
        }),
        None => Err(JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "Missing params",
        )),
    }
}

fn ui_error_response(id: Option<Value>, error: UiError) -> JsonRpcResponse {
    let code = match error {
        UiError::InvalidInput(_) => INVALID_PARAMS,
        UiError::StaleAttachment | UiError::Closed | UiError::Unavailable => SERVICE_UNAVAILABLE,
        UiError::QueueFull
        | UiError::Busy
        | UiError::RenderFailed
        | UiError::Timeout
        | UiError::Cancelled => INTERNAL_ERROR,
        UiError::Poisoned => INTERNAL_ERROR,
    };
    JsonRpcResponse::error(id, code, error.to_string())
}

fn invocation_error_response(
    id: Option<Value>,
    error: CapabilityInvocationError,
) -> JsonRpcResponse {
    match error {
        CapabilityInvocationError::Policy(error) => {
            JsonRpcResponse::error(id, PERMISSION_DENIED, error.to_string())
        }
        CapabilityInvocationError::Resolve(error) => JsonRpcResponse::error(
            id,
            SERVICE_UNAVAILABLE,
            format!("UI surface unavailable: {error:?}"),
        ),
        CapabilityInvocationError::Handler(
            crate::capability::special::CapabilityDispatchError::InvalidInput(error),
        ) => JsonRpcResponse::error(id, INVALID_PARAMS, error),
        CapabilityInvocationError::Handler(error) => {
            JsonRpcResponse::error(id, INTERNAL_ERROR, error.to_string())
        }
        CapabilityInvocationError::Unavailable | CapabilityInvocationError::Cancelled => {
            JsonRpcResponse::error(id, SERVICE_UNAVAILABLE, "UI surface unavailable")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_surface_ids_are_capability_ids_in_the_ui_namespace() {
        let id = surface_capability_id("surface-test");
        assert_eq!(id.namespace, UI_NAMESPACE);
        assert_eq!(id.name, "surface-test");
    }

    #[test]
    fn mount_generation_is_nonzero() {
        assert!(NEXT_MOUNT_GENERATION.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn wire_input_is_still_the_shared_type() {
        let _ = UiInputEvent::KeyDown { key: "a".into() };
        let _ = UiOperation::Input {};
    }
}
