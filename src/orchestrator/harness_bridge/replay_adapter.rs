//! Bridge adapter: turns a [`ReplayRequest`] into a [`ReplayPermit`] backed by
//! one frozen [`RegistrySnapshot`], and runs the prepared handler on invoke.
//!
//! `prepare` is the fail-closed eligibility gate (contract C4/C5/C6): it
//! refuses on `effective_input == None`, on a missing stored identity, on a
//! closed registry, on an unknown tool name, and on any policy/schema/
//! fingerprint drift — reading handler and descriptor from a single
//! [`ToolHandlerRegistry::snapshot_state`] so they can never be paired across
//! registry generations.

use futures::future::BoxFuture;

use crate::session::events::{now_ms, SessionEvent, TurnId};
use crate::session::replay::{
    ReplayInvoker, ReplayPermit, ReplayPrepare, ReplayPreparer, ReplayRequest,
};
use crate::sync_primitives::Arc;
use crate::tools::descriptor::{ReplayPolicy, ToolCallIdentity};
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::ToolHandlerRegistry;

/// Concrete [`ReplayPreparer`] for the orchestrator bridge.
pub struct ReplayAdapter {
    registry: Arc<ToolHandlerRegistry>,
}

impl ReplayAdapter {
    #[must_use]
    pub fn new(registry: Arc<ToolHandlerRegistry>) -> Self {
        Self { registry }
    }
}

impl ReplayPreparer for ReplayAdapter {
    fn prepare(&self, req: &ReplayRequest) -> ReplayPrepare {
        // C4: the effective-input gate is first — refuse before any snapshot or
        // eligibility read, so a `None` never reaches the registry.
        let Some(effective_input) = req.effective_input.clone() else {
            return ReplayPrepare::Refused;
        };
        let Some(stored) = req.stored_identity else {
            return ReplayPrepare::Refused;
        };
        let snapshot = self.registry.snapshot_state(); // C6: one snapshot
        if snapshot.is_closed() {
            return ReplayPrepare::Refused;
        }
        let Some(entry) = snapshot.entry(&req.tool_name) else {
            return ReplayPrepare::Refused;
        };
        let current = ToolCallIdentity::from_descriptor(&entry.descriptor);
        // C5: predicate parity with `boundary_repair::replay_decision`, plus the
        // `closed`/`effective_input` gates above.
        if stored.replay_policy != ReplayPolicy::Safe
            || current.replay_policy != ReplayPolicy::Safe
            || stored.schema_version != current.schema_version
            || stored.replay_contract_fingerprint != current.replay_contract_fingerprint
            || stored.replay_contract_fingerprint.is_none()
        // both Some + equal
        {
            return ReplayPrepare::Refused;
        }
        ReplayPrepare::Ready(ReplayPermit::new(Box::new(PermitInvoker {
            handler: entry.handler.clone(),
            call_id: req.call_id.clone(),
            turn_id: req.turn_id,
            effective_input,
        })))
    }
}

/// The bridge-private permit inner: a handler captured from the frozen
/// snapshot plus the call correlation needed to rebuild the outcome event.
struct PermitInvoker {
    handler: Arc<dyn ToolHandler>,
    call_id: String,
    turn_id: TurnId,
    effective_input: serde_json::Value,
}

