//! Ephemeral canonical Tool calls: policy and lease gates without session ledger.
use crate::capability::{
    backend::TOOL_NAMESPACE,
    descriptor::CapabilityId,
    facade::{BackendLease, Reference, Zahir},
    ownership::VisibilityScope,
    policy_runtime::{CapabilityCallContext, ResourceUse},
    zahir_facade::ZahirFacade,
};
use crate::tools::{
    registry::{CanonicalEntryLease, ToolHandlerRegistry},
    service::{ToolError, ToolOutput},
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) struct EphemeralTool {
    captured: CanonicalEntryLease,
    lease: BackendLease,
}

impl EphemeralTool {
    pub(crate) fn prepare(
        facade: &ZahirFacade,
        captured: CanonicalEntryLease,
        context: &CapabilityCallContext,
    ) -> Option<Self> {
        // Cold plugin activation can persist state. This path only calls ready tools.
        if captured.is_cold_declaration() || captured.entry().descriptor.requires_confirmation {
            return None;
        }
        let lease = facade
            .resolve_with_capture_for_call(
                Reference {
                    id: CapabilityId {
                        namespace: TOOL_NAMESPACE.into(),
                        name: captured.entry().descriptor.name.clone(),
                    },
                    visibility: VisibilityScope::default(),
                },
                captured.clone(),
                context,
            )
            .ok()?;
        facade
            .prepare_tool_policy_for_call(&facade.policy(), &lease, context)
            .ok()?;
        Some(Self { captured, lease })
    }

    pub(crate) fn remains_authorized(
        &self,
        facade: &ZahirFacade,
        registry: &ToolHandlerRegistry,
        context: &CapabilityCallContext,
    ) -> bool {
        self.captured.validate(&registry.canonical_publication())
            && facade.validate_lease(&self.lease).is_ok()
            && facade
                .prepare_tool_policy_for_call(&facade.policy(), &self.lease, context)
                .is_ok()
    }

    pub(crate) async fn invoke(
        &self,
        facade: &ZahirFacade,
        registry: &ToolHandlerRegistry,
        input: Value,
        context: &CapabilityCallContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let entry = self.captured.entry();
        let denied = || ToolError::PermissionDenied {
            name: entry.descriptor.name.clone(),
            reason: "ephemeral capability admission failed".into(),
        };
        let policy = facade.policy();
        let attachment = facade
            .prepare_tool_policy_for_call(&policy, &self.lease, context)
            .map_err(|_| denied())?;
        let _running = policy
            .admit_runtime(
                &self.lease,
                Some(&attachment),
                context,
                ResourceUse::Allocate,
            )
            .map_err(|_| denied())?;
        if !self.remains_authorized(facade, registry, context) {
            return Err(denied());
        }
        let output = crate::tools::adapters::mcp_adapter::invoke_handler(
            &entry.handler,
            &entry.descriptor,
            input,
            cancel,
        )
        .await?;
        if !self.remains_authorized(facade, registry, context) {
            return Err(denied());
        }
        Ok(output)
    }
}
