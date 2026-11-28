//! Ephemeral canonical Tool calls for owner-checked thought translation.

use super::GateAdmissionToken;
use crate::approval::{with_call_identity, CallIdentity};
use crate::capability::{
    backend::TOOL_NAMESPACE,
    descriptor::CapabilityId,
    facade::{BackendLease, Reference, Zahir},
    ownership::VisibilityScope,
    policy_runtime::{CapabilityCallContext, ResourceUse},
    zahir_facade::ZahirFacade,
};
use crate::tools::{
    descriptor::{ToolCallIdentity, ToolSource},
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
        let descriptor = &captured.entry().descriptor;
        let translation_tool = match &descriptor.source {
            ToolSource::Builtin => matches!(
                descriptor.name.as_str(),
                "thought_translate_machine" | "thought_translate_ai"
            ),
            ToolSource::Mcp { .. } => true,
            _ => false,
        };
        // Only ready translation adapters and the operator-selected MCP recipient
        // can enter this path. It has no interactive confirmation or replay lane.
        if !translation_tool || captured.is_cold_declaration() || descriptor.requires_confirmation {
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
        let call = CallIdentity {
            turn_id: uuid::Uuid::nil(),
            call_id: uuid::Uuid::new_v4().to_string(),
        };
        let admission = GateAdmissionToken::new(false);
        let invocation = crate::tools::dispatch_verdict::with_gate_admission(
            &admission,
            call.call_id.clone(),
            entry.descriptor.name.clone(),
            ToolCallIdentity::from_descriptor(&entry.descriptor),
            input.clone(),
            crate::identity::current_actor(),
            async {
                // The recipient result is parsed as data for Panel, outside the
                // model-facing output fence used by regular chat dispatch.
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Err(ToolError::Cancelled {
                        name: entry.descriptor.name.clone(),
                    }),
                    result = entry.handler.invoke(input) => result,
                }
            },
        );
        let output = with_call_identity(Some(call), invocation).await?;
        if !self.remains_authorized(facade, registry, context) {
            return Err(denied());
        }
        Ok(output)
    }
}
