//! Regression tests for actual capability observation and live delivery boundaries.
use crate::capability::facade::{Reference, Zahir};
use crate::capability::ownership::{OwnershipTree, VisibilityScope};
use crate::capability::policy_runtime::ResourceUse;
use crate::capability::special::CapabilityHandlerRegistry;
use crate::capability::zahir_facade::ZahirFacade;
use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::service::{ToolDefinition, ToolError, ToolOutput};

struct ObservationTool {
    definition: ToolDefinition,
    effect: Option<std::path::PathBuf>,
}
#[async_trait::async_trait]
impl ToolHandler for ObservationTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    async fn invoke(&self, _: Value) -> Result<ToolOutput, ToolError> {
        if let Some(path) = &self.effect {
            std::fs::write(path, b"actual canonical handler effect").map_err(|error| {
                ToolError::Execution {
                    name: self.definition.name.clone(),
                    cause: error.to_string(),
                }
            })?;
        }
        Ok(ToolOutput {
            value: json!({"secret_result": "must-not-be-audited"}),
            metadata: Default::default(),
        })
    }
}
fn register_observation_tool(
    registry: &ToolHandlerRegistry,
    name: &str,
) -> crate::tools::registry::RegistrationHandle {
    register_observation_tool_with_effect(registry, name, None)
}
fn register_observation_tool_with_effect(
    registry: &ToolHandlerRegistry,
    name: &str,
    effect: Option<std::path::PathBuf>,
) -> crate::tools::registry::RegistrationHandle {
    let definition: ToolDefinition = serde_json::from_value(json!({
        "name": name, "description": "Observation fixture", "input_schema": {"type":"object"}, "source": crate::tools::descriptor::ToolSource::Builtin
    })).unwrap();
    registry
        .register(
            ToolCapabilityDescriptor::from_definition(&definition, 0),
            Arc::new(ObservationTool { definition, effect }),
        )
        .unwrap()
}
fn observation_facade(
    root: &std::path::Path,
    registry: ToolHandlerRegistry,
    hub: Arc<InMemoryEventStream>,
) -> Arc<ZahirFacade> {
    let tree = Arc::new(OwnershipTree::new());
    let policy = crate::capability::production_init::build_capability_policy(
        &crate::Config::default(),
        tree.clone(),
    )
    .unwrap();
    let memory = crate::memory::runtime::MemoryRuntime::new(
        crate::memory::runtime::MemoryRuntimeConfig {
            metadata_path: root.join("memory.db"),
            note_memory_dir: root.join("notes"),
        },
        tree.clone(),
    );
    ZahirFacade::new_with_memory_and_policy_and_specials_and_events(
        registry,
        tree,
        memory,
        policy,
        CapabilityHandlerRegistry::new(),
        hub,
    )
}
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::capability::descriptor::CapabilityId;
use crate::capability::event_audience::EventAudience;
use crate::capability::events::{
    CapabilityEvent, CapabilityEventStream, EventFilter, InMemoryEventStream,
};
use crate::capability::ownership::OwnerRef;
use crate::capability::policy_runtime::CapabilityCallContext;
use crate::gateway::capability_events::{CapabilityEventBridge, GatewayCapabilityEventAccess};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::event_scope::scope_for_role;
use crate::gateway::event_visibility::EventVisibilityIndex;
use crate::gateway::router::SessionKey;
use crate::gateway::server::ConnectionState;
use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
use crate::gateway::session_store::SessionStore;

fn event() -> CapabilityEvent {
    CapabilityEvent::CapabilityRevoked {
        id: CapabilityId {
            namespace: "aleph/tools".into(),
            name: "fixture".into(),
        },
        reason: "source_withdrawn".into(),
    }
}

#[test]
fn produced_audience_decoder_refuses_missing_and_malformed_authority() {
    let call = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some("alice".into()),
        Some(SessionKey::main("audience").to_key_string()),
        None,
        Some("conn-a".into()),
        "call-a".into(),
    );
    let audience = EventAudience::from_call(&call, None);
    let hub = InMemoryEventStream::new(8).unwrap();
    hub.emit_scoped(OwnerRef::Runtime, audience.clone(), event())
        .unwrap();
    let mut frame = serde_json::to_value(&hub.snapshot().unwrap().records()[0]).unwrap();
    assert_eq!(EventAudience::from_produced_frame(&frame), Some(audience));
    for invalid in [
        json!(null),
        json!({}),
        json!({"kind":"principal"}),
        json!({"kind":"principal","user_id":"","session_key":null,"workspace":null,"source_incarnation":null}),
        json!({"kind":"principal","user_id":"alice","session_key":23,"workspace":null,"source_incarnation":null}),
        json!({"kind":"operator_only","user_id":"alice"}),
    ] {
        frame["audience"] = invalid;
        assert!(EventAudience::from_produced_frame(&frame).is_none());
    }
}

#[tokio::test]
async fn current_connection_entitlement_guards_both_delivery_directions() {
    let temp = tempfile::TempDir::new().unwrap();
    let concrete = Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().to_owned(),
            ..Default::default()
        })
        .unwrap(),
    );
    let key = SessionKey::main("task7-current-user");
    crate::scope::with_scope(
        Some(crate::scope::ScopeAttribution::personal("alice")),
        concrete.get_or_create(&key),
    )
    .await
    .unwrap();
    let store: Arc<dyn SessionStore> = concrete;
    let visibility = Arc::new(EventVisibilityIndex::new());
    let mut state = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
    state.first_message = false;
    state.caller_user = Some("alice".into());
    state.caller_role = "operator".into();
    state.permissions = scope_for_role("operator");
    let connections = Arc::new(RwLock::new(HashMap::from([("conn-a".into(), state)])));
    let scope = GatewayCapabilityEventAccess::for_connection(
        "conn-a".into(),
        connections.clone(),
        Some(store.clone()),
        visibility.clone(),
    );
    let call = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some("alice".into()),
        Some(key.to_key_string()),
        None,
        Some("conn-a".into()),
        "call-a".into(),
    );
    let audience = EventAudience::from_call(&call, None);
    let hub = InMemoryEventStream::new(8).unwrap();
    hub.emit_scoped(OwnerRef::Runtime, audience.clone(), event())
        .unwrap();
    let record = hub.snapshot().unwrap().records()[0].clone();
    assert!(scope.admits(&record).await);
    for (user, role, expected) in [
        ("bob", "operator", false),
        ("alice", "operator", true),
        ("alice", "guest", false),
        ("alice", "member", true),
    ] {
        {
            let mut states = connections.write().await;
            let state = states.get_mut("conn-a").unwrap();
            state.caller_user = Some(user.into());
            state.caller_role = role.into();
            state.permissions = scope_for_role(role);
        }
        assert_eq!(scope.admits(&record).await, expected);
        assert_eq!(
            visibility
                .capability_audience_admits(&audience, Some(user), Some(role), &store)
                .await,
            expected
        );
    }
    let missing = GatewayCapabilityEventAccess::for_connection(
        "conn-a".into(),
        connections.clone(),
        None,
        visibility.clone(),
    );
    assert!(!missing.admits(&record).await);
    assert!(
        !visibility
            .capability_audience_admits(&EventAudience::OperatorOnly, Some("alice"), None, &store)
            .await
    );
    connections.write().await.remove("conn-a");
    assert!(!scope.admits(&record).await);
}