impl ReplayInvoker for PermitInvoker {
    fn invoke(self: Box<Self>, _claim_token: String) -> BoxFuture<'static, SessionEvent> {
        Box::pin(async move {
            let PermitInvoker {
                handler,
                call_id,
                turn_id,
                effective_input,
            } = *self;
            let at = now_ms();
            match handler.invoke(effective_input).await {
                Ok(output) => SessionEvent::ToolResult {
                    turn_id,
                    call_id,
                    output,
                    at,
                },
                Err(e) => SessionEvent::ToolError {
                    turn_id,
                    call_id,
                    error: e.to_string(),
                    at,
                },
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::ToolOutput;
    use crate::tools::descriptor::{
        ImplementationContract, ToolCapabilityDescriptor, ToolDescriptorLookup, ToolKind,
        SCHEMA_VERSION,
    };
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
    use async_trait::async_trait;
    use serde_json::Value;
    use uuid::Uuid;

    struct FakeHandler {
        name: String,
        fail: bool,
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            if self.fail {
                return Err(ToolError::Execution {
                    name: self.name.clone(),
                    cause: "boom".into(),
                });
            }
            Ok(ToolOutput {
                value: serde_json::json!({"tool": self.name}),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata {
                    idempotent: false,
                    ..Default::default()
                },
            }
        }
    }

    fn desc(name: &str) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.into(),
            kind: ToolKind::Tool,
            schema_version: SCHEMA_VERSION,
            description: String::new(),
            input_schema: serde_json::json!({}),
            source: ToolSource::Builtin,
            replay_policy: ReplayPolicy::Unsafe,
            requires_confirmation: false,
            idempotent: false,
            concurrent_safe: false,
            max_duration_ms: None,
            revision: 0,
            implementation_contract: None,
        }
    }

    fn safe_desc(name: &str) -> ToolCapabilityDescriptor {
        let mut d = desc(name);
        d.replay_policy = ReplayPolicy::Safe;
        d.implementation_contract = Some(ImplementationContract {
            id: format!("test:{name}"),
            version: "1".into(),
        });
        d
    }

    fn handler(name: &str, fail: bool) -> Arc<dyn ToolHandler> {
        Arc::new(FakeHandler {
            name: name.into(),
            fail,
        })
    }

    /// Register one Safe tool with an audited implementation contract and
    /// return the registry plus its current durable identity.
    fn register_safe(name: &str) -> (Arc<ToolHandlerRegistry>, ToolCallIdentity) {
        let registry = ToolHandlerRegistry::new();
        registry
            .register(safe_desc(name), handler(name, false))
            .expect("register safe tool");
        let identity =
            ToolDescriptorLookup::tool_call_identity(&registry, name).expect("safe identity");
        (Arc::new(registry), identity)
    }

    fn request(
        name: &str,
        stored_identity: ToolCallIdentity,
        effective_input: Option<Value>,
    ) -> ReplayRequest {
        ReplayRequest {
            call_id: "call-1".into(),
            tool_name: name.into(),
            turn_id: Uuid::new_v4(),
            effective_input,
            stored_identity: Some(stored_identity),
        }
    }

    #[test]
    fn none_effective_input_is_refused_before_any_registry_read() {
        // Even a fully eligible Safe tool must be refused when `effective_input`
        // is `None`: the gate is the first statement of `prepare` (C4), ahead of
        // the snapshot and every eligibility read.
        let (registry, identity) = register_safe("safe_tool");
        let adapter = ReplayAdapter::new(registry);
        let req = request("safe_tool", identity, None);
        assert!(matches!(adapter.prepare(&req), ReplayPrepare::Refused));
    }

    #[tokio::test]
    async fn some_effective_input_on_safe_tool_is_ready_and_invokes() {
        let (registry, identity) = register_safe("safe_tool");
        let adapter = ReplayAdapter::new(registry);
        let req = request("safe_tool", identity, Some(serde_json::json!({"x": 1})));
        let turn_id = req.turn_id;

        let permit = match adapter.prepare(&req) {
            ReplayPrepare::Ready(p) => p,
            ReplayPrepare::Refused => panic!("expected Ready for a Safe, matching tool"),
        };

        // The permit is single-use: it is consumed here exactly once.
        match permit.invoke("claim-token".into()).await {
            SessionEvent::ToolResult {
                turn_id: t,
                call_id,
                output,
                ..
            } => {
                assert_eq!(t, turn_id);
                assert_eq!(call_id, "call-1");
                assert_eq!(output.value, serde_json::json!({"tool": "safe_tool"}));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn closed_registry_is_refused() {
        let (registry, identity) = register_safe("safe_tool");
        registry.close();
        let adapter = ReplayAdapter::new(registry);
        let req = request("safe_tool", identity, Some(serde_json::json!({})));
        assert!(matches!(adapter.prepare(&req), ReplayPrepare::Refused));
    }

    #[test]
    fn unknown_tool_name_is_refused() {
        let (registry, identity) = register_safe("safe_tool");
        let adapter = ReplayAdapter::new(registry);
        let req = request("missing", identity, Some(serde_json::json!({})));
        assert!(matches!(adapter.prepare(&req), ReplayPrepare::Refused));
    }

    #[test]
    fn unsafe_stored_policy_is_refused() {
        let (registry, identity) = register_safe("safe_tool");
        let adapter = ReplayAdapter::new(registry);
        let mut stored = identity;
        stored.replay_policy = ReplayPolicy::Unsafe;
        let req = request("safe_tool", stored, Some(serde_json::json!({})));
        assert!(matches!(adapter.prepare(&req), ReplayPrepare::Refused));
    }

    #[test]
    fn schema_version_mismatch_is_refused() {
        let (registry, identity) = register_safe("safe_tool");
        let adapter = ReplayAdapter::new(registry);
        let mut stored = identity;
        stored.schema_version += 1;
        let req = request("safe_tool", stored, Some(serde_json::json!({})));
        assert!(matches!(adapter.prepare(&req), ReplayPrepare::Refused));
    }

    #[test]
    fn fingerprint_drift_is_refused() {
        let (registry, _identity) = register_safe("safe_tool");
        let adapter = ReplayAdapter::new(registry);
        // A different audited implementation contract yields a different
        // fingerprint, so the stored identity must refuse even though policy and
        // schema still match.
        let drifted = ToolCallIdentity::from_descriptor(&ToolCapabilityDescriptor {
            implementation_contract: Some(ImplementationContract {
                id: "test:other".into(),
                version: "2".into(),
            }),
            ..safe_desc("safe_tool")
        });
        let req = request("safe_tool", drifted, Some(serde_json::json!({})));
        assert!(matches!(adapter.prepare(&req), ReplayPrepare::Refused));
    }

    #[tokio::test]
    async fn invoke_maps_ok_to_tool_result_and_err_to_tool_error() {
        let turn_id = Uuid::new_v4();

        let ok_invoker = PermitInvoker {
            handler: handler("t", false),
            call_id: "c-ok".into(),
            turn_id,
            effective_input: serde_json::json!({}),
        };
        match Box::new(ok_invoker).invoke("token".into()).await {
            SessionEvent::ToolResult {
                turn_id: t,
                call_id,
                output,
                ..
            } => {
                assert_eq!(t, turn_id);
                assert_eq!(call_id, "c-ok");
                assert_eq!(output.value, serde_json::json!({"tool": "t"}));
            }
            _ => panic!("expected ToolResult"),
        }

        let err_invoker = PermitInvoker {
            handler: handler("t", true),
            call_id: "c-err".into(),
            turn_id,
            effective_input: serde_json::json!({}),
        };
        match Box::new(err_invoker).invoke("token".into()).await {
            SessionEvent::ToolError {
                turn_id: t,
                call_id,
                error,
                ..
            } => {
                assert_eq!(t, turn_id);
                assert_eq!(call_id, "c-err");
                assert!(error.contains("boom"), "error body: {error}");
            }
            _ => panic!("expected ToolError"),
        }
    }
}
