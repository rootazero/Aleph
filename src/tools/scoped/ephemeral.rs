//! Ephemeral canonical Tool calls: policy and lease gates without session ledger.
use crate::capability::{
    backend::TOOL_NAMESPACE,
    descriptor::CapabilityId,
    facade::{Reference, Zahir},
    ownership::VisibilityScope,
    policy_runtime::{CapabilityCallContext, ResourceUse},
    zahir_facade::ZahirFacade,
};
use crate::tools::{
    registry::ToolHandlerRegistry,
    service::{ToolError, ToolOutput},
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

tokio::task_local! {
    pub(crate) static EPHEMERAL_LEASE: crate::capability::facade::BackendLease;
}

pub(crate) async fn invoke_ephemeral_tool(
    facade: &ZahirFacade,
    registry: &ToolHandlerRegistry,
    name: &str,
    input: Value,
    context: &CapabilityCallContext,
    cancel: CancellationToken,
) -> Result<ToolOutput, ToolError> {
    let denied = || ToolError::PermissionDenied {
        name: name.into(),
        reason: "ephemeral capability admission failed".into(),
    };
    let captured = registry.capture_entry(name).ok_or_else(denied)?;
    // Cold plugin activation can persist state; this operation only calls Ready tools.
    if captured.is_cold_declaration() || captured.entry().descriptor.requires_confirmation {
        return Err(denied());
    }
    let lease = facade
        .resolve_with_capture_for_call(
            Reference {
                id: CapabilityId {
                    namespace: TOOL_NAMESPACE.into(),
                    name: name.into(),
                },
                visibility: VisibilityScope::default(),
            },
            captured.clone(),
            context,
        )
        .map_err(|_| denied())?;
    let policy = facade.policy();
    let attachment = facade
        .prepare_tool_policy_for_call(&policy, &lease, context)
        .map_err(|_| denied())?;
    let _running = policy
        .admit_runtime(&lease, Some(&attachment), context, ResourceUse::Allocate)
        .map_err(|_| denied())?;
    facade.validate_lease(&lease).map_err(|_| denied())?;
    let entry = captured.entry();
    let output = EPHEMERAL_LEASE
        .scope(
            lease.clone(),
            crate::tools::adapters::mcp_adapter::invoke_handler(
                &entry.handler,
                &entry.descriptor,
                input,
                cancel,
            ),
        )
        .await?;
    if !captured.validate(&registry.canonical_publication()) {
        return Err(denied());
    }
    facade.validate_lease(&lease).map_err(|_| denied())?;
    Ok(output)
}