#[tokio::test]
async fn all_admissions_drain_even_without_configured_quota() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ToolHandlerRegistry::new();
    let facade = observation_facade(
        temp.path(),
        registry.clone(),
        Arc::new(InMemoryEventStream::default()),
    );
    let _registration = register_observation_tool(&registry, "drain_fixture");
    let lease = facade
        .resolve(Reference {
            id: CapabilityId {
                namespace: "aleph/tools".into(),
                name: "drain_fixture".into(),
            },
            visibility: VisibilityScope::default(),
        })
        .unwrap();
    let call = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some("alice".into()),
        None,
        None,
        None,
        "drain".into(),
    );
    let policy = facade.policy();
    assert!(!policy.is_configured(&lease.descriptor.id));
    let running = policy
        .admit_runtime(&lease, None, &call, ResourceUse::Existing)
        .unwrap();
    policy.stop_admission();
    assert!(policy
        .admit_runtime(&lease, None, &call, ResourceUse::Release)
        .is_err());
    let drained = policy.wait_for_idle();
    tokio::pin!(drained);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut drained)
            .await
            .is_err()
    );
    drop(running);
    tokio::time::timeout(Duration::from_secs(2), drained)
        .await
        .unwrap();
}

#[test]
fn contextual_resolve_and_prepare_refusals_are_observed_at_the_actual_boundary() {
    use crate::config::types::policies::capability_constraints::{
        CapabilityConstraintRule, CapabilityConstraintsConfig,
    };
    let tools = ToolHandlerRegistry::new();
    let tree = Arc::new(OwnershipTree::new());
    let id = CapabilityId {
        namespace: "aleph/tools".into(),
        name: "contextual_refusal".into(),
    };
    let mut constraints = CapabilityConstraintsConfig::new();
    constraints.insert(
        "aleph/tools/contextual_refusal".into(),
        CapabilityConstraintRule {
            required_conditions: std::collections::BTreeMap::from([(
                "provider_ready".into(),
                true,
            )]),
            ..Default::default()
        },
    );
    let policy = Arc::new(
        crate::capability::policy::PolicyEngine::with_shared_tree(tree.clone(), constraints)
            .unwrap(),
    );
    let facade = ZahirFacade::new_with_specials_and_policy(
        tools.clone(),
        tree.clone(),
        CapabilityHandlerRegistry::new(),
        policy.clone(),
    );
    let _registration = register_observation_tool(&tools, &id.name);
    let reference = Reference {
        id: id.clone(),
        visibility: VisibilityScope::default(),
    };
    let lease = facade.resolve(reference.clone()).unwrap();
    let capture = tools.capture_entry(&id.name).unwrap();
    let call = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some("alice".into()),
        None,
        None,
        None,
        "never-audit-this-call-id".into(),
    );
    assert_eq!(
        tree.revoke_bound(&id),
        crate::capability::ownership::BoundRevoke::Revoked
    );
    assert!(facade
        .prepare_tool_policy_for_call(&policy, &lease, &call)
        .is_err());
    assert!(facade
        .resolve_with_capture_for_call(reference, capture, &call)
        .is_err());
    let snapshot = facade.event_stream().snapshot().unwrap();
    assert_eq!(
        snapshot
            .records()
            .iter()
            .filter(|record| matches!(record.event(), CapabilityEvent::PolicyViolation { .. }))
            .count(),
        1
    );
    assert_eq!(snapshot.records().iter().filter(|record| matches!(record.event(), CapabilityEvent::ExecutionFailed { operation, reason, .. } if operation == "resolve" && reason == "source_withdrawn")).count(), 1);
    assert!(!serde_json::to_string(snapshot.records())
        .unwrap()
        .contains("never-audit-this-call-id"));
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn domain_adapter_cannot_skip_final_policy() {
    use crate::capability::special::CapabilityHandlerCall;
    use crate::capability::special_dispatch::CapabilityInvocationOrigin;
    use crate::capability::tool_adapters::{
        register_canonical_adapters_with_tools_and_source, TerminalCapabilitySource,
    };
    use crate::config::types::policies::capability_constraints::{
        CapabilityConstraintRule, CapabilityConstraintsConfig,
    };
    let specials = CapabilityHandlerRegistry::new();
    let tools = ToolHandlerRegistry::new();
    let tree = Arc::new(OwnershipTree::new());
    let mut constraints = CapabilityConstraintsConfig::new();
    constraints.insert(
        "aleph/tools/terminal_capability".into(),
        CapabilityConstraintRule {
            required_conditions: std::collections::BTreeMap::from([(
                "provider_ready".into(),
                false,
            )]),
            ..Default::default()
        },
    );
    let policy = Arc::new(
        crate::capability::policy::PolicyEngine::with_shared_tree(tree.clone(), constraints)
            .unwrap(),
    );
    let facade = ZahirFacade::new_with_specials_and_policy(
        tools.clone(),
        tree,
        specials.clone(),
        policy.clone(),
    );
    let config = Arc::new(RwLock::new(crate::Config::default()));
    config.write().await.policies.terminal.enabled = true;
    let source = Arc::new(TerminalCapabilitySource::new(
        specials.clone(),
        policy,
        Arc::downgrade(&facade),
        config,
    ));
    facade
        .bind_terminal_source(Arc::downgrade(&source))
        .unwrap();
    source.bind_handler().unwrap();
    register_canonical_adapters_with_tools_and_source(&specials, Some(&tools), Some(&source))
        .unwrap();
    let manager = crate::gateway::pty::manager();
    manager.activate(Arc::new(GatewayEventBus::new())).unwrap();
    source
        .bind_manager_handle(manager.provider_handle())
        .unwrap();
    let reference = Reference {
        id: CapabilityId {
            namespace: "aleph/tools".into(),
            name: "terminal_capability".into(),
        },
        visibility: VisibilityScope::default(),
    };
    // Canonical capture reconciles the same binding before the domain view is resolved.
    facade
        .resolve_with_capture(
            reference.clone(),
            tools.capture_entry("terminal_capability").unwrap(),
        )
        .unwrap();
    let lease = facade.resolve(reference).unwrap();
    let call = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some("alice".into()),
        None,
        None,
        None,
        "domain-denial".into(),
    );
    let result = facade
        .invoke_special(
            &lease,
            json!({"operation":"spawn", "params":{"command":"must-not-execute"}}),
            CapabilityHandlerCall::new(call, vec![]),
            CapabilityInvocationOrigin::GatewayDomain,
            ResourceUse::Allocate,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        result,
        Err(crate::capability::special_dispatch::CapabilityInvocationError::Policy(_))
    ));
    assert!(manager.list().is_empty());
    let snapshot = facade.event_stream().snapshot().unwrap();
    assert_eq!(
        snapshot
            .records()
            .iter()
            .filter(|record| matches!(record.event(), CapabilityEvent::PolicyViolation { .. }))
            .count(),
        1,
        "the actual final domain refusal is observed exactly once"
    );
    assert!(!serde_json::to_string(snapshot.records())
        .unwrap()
        .contains("must-not-execute"));
    manager.deactivate().await.unwrap();
}

#[tokio::test]
async fn diagnostic_dispose_uses_current_source_audience_and_deduplicates_withdrawal() {
    use crate::capability::diagnostic_control::DiagnosticControl;
    use crate::capability::ownership::LifetimeScope;
    use crate::capability::provider::{
        ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport,
    };
    use crate::capability::special::SpecialRegistration;
    use crate::capability::ui_surface::UiSurface;
    let temp = tempfile::tempdir().unwrap();
    let tree = Arc::new(OwnershipTree::new());
    let policy = crate::capability::production_init::build_capability_policy(
        &crate::Config::default(),
        tree.clone(),
    )
    .unwrap();
    let memory = crate::memory::runtime::MemoryRuntime::new(
        crate::memory::runtime::MemoryRuntimeConfig {
            metadata_path: temp.path().join("memory.db"),
            note_memory_dir: temp.path().join("notes"),
        },
        tree.clone(),
    );
    let specials = CapabilityHandlerRegistry::new();
    let hub = Arc::new(InMemoryEventStream::default());
    let host = crate::capability::projection_host::ProjectionHost::mount_with_runtime_and_events(
        ToolHandlerRegistry::new(),
        tree.clone(),
        memory,
        specials.clone(),
        policy,
        hub.clone(),
        true,
    );
    assert!(host.wait_until_ready().await);
    let control = DiagnosticControl::new(host.clone(), tree).unwrap();
    let id = CapabilityId {
        namespace: "aleph/ui".into(),
        name: "same-mount-id".into(),
    };
    let register = |human: &str, generation, bind: bool| {
        let call = CapabilityCallContext::from_trusted_parts(
            OwnerRef::Runtime,
            Some(human.into()),
            None,
            None,
            Some(format!("conn-{human}")),
            format!("mount-{human}"),
        );
        let attachment = aleph_protocol::capability::UiAttachment {
            surface_id: "same-mount-id".into(),
            mount_generation: generation,
        };
        let surface = Arc::new(UiSurface::new(attachment, call, Arc::new(|_| Ok(()))));
        let mut descriptor = crate::capability::ui::descriptor();
        descriptor.id = id.clone();
        descriptor.lifetime = LifetimeScope::Session;
        descriptor.visibility = VisibilityScope {
            principal: Some(human.into()),
            ..Default::default()
        };
        if bind {
            host.facade()
                .bind_terminal_session(id.clone(), descriptor.visibility.clone())
                .unwrap();
        }
        let provider = ProviderHandle::new(ProviderLifecycle::ready(
            ProviderId::new(format!("fixture/{human}")).unwrap(),
            ProviderTransport::InProcess,
        ));
        specials
            .register_with_handle(
                SpecialRegistration::new(
                    descriptor,
                    crate::capability::ui::schema(),
                    surface.clone(),
                    provider,
                )
                .with_ui_surface(surface),
            )
            .unwrap()
    };
    let old = register("alice", 1, true);
    let barrier = std::sync::Barrier::new(2);
    let current = std::thread::scope(|scope| {
        let replacement = scope.spawn(|| {
            host.facade()
                .bind_terminal_session(
                    id.clone(),
                    VisibilityScope {
                        principal: Some("bob".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            barrier.wait();
            assert!(old.dispose());
            register("bob", 2, false)
        });
        barrier.wait();
        control.dispose_runtime().unwrap();
        replacement.join().unwrap()
    });
    control.dispose_runtime().unwrap();
    let reference = Reference {
        id: id.clone(),
        visibility: VisibilityScope {
            principal: Some("bob".into()),
            ..Default::default()
        },
    };
    let before_metadata = hub.snapshot().unwrap().records().len();
    assert!(host.facade().resolve(reference.clone()).is_err());
    assert_eq!(
        hub.snapshot().unwrap().records().len(),
        before_metadata,
        "metadata resolve stays silent"
    );
    let bob = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some("bob".into()),
        None,
        None,
        Some("conn-bob".into()),
        "private-special-call-id".into(),
    );
    assert!(host
        .facade()
        .resolve_special_for_call(&reference, &bob)
        .is_err());
    let no_human = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        None,
        None,
        None,
        None,
        "not-human-authority".into(),
    );
    assert!(host
        .facade()
        .resolve_special_for_call(&reference, &no_human)
        .is_err());
    assert!(current.dispose());
    assert!(!current.dispose());
    let records = hub.snapshot().unwrap();
    let outcome = host.close_and_await().await;
    assert!(outcome.source_joined && outcome.applier_joined);
    let revoked = records.records().iter().filter(|record| matches!(record.event(), CapabilityEvent::CapabilityRevoked { id: event_id, .. } if event_id == &id)).map(|record| serde_json::to_value(record).unwrap()).collect::<Vec<_>>();
    assert_eq!(
        revoked.len(),
        2,
        "old physical withdrawal and current actual diagnostic mutation each emit once"
    );
    assert_eq!(revoked[0]["audience"]["user_id"], "alice");
    assert_eq!(revoked[1]["audience"]["user_id"], "bob");
    assert_ne!(
        revoked[0]["audience"]["source_incarnation"],
        revoked[1]["audience"]["source_incarnation"]
    );
    let failed: Vec<_> = records.records().iter().filter(|record| matches!(record.event(), CapabilityEvent::ExecutionFailed { operation, .. } if operation == "resolve")).map(|record| serde_json::to_value(record).unwrap()).collect();
    assert_eq!(
        failed.len(),
        1,
        "only the contextual attempt with a real human emits"
    );
    assert_eq!(failed[0]["audience"]["user_id"], "bob");
    assert_eq!(
        failed[0]["audience"]["source_incarnation"],
        revoked[1]["audience"]["source_incarnation"]
    );
    assert!(!serde_json::to_string(records.records())
        .unwrap()
        .contains("private-special-call-id"));
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn production_events_reach_zahir_ws_and_same_sequence_audit() {
    #[derive(Default)]
    struct Audit(std::sync::Mutex<Vec<crate::capability::events::CapabilityEventRecord>>);
    impl crate::capability::events::EventPersistence for Audit {
        fn persist(
            &self,
            record: &crate::capability::events::CapabilityEventRecord,
        ) -> Result<(), String> {
            self.0.lock().unwrap().push(record.clone());
            Ok(())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let audit = Arc::new(Audit::default());
    let hub = Arc::new(InMemoryEventStream::with_persistence(256, Some(audit.clone())).unwrap());
    let registry = ToolHandlerRegistry::new();
    let tree = Arc::new(OwnershipTree::new());
    let policy = crate::capability::production_init::build_capability_policy(
        &crate::Config::default(),
        tree.clone(),
    )
    .unwrap();
    let memory = crate::memory::runtime::MemoryRuntime::new(
        crate::memory::runtime::MemoryRuntimeConfig {
            metadata_path: temp.path().join("memory.db"),
            note_memory_dir: temp.path().join("notes"),
        },
        tree.clone(),
    );
    let specials = CapabilityHandlerRegistry::new();
    let facade = ZahirFacade::new_with_memory_and_policy_and_specials_and_events(
        registry.clone(),
        tree,
        memory.clone(),
        policy.clone(),
        specials.clone(),
        hub.clone(),
    );
    let bus = Arc::new(GatewayEventBus::new());
    let mut ws = bus.subscribe();
    let bridge = CapabilityEventBridge::start(hub.subscribe(EventFilter::default()), bus);
    let concrete = Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().join("sessions"),
            ..Default::default()
        })
        .unwrap(),
    );
    let store: Arc<dyn SessionStore> = concrete;
    let mut state = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
    state.first_message = false;
    state.caller_user = Some("alice".into());
    state.caller_role = "operator".into();
    state.permissions = scope_for_role("operator");
    let connections = Arc::new(RwLock::new(HashMap::from([(
        "real-operator".into(),
        state,
    )])));
    let scope = GatewayCapabilityEventAccess::for_connection(
        "real-operator".into(),
        connections,
        Some(store),
        Arc::new(EventVisibilityIndex::new()),
    );
    let mut typed = facade
        .subscribe_events(scope, EventFilter::default())
        .unwrap();
    let handler_effect = temp.path().join("canonical-handler-effect");
    let registration = register_observation_tool_with_effect(
        &registry,
        "observed_production_fixture",
        Some(handler_effect.clone()),
    );
    let canonical = Arc::new(registry.clone());
    let mut inner = crate::tools::runtime::LoopToolRegistry::new();
    inner.bind_canonical_registry(canonical.clone(), Arc::new(|_| true));
    assert!(inner.bind_canonical_facade(facade.clone()));
    let entry = registry
        .capture_entry("observed_production_fixture")
        .unwrap();
    inner.register(Box::new(
        crate::tools::adapters::McpRegistryTool::from_registry_entry(
            entry.entry().handler.clone(),
            &entry.entry().descriptor,
        ),
    ));
    let service = crate::tools::scoped::ScopedToolService::new(Arc::new(inner), Default::default());
    use crate::tools::service::ToolService;
    crate::gateway::caller_identity::with_caller_identity(
        Some("operator".into()),
        Some("alice".into()),
        true,
        Some("real-operator".into()),
        crate::approval::with_call_identity(
            Some(crate::approval::CallIdentity {
                turn_id: crate::session::events::TurnId::nil(),
                call_id: "private-canonical-call".into(),
            }),
            service.execute_with_cancel(
                "observed_production_fixture",
                json!({"secret_argument":"must-not-be-audited"}),
                tokio_util::sync::CancellationToken::new(),
            ),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(&handler_effect).unwrap(),
        b"actual canonical handler effect"
    );
    assert!(registration.dispose());
    assert!(!registration.dispose());

    memory.start().await.unwrap();
    memory.stop().await.unwrap();

    let config = Arc::new(RwLock::new(crate::Config::default()));
    config.write().await.policies.terminal.enabled = true;
    let source = Arc::new(
        crate::capability::tool_adapters::TerminalCapabilitySource::new(
            specials.clone(),
            policy,
            Arc::downgrade(&facade),
            config,
        ),
    );
    facade
        .bind_terminal_source(Arc::downgrade(&source))
        .unwrap();
    source.bind_handler().unwrap();
    crate::capability::tool_adapters::register_canonical_adapters_with_tools_and_source(
        &specials,
        Some(&registry),
        Some(&source),
    )
    .unwrap();
    let manager = crate::gateway::pty::manager();
    manager
        .bind_capability_source(Arc::downgrade(&source))
        .unwrap();
    let pty_bus = Arc::new(GatewayEventBus::new());
    let mut pty_frames = pty_bus.subscribe();
    let generation = manager.activate(pty_bus).unwrap();
    source
        .bind_manager_handle(manager.provider_handle())
        .unwrap();
    facade.reconcile_registry_bindings();
    let terminal = facade
        .resolve(Reference {
            id: CapabilityId {
                namespace: "aleph/tools".into(),
                name: "terminal_capability".into(),
            },
            visibility: VisibilityScope::default(),
        })
        .unwrap();
    let call = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some("alice".into()),
        None,
        None,
        Some("real-operator".into()),
        "real-pty".into(),
    );
    let child_effect = temp.path().join("actual-pty-child-effect");
    let spawn_params = if cfg!(windows) {
        json!({"command":"cmd.exe", "args":["/Q", "/K", format!("echo actual-child-effect>\"{}\" & echo actual-child-ready", child_effect.display())]})
    } else {
        json!({"command":"/bin/sh", "args":["-c", "printf actual-child-effect > \"$1\"; printf 'actual-child-ready\\n'; exec cat", "task7", child_effect]})
    };
    let spawned = crate::gateway::caller_identity::with_caller_identity(
        Some("operator".into()),
        Some("alice".into()),
        true,
        Some("real-operator".into()),
        facade.invoke_special(
            &terminal,
            json!({"operation":"spawn", "params":spawn_params}),
            crate::capability::special::CapabilityHandlerCall::new(call.clone(), vec![]),
            crate::capability::special_dispatch::CapabilityInvocationOrigin::GatewayDomain,
            ResourceUse::Allocate,
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await;
    if spawned.is_err() {
        manager.deactivate().await.unwrap();
    }
    let session = spawned.unwrap()["session_id"].as_str().unwrap().to_owned();
    let effect = tokio::time::timeout(Duration::from_secs(5), async {
        manager.write_at(generation, &session, b"opaque-pty-stream\n")?;
        loop {
            let frame = pty_frames.recv().await.map_err(|error| error.to_string())?;
            let frame: Value = serde_json::from_str(&frame).map_err(|error| error.to_string())?;
            if frame["topic"] == "pty.screen" && frame["data"]["session_id"] == session {
                let screen = manager.attach_snapshot_at(generation, &session)?;
                let text: String = screen
                    .patch
                    .rows
                    .iter()
                    .flat_map(|row| row.runs.iter().map(|run| run.text.as_str()))
                    .collect();
                // Kernel input echo alone is not evidence that the child ran.
                if text.contains("actual-child-ready") && text.contains("opaque-pty-stream") {
                    return Ok::<_, String>(
                        std::fs::read_to_string(&child_effect)
                            .map_err(|error| error.to_string())?
                            .trim()
                            == "actual-child-effect",
                    );
                }
            }
        }
    })
    .await;
    manager.close_at(generation, &session).unwrap();
    manager.deactivate().await.unwrap();
    assert!(
        effect.unwrap().unwrap(),
        "the actual child changed its screen and an external marker"
    );

    use crate::capability::provider::{
        ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport,
    };
    let ui_id = CapabilityId {
        namespace: "aleph/ui".into(),
        name: "observed-ui".into(),
    };
    let mut descriptor = crate::capability::ui::descriptor();
    descriptor.id = ui_id.clone();
    descriptor.lifetime = crate::capability::ownership::LifetimeScope::Session;
    descriptor.visibility = VisibilityScope {
        principal: Some("alice".into()),
        ..Default::default()
    };
    facade
        .bind_terminal_session(ui_id.clone(), descriptor.visibility.clone())
        .unwrap();
    let (shown, mut renders) = tokio::sync::mpsc::unbounded_channel();
    let surface = Arc::new(crate::capability::ui_surface::UiSurface::new(
        aleph_protocol::capability::UiAttachment {
            surface_id: "observed-ui".into(),
            mount_generation: 1,
        },
        call.clone(),
        Arc::new(move |message| {
            shown
                .send(message)
                .map_err(|_| crate::capability::ui::UiError::Closed)
        }),
    ));
    let provider = ProviderHandle::new(ProviderLifecycle::ready(
        ProviderId::new("observed/ui").unwrap(),
        ProviderTransport::InProcess,
    ));
    let mount = specials
        .register_with_handle(
            crate::capability::special::SpecialRegistration::new(
                descriptor.clone(),
                crate::capability::ui::schema(),
                surface.clone(),
                provider,
            )
            .with_ui_surface(surface.clone()),
        )
        .unwrap();
    let ui_lease = facade
        .resolve(Reference {
            id: ui_id,
            visibility: descriptor.visibility,
        })
        .unwrap();
    let (display, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            facade.invoke_special(
                &ui_lease,
                serde_json::to_value(aleph_protocol::capability::UiOperation::Display {
                    text: "opaque-ui-render".into()
                })
                .unwrap(),
                crate::capability::special::CapabilityHandlerCall::new(call, vec![]),
                crate::capability::special_dispatch::CapabilityInvocationOrigin::GatewayDomain,
                ResourceUse::Existing,
                tokio_util::sync::CancellationToken::new()
            ),
            async {
                let message = renders.recv().await.unwrap();
                assert_eq!(message.text, "opaque-ui-render");
                surface
                    .ack(
                        "real-operator",
                        aleph_protocol::capability::UiRenderAck {
                            surface_id: message.surface_id,
                            mount_generation: message.mount_generation,
                            request_id: message.request_id,
                            rendered: true,
                        },
                    )
                    .unwrap();
            }
        )
    })
    .await
    .unwrap();
    display.unwrap();
    surface.close().await.unwrap();
    assert!(mount.dispose());
    assert!(!mount.dispose());
    let retained = hub.snapshot().unwrap();
    for (namespace, name) in [
        ("aleph/tools", "observed_production_fixture".to_owned()),
        ("aleph/resources", "memory.runtime".to_owned()),
        ("aleph/terminal", format!("session-{session}")),
        ("aleph/ui", "observed-ui".to_owned()),
    ] {
        assert_eq!(retained.records().iter().filter(|record| matches!(record.event(), CapabilityEvent::CapabilityActivated { id, .. } if id.namespace == namespace && id.name == name)).count(), 1, "one actual source commit for {namespace}/{name}");
        assert_eq!(retained.records().iter().filter(|record| matches!(record.event(), CapabilityEvent::CapabilityRevoked { id, .. } if id.namespace == namespace && id.name == name)).count(), 1, "one actual source withdrawal for {namespace}/{name}");
    }
    assert!(retained.records().iter().any(|record| matches!(record.event(), CapabilityEvent::CapabilityInvoked { id, .. } if id.name == "observed_production_fixture")), "the actual canonical Tool result is observed");
    let audit_json = serde_json::to_string(retained.records()).unwrap();
    for private in [
        "must-not-be-audited",
        "opaque-pty-stream",
        "opaque-ui-render",
        "private-canonical-call",
        "actual-child-ready",
        "actual-child-effect",
    ] {
        assert!(
            !audit_json.contains(private),
            "payloads and call identifiers never enter capability audit"
        );
    }
    hub.close().unwrap();
    tokio::time::timeout(Duration::from_secs(2), bridge.join())
        .await
        .unwrap()
        .unwrap();
    let audit = audit.0.lock().unwrap().clone();
    assert_eq!(
        serde_json::to_value(retained.records()).unwrap(),
        serde_json::to_value(&audit).unwrap()
    );
    for expected in retained.records() {
        let record = tokio::time::timeout(Duration::from_secs(2), typed.recv())
            .await
            .unwrap()
            .unwrap();
        let frame: Value = serde_json::from_str(&ws.recv().await.unwrap()).unwrap();
        assert_eq!(*record, **expected);
        assert_eq!(frame["topic"], "capability.event");
        assert_eq!(frame["data"], serde_json::to_value(expected).unwrap());
        assert!(!frame.to_string().contains("must-not-be-audited"));
    }
}

mod ordinary_settlement {
    use super::*;
    use crate::tools::service::ToolService;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex};
    use tokio_util::sync::CancellationToken;

    const NAME: &str = "ordinary_settlement_fixture";

    #[derive(Default)]
    struct Audit(Mutex<Vec<crate::capability::events::CapabilityEventRecord>>);
    impl crate::capability::events::EventPersistence for Audit {
        fn persist(
            &self,
            record: &crate::capability::events::CapabilityEventRecord,
        ) -> Result<(), String> {
            self.0.lock().unwrap().push(record.clone());
            Ok(())
        }
    }

    // Every assertion path releases all possible blocking copies. A failed RED
    // must not strand a blocking pool thread at Tokio runtime shutdown.
    struct ReleaseCopies(mpsc::Sender<()>);
    impl ReleaseCopies {
        fn release_all(&self) {
            for _ in 0..16 {
                let _ = self.0.send(());
            }
        }
    }
    impl Drop for ReleaseCopies {
        fn drop(&mut self) {
            self.release_all();
        }
    }

    struct BlockingCopy {
        definition: ToolDefinition,
        source: std::path::PathBuf,
        root: std::path::PathBuf,
        release: Arc<Mutex<mpsc::Receiver<()>>>,
        entered: tokio::sync::mpsc::UnboundedSender<String>,
        settled: tokio::sync::mpsc::UnboundedSender<String>,
        scheduled: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl ToolHandler for BlockingCopy {
        fn definition(&self) -> ToolDefinition {
            self.definition.clone()
        }
        async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
            assert_eq!(
                crate::gateway::caller_identity::current_caller_user().as_deref(),
                Some("alice")
            );
            assert!(crate::approval::current_tool_call_id().is_some());
            let tag = input["effect"].as_str().unwrap().to_owned();
            let source = self.source.clone();
            let destination = self.root.join(&tag);
            let release = self.release.clone();
            let entered = self.entered.clone();
            let settled = self.settled.clone();
            self.scheduled.fetch_add(1, Ordering::SeqCst);
            tokio::task::spawn_blocking(move || {
                let _ = entered.send(tag.clone());
                let released = release.lock().unwrap().recv();
                let result = released.map_err(|error| error.to_string()).and_then(|()| {
                    std::fs::copy(source, destination).map_err(|error| error.to_string())
                });
                let _ = settled.send(tag);
                result
            })
            .await
            .map_err(|error| ToolError::Execution {
                name: NAME.into(),
                cause: error.to_string(),
            })?
            .map_err(|cause| ToolError::Execution {
                name: NAME.into(),
                cause,
            })?;
            Ok(ToolOutput {
                value: json!({"private_copy_result":"settled"}),
                metadata: Default::default(),
            })
        }
    }

    struct Fixture {
        root: tempfile::TempDir,
        service: Arc<crate::tools::scoped::ScopedToolService>,
        policy: Arc<crate::capability::policy::PolicyEngine>,
        hub: Arc<InMemoryEventStream>,
        audit: Arc<Audit>,
        registration: crate::tools::registry::RegistrationHandle,
        bridge: CapabilityEventBridge,
        release: ReleaseCopies,
        entered: tokio::sync::mpsc::UnboundedReceiver<String>,
        settled: tokio::sync::mpsc::UnboundedReceiver<String>,
        scheduled: Arc<AtomicUsize>,
        incarnation: String,
    }
    impl Fixture {
        fn new(quota: bool, bus: Arc<GatewayEventBus>) -> Self {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(
                root.path().join("source"),
                b"actual non-cancellable filesystem copy",
            )
            .unwrap();
            let tree = Arc::new(OwnershipTree::new());
            let config = if quota {
                toml::from_str(&format!("[policies.capability_constraints.\"aleph/tools/{NAME}\"]\nresource = {{ unit = \"active_calls\", max_active = 1 }}\n")).unwrap()
            } else {
                crate::Config::default()
            };
            let policy =
                crate::capability::production_init::build_capability_policy(&config, tree.clone())
                    .unwrap();
            let registry = ToolHandlerRegistry::new();
            let audit = Arc::new(Audit::default());
            let hub =
                Arc::new(InMemoryEventStream::with_persistence(128, Some(audit.clone())).unwrap());
            let bridge = CapabilityEventBridge::start(hub.subscribe(EventFilter::default()), bus);
            let memory = crate::memory::runtime::MemoryRuntime::new(
                crate::memory::runtime::MemoryRuntimeConfig {
                    metadata_path: root.path().join("memory.db"),
                    note_memory_dir: root.path().join("notes"),
                },
                tree.clone(),
            );
            let facade = ZahirFacade::new_with_memory_and_policy_and_specials_and_events(
                registry.clone(),
                tree,
                memory,
                policy.clone(),
                CapabilityHandlerRegistry::new(),
                hub.clone(),
            );
            let (release, receiver) = mpsc::channel();
            let (entered_tx, entered) = tokio::sync::mpsc::unbounded_channel();
            let (settled_tx, settled) = tokio::sync::mpsc::unbounded_channel();
            let scheduled = Arc::new(AtomicUsize::new(0));
            let definition: ToolDefinition = serde_json::from_value(json!({"name":NAME, "description":"Deterministically gated real blocking copy", "input_schema":{"type":"object"}, "source":crate::tools::descriptor::ToolSource::Builtin, "metadata":{"concurrent_safe":true}})).unwrap();
            let registration = registry
                .register(
                    ToolCapabilityDescriptor::from_definition(&definition, 0),
                    Arc::new(BlockingCopy {
                        definition,
                        source: root.path().join("source"),
                        root: root.path().to_owned(),
                        release: Arc::new(Mutex::new(receiver)),
                        entered: entered_tx,
                        settled: settled_tx,
                        scheduled: scheduled.clone(),
                    }),
                )
                .unwrap();
            let entry = registry.capture_entry(NAME).unwrap();
            let incarnation = entry
                .entry()
                .observation
                .as_ref()
                .unwrap()
                .incarnation()
                .to_owned();
            let mut inner = crate::tools::runtime::LoopToolRegistry::new();
            inner.bind_canonical_registry(Arc::new(registry.clone()), Arc::new(|_| true));
            assert!(inner.bind_canonical_facade(facade));
            inner.register(Box::new(
                crate::tools::adapters::McpRegistryTool::from_registry_entry(
                    entry.entry().handler.clone(),
                    &entry.entry().descriptor,
                ),
            ));
            let service = Arc::new(crate::tools::scoped::ScopedToolService::new(
                Arc::new(inner),
                Default::default(),
            ));
            Self {
                root,
                service,
                policy,
                hub,
                audit,
                registration,
                bridge,
                release: ReleaseCopies(release),
                entered,
                settled,
                scheduled,
                incarnation,
            }
        }
        fn start(
            &self,
            tag: &str,
            cancel: CancellationToken,
        ) -> tokio::task::JoinHandle<Result<ToolOutput, ToolError>> {
            let service = self.service.clone();
            let tag = tag.to_owned();
            tokio::spawn(async move {
                crate::gateway::caller_identity::with_caller_identity(
                    Some("operator".into()),
                    Some("alice".into()),
                    true,
                    Some("ordinary-owner".into()),
                    crate::approval::with_call_identity(
                        Some(crate::approval::CallIdentity {
                            turn_id: crate::session::events::TurnId::nil(),
                            call_id: format!("private-copy-{tag}"),
                        }),
                        service.execute_with_cancel(NAME, json!({"effect":tag}), cancel),
                    ),
                )
                .await
            })
        }
        fn terminals(&self) -> Vec<Arc<crate::capability::events::CapabilityEventRecord>> {
            self.hub.snapshot().unwrap().records().iter().filter(|record| matches!(record.event(), CapabilityEvent::CapabilityInvoked { id, .. } | CapabilityEvent::ExecutionFailed { id, .. } if id.name == NAME)).cloned().collect()
        }
        async fn release_started(&mut self) {
            self.release.release_all();
            for _ in 0..self.scheduled.load(Ordering::SeqCst) {
                tokio::time::timeout(Duration::from_secs(2), self.settled.recv())
                    .await
                    .unwrap()
                    .unwrap();
            }
            tokio::time::timeout(Duration::from_secs(2), self.policy.wait_for_idle())
                .await
                .unwrap();
        }
        async fn seal(&self) -> Vec<Arc<crate::capability::events::CapabilityEventRecord>> {
            self.policy.stop_admission();
            tokio::time::timeout(Duration::from_secs(2), self.policy.wait_for_idle())
                .await
                .unwrap();
            let _ = self.registration.dispose();
            self.hub.close().unwrap();
            tokio::time::timeout(Duration::from_secs(2), self.bridge.clone().join())
                .await
                .unwrap()
                .unwrap();
            let records = self.hub.snapshot().unwrap().records().to_vec();
            assert_eq!(
                serde_json::to_value(&records).unwrap(),
                serde_json::to_value(&*self.audit.0.lock().unwrap()).unwrap()
            );
            records
        }
    }

    async fn cancellation_does_not_settle_started_copy(token_cancel: bool) {
        let bus = Arc::new(GatewayEventBus::new());
        let mut frames = bus.subscribe();
        let mut fixture = Fixture::new(false, bus);
        let cancel = CancellationToken::new();
        let caller = fixture.start("first", cancel.clone());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), fixture.entered.recv())
                .await
                .unwrap()
                .as_deref(),
            Some("first")
        );
        let caller_cancelled = if token_cancel {
            cancel.cancel();
            matches!(
                tokio::time::timeout(Duration::from_secs(2), caller)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(ToolError::Cancelled { .. })
            )
        } else {
            caller.abort();
            caller.await.unwrap_err().is_cancelled()
        };
        let terminals_before_release = fixture.terminals().len();
        let effect_before_release = fixture.root.path().join("first").exists();
        fixture.policy.stop_admission();
        let idle_waited =
            tokio::time::timeout(Duration::from_millis(10), fixture.policy.wait_for_idle())
                .await
                .is_err();
        let stopped = fixture
            .start("post-stop", CancellationToken::new())
            .await
            .unwrap();
        // Cleanup precedes every assertion of the bug facts, including RED.
        fixture.release_started().await;
        let copied = std::fs::read(fixture.root.path().join("first")).unwrap();
        let terminal = fixture.terminals();
        let records = fixture.seal().await;
        let mut forwarded = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            let frame: Value = serde_json::from_str(&frame).unwrap();
            if frame["topic"] == "capability.event" {
                forwarded.push(frame["data"].clone());
            }
        }
        assert_eq!(
            forwarded,
            records
                .iter()
                .map(|record| serde_json::to_value(record).unwrap())
                .collect::<Vec<_>>()
        );
        assert!(caller_cancelled);
        assert!(!effect_before_release);
        assert_eq!(copied, b"actual non-cancellable filesystem copy");
        assert!(matches!(stopped, Err(ToolError::PermissionDenied { .. })));
        assert!(!fixture.root.path().join("post-stop").exists());
        assert!(idle_waited, "STOP/drain must retain the started ordinary filesystem effect after caller cancellation");
        assert_eq!(
            terminals_before_release, 0,
            "caller disinterest is not actual handler settlement"
        );
        assert_eq!(
            terminal.len(),
            1,
            "one actual completion remains owned after caller cancellation"
        );
        assert!(matches!(
            terminal[0].event(),
            CapabilityEvent::CapabilityInvoked { .. }
        ));
        let value = serde_json::to_value(&terminal[0]).unwrap();
        assert_eq!(value["audience"]["source_incarnation"], fixture.incarnation);
        assert_eq!(value["audience"]["user_id"], "alice");
        assert!(!serde_json::to_string(&records)
            .unwrap()
            .contains("private-copy"));
        assert!(!serde_json::to_string(&records)
            .unwrap()
            .contains("private_copy_result"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn caller_abort_keeps_started_filesystem_effect_owned_until_settlement() {
        cancellation_does_not_settle_started_copy(false).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn token_cancel_does_not_drop_started_handler_or_release_stop_drain() {
        cancellation_does_not_settle_started_copy(true).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configured_active_slot_is_not_released_by_caller_abort() {
        let mut fixture = Fixture::new(true, Arc::new(GatewayEventBus::new()));
        let caller = fixture.start("first", CancellationToken::new());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), fixture.entered.recv())
                .await
                .unwrap()
                .as_deref(),
            Some("first")
        );
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        let mut candidate = fixture.start("candidate", CancellationToken::new());
        let denial = tokio::time::timeout(Duration::from_millis(50), &mut candidate).await;
        let denied_for_occupied_slot = matches!(&denial, Ok(Ok(Err(ToolError::PermissionDenied { reason, .. }))) if reason.contains("capability constraint denied: runtime_constraints"));
        let constraint_violation = fixture.hub.snapshot().unwrap().records().iter().any(|record| {
            matches!(record.event(), CapabilityEvent::ConstraintViolation { id, .. } if id.name == NAME)
        });
        let denial_diagnostic = format!(
            "second_ingress={denial:?}; constraint_violation={constraint_violation}; scheduled_before_release={}; terminals_before_release={}",
            fixture.scheduled.load(Ordering::SeqCst),
            fixture.terminals().len(),
        );
        if denial.is_err() {
            candidate.abort();
            let _ = candidate.await;
        }
        // The failed implementation can start candidate copy work; release all
        // scheduled work even when the denial is absent before asserting RED.
        fixture.release_started().await;
        let candidate_effect = fixture.root.path().join("candidate").exists();
        let first = std::fs::read(fixture.root.path().join("first")).unwrap();
        let healthy = fixture
            .start("healthy", CancellationToken::new())
            .await
            .unwrap();
        let healthy_effect = std::fs::read(fixture.root.path().join("healthy")).unwrap();
        let records = fixture.seal().await;
        assert_eq!(first, b"actual non-cancellable filesystem copy");
        assert!(healthy.is_ok());
        assert_eq!(healthy_effect, first);
        assert!(denied_for_occupied_slot, "a real second ingress must see the configured active_calls constraint, not free occupancy, STOP or generic unavailability: {denial_diagnostic}");
        assert!(constraint_violation, "the configured resource constraint must emit its typed violation event: {denial_diagnostic}");
        assert!(
            !candidate_effect,
            "denied candidate must have zero filesystem effect"
        );
        assert_eq!(records.iter().filter(|record| matches!(record.event(), CapabilityEvent::CapabilityInvoked { id, .. } if id.name == NAME)).count(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pre_cancelled_canonical_call_starts_no_filesystem_effect() {
        let fixture = Fixture::new(false, Arc::new(GatewayEventBus::new()));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = fixture.start("pre-cancelled", cancel).await.unwrap();
        let records = fixture.seal().await;
        assert!(matches!(result, Err(ToolError::Cancelled { .. })));
        assert_eq!(fixture.scheduled.load(Ordering::SeqCst), 0);
        assert!(!fixture.root.path().join("pre-cancelled").exists());
        assert!(!records.iter().any(|record| matches!(record.event(), CapabilityEvent::CapabilityInvoked { id, .. } if id.name == NAME)));
    }
}

#[tokio::test]
async fn cancelled_memory_caller_keeps_actual_sqlite_completion_before_source_withdrawal() {
    struct MemorySqliteTool {
        definition: ToolDefinition,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl ToolHandler for MemorySqliteTool {
        fn definition(&self) -> ToolDefinition {
            self.definition.clone()
        }
        async fn invoke(&self, _: Value) -> Result<ToolOutput, ToolError> {
            let operation =
                crate::memory::runtime::MemoryOperation::current().ok_or_else(|| {
                    ToolError::PermissionDenied {
                        name: self.definition.name.clone(),
                        reason: "missing actual Memory-owned operation".into(),
                    }
                })?;
            self.entered.notify_one();
            self.release.notified().await;
            operation.backend().with_conn(|connection| {
                connection.execute_batch("CREATE TABLE task7_owned_effect(value INTEGER); INSERT INTO task7_owned_effect VALUES (1)")
                    .map_err(|error| crate::error::AlephError::other(error.to_string()))?;
                Ok(())
            }).await.map_err(|error| ToolError::Execution { name: self.definition.name.clone(), cause: error.to_string() })?;
            Ok(ToolOutput {
                value: json!({"private_sqlite_result":"persisted"}),
                metadata: Default::default(),
            })
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let tree = Arc::new(OwnershipTree::new());
    let policy = crate::capability::production_init::build_capability_policy(
        &crate::Config::default(),
        tree.clone(),
    )
    .unwrap();
    let memory = crate::memory::runtime::MemoryRuntime::new(
        crate::memory::runtime::MemoryRuntimeConfig {
            metadata_path: temp.path().join("memory.db"),
            note_memory_dir: temp.path().join("notes"),
        },
        tree.clone(),
    );
    let registry = ToolHandlerRegistry::new();
    let hub = Arc::new(InMemoryEventStream::default());
    let facade = ZahirFacade::new_with_memory_and_policy_and_specials_and_events(
        registry.clone(),
        tree,
        memory.clone(),
        policy.clone(),
        CapabilityHandlerRegistry::new(),
        hub.clone(),
    );
    memory.start().await.unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let definition: ToolDefinition = serde_json::from_value(json!({
        "name":"memory_owned_fixture", "description":"Actual Memory-owned SQLite completion", "input_schema":{"type":"object"}, "source":crate::tools::descriptor::ToolSource::Builtin,
        "metadata":{"services":[{"relation":"requires", "target":{"namespace":"aleph/resources", "name":"memory.runtime"}}]}
    })).unwrap();
    let registration = registry
        .register(
            ToolCapabilityDescriptor::from_definition(&definition, 0),
            Arc::new(MemorySqliteTool {
                definition,
                entered: entered.clone(),
                release: release.clone(),
            }),
        )
        .unwrap();
    let canonical = Arc::new(registry.clone());
    let mut inner = crate::tools::runtime::LoopToolRegistry::new();
    inner.bind_canonical_registry(canonical, Arc::new(|_| true));
    assert!(inner.bind_canonical_facade(facade));
    let entry = registry.capture_entry("memory_owned_fixture").unwrap();
    inner.register(Box::new(
        crate::tools::adapters::McpRegistryTool::from_registry_entry(
            entry.entry().handler.clone(),
            &entry.entry().descriptor,
        ),
    ));
    let service = crate::tools::scoped::ScopedToolService::new(Arc::new(inner), Default::default());
    use crate::tools::service::ToolService;
    let caller = tokio::spawn(async move {
        crate::gateway::caller_identity::with_caller_identity(
            Some("operator".into()),
            Some("alice".into()),
            true,
            Some("memory-connection".into()),
            crate::approval::with_call_identity(
                Some(crate::approval::CallIdentity {
                    turn_id: crate::session::events::TurnId::nil(),
                    call_id: "private-memory-call".into(),
                }),
                service.execute_with_cancel(
                    "memory_owned_fixture",
                    json!({"opaque_memory_argument":"secret"}),
                    tokio_util::sync::CancellationToken::new(),
                ),
            ),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    policy.stop_admission();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), policy.wait_for_idle())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(10), memory.stop())
            .await
            .is_err(),
        "source shutdown retains the original admitted SQLite work"
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), memory.stop())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), policy.wait_for_idle())
        .await
        .unwrap();
    let count: i64 = rusqlite::Connection::open(temp.path().join("memory.db"))
        .unwrap()
        .query_row("SELECT SUM(value) FROM task7_owned_effect", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        count, 1,
        "the real retained Memory backend committed SQLite exactly once"
    );
    assert!(registration.dispose());
    let snapshot = hub.snapshot().unwrap();
    let results = snapshot.records().iter().filter(|record| matches!(record.event(), CapabilityEvent::CapabilityInvoked { id, .. } if id.name == "memory_owned_fixture")).collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    let withdrawal = snapshot.records().iter().find(|record| matches!(record.event(), CapabilityEvent::CapabilityRevoked { id, .. } if id.namespace == "aleph/resources" && id.name == "memory.runtime")).unwrap();
    assert!(results[0].sequence() < withdrawal.sequence());
    let audit = serde_json::to_string(snapshot.records()).unwrap();
    for secret in [
        "private_sqlite_result",
        "private-memory-call",
        "opaque_memory_argument",
    ] {
        assert!(!audit.contains(secret));
    }
}

#[tokio::test]
async fn cancelled_ui_caller_keeps_owned_completion_and_close_releases_drain() {
    use crate::capability::provider::{
        ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport,
    };
    let tree = Arc::new(OwnershipTree::new());
    let policy = crate::capability::production_init::build_capability_policy(
        &crate::Config::default(),
        tree.clone(),
    )
    .unwrap();
    let specials = CapabilityHandlerRegistry::new();
    // Exact UI calls also pass the real runtime-owned canonical adapter gate.
    for adapter_id in crate::capability::tool_adapters::register_canonical_adapters(&specials)
        .expect("canonical special adapter registration")
    {
        tree.register(
            adapter_id,
            OwnerRef::Runtime,
            crate::capability::ownership::LifetimeScope::Runtime,
            VisibilityScope::default(),
        )
        .expect("canonical adapter ownership binding");
    }
    let facade = ZahirFacade::new_with_specials_and_policy(
        ToolHandlerRegistry::new(),
        tree,
        specials.clone(),
        policy.clone(),
    );
    let id = CapabilityId {
        namespace: "aleph/ui".into(),
        name: "owned-completion".into(),
    };
    let call = CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some("alice".into()),
        None,
        None,
        Some("conn-alice".into()),
        "private-ui-call-id".into(),
    );
    let attachment = aleph_protocol::capability::UiAttachment {
        surface_id: id.name.clone(),
        mount_generation: 1,
    };
    let (shown, mut messages) = tokio::sync::mpsc::unbounded_channel();
    let surface = Arc::new(crate::capability::ui_surface::UiSurface::new(
        attachment.clone(),
        call.clone(),
        Arc::new(move |message| {
            shown
                .send(message)
                .map_err(|_| crate::capability::ui::UiError::Closed)
        }),
    ));
    let mut descriptor = crate::capability::ui::descriptor();
    descriptor.id = id.clone();
    descriptor.lifetime = crate::capability::ownership::LifetimeScope::Session;
    descriptor.visibility = VisibilityScope {
        principal: Some("alice".into()),
        ..Default::default()
    };
    facade
        .bind_terminal_session(id.clone(), descriptor.visibility.clone())
        .unwrap();
    let provider = ProviderHandle::new(ProviderLifecycle::ready(
        ProviderId::new("owned/ui").unwrap(),
        ProviderTransport::InProcess,
    ));
    let mount = specials
        .register_with_handle(
            crate::capability::special::SpecialRegistration::new(
                descriptor.clone(),
                crate::capability::ui::schema(),
                surface.clone(),
                provider,
            )
            .with_ui_surface(surface.clone()),
        )
        .unwrap();
    let lease = facade
        .resolve(Reference {
            id: id.clone(),
            visibility: descriptor.visibility,
        })
        .unwrap();
    let caller_facade = facade.clone();
    let mut caller = tokio::spawn(async move {
        caller_facade
            .invoke_special(
                &lease,
                serde_json::to_value(aleph_protocol::capability::UiOperation::Display {
                    text: "opaque-ui-text".into(),
                })
                .unwrap(),
                crate::capability::special::CapabilityHandlerCall::new(call, vec![]),
                crate::capability::special_dispatch::CapabilityInvocationOrigin::GatewayDomain,
                ResourceUse::Existing,
                tokio_util::sync::CancellationToken::new(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            result = &mut caller => panic!("UI invocation returned before render delivery: {result:?}"),
            message = messages.recv() => {
                message.expect("the actual UI source must deliver a display frame");
            }
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    policy.stop_admission();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), policy.wait_for_idle())
            .await
            .is_err(),
        "the detached UI owner still retains the original admission"
    );
    surface.close().await.unwrap();
    surface.close().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), policy.wait_for_idle())
        .await
        .unwrap();
    assert!(surface
        .validate_attachment(
            "conn-alice",
            &attachment.surface_id,
            attachment.mount_generation
        )
        .is_err());
    assert!(mount.dispose());
    let snapshot = facade.event_stream().snapshot().unwrap();
    assert_eq!(snapshot.records().iter().filter(|record| matches!(record.event(), CapabilityEvent::ExecutionFailed { id: actual, .. } if actual == &id)).count(), 1);
    let audit = serde_json::to_string(snapshot.records()).unwrap();
    assert!(!audit.contains("opaque-ui-text") && !audit.contains("private-ui-call-id"));
}

#[tokio::test]
async fn idle_forwarder_drains_and_joins_after_producer_close() {
    for count in [0, 1, 12] {
        let hub = InMemoryEventStream::new(4).unwrap();
        let bus = Arc::new(GatewayEventBus::new());
        let mut frames = bus.subscribe();
        let receiver = hub.subscribe(EventFilter::default());
        let bridge = CapabilityEventBridge::start(receiver, bus);
        for _ in 0..count {
            hub.emit(OwnerRef::Runtime, event()).unwrap();
        }
        hub.close().unwrap();
        tokio::time::timeout(Duration::from_secs(2), bridge.clone().join())
            .await
            .unwrap()
            .unwrap();
        bridge.join().await.unwrap();
        let mut delivered = Vec::<Value>::new();
        while let Ok(frame) = frames.try_recv() {
            delivered.push(serde_json::from_str(&frame).unwrap());
        }
        let records = delivered
            .iter()
            .filter(|frame| frame["topic"] == "capability.event")
            .collect::<Vec<_>>();
        assert_eq!(records.len(), count.min(4));
        if count > 4 {
            assert!(delivered
                .iter()
                .any(|frame| frame["topic"] == "capability.event.status"
                    && frame["data"] == json!({"state":"lagged","incomplete":true})));
        }
        if count > 0 {
            assert_eq!(records.last().unwrap()["data"]["sequence"], count);
        }
    }
}
