use super::facade::{
    BackendSourceEvidence, CapabilityChange, CapabilityChangeStream, CapabilitySnapshot, Cursor,
    Reference, Zahir,
};
use super::ownership::{LifetimeScope, OwnerRef, OwnershipTree, VisibilityScope};
use super::provider::{ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport};
use super::special::{
    AdmittedSpecialCall, CapabilityDispatchError, CapabilityHandler, CapabilityHandlerCall,
    CapabilityHandlerRegistry, SpecialRegistration,
};
use super::special_dispatch::CapabilityInvocationOrigin;
use super::tool_adapters::{
    TERMINAL_ADAPTER_ID, UI_ADAPTER_ID, register_canonical_adapters,
    register_canonical_adapters_with_tools,
};
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacets, CapabilityId, CapabilityKind, CapabilityRevision,
    ProjectionMetadata, SchemaRef,
};
use crate::capability::policy_runtime::{CapabilityCallContext, ResourceUse};
use crate::sync_primitives::Arc;
use aleph_protocol::capability::{
    MAX_PTY_INPUT_BYTES, PtyInvokeParams, PtyOperation, UiInvokeParams, UiOperation,
};
use async_trait::async_trait;
use schemars::schema_for;
use serde_json::{Value, json};
use sha2::Digest;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

fn provider() -> ProviderHandle {
    ProviderHandle::new(ProviderLifecycle::ready(
        ProviderId::new("special-test").expect("non-empty provider id"),
        ProviderTransport::InProcess,
    ))
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"value": {"type": "integer"}},
        "required": ["value"],
        "additionalProperties": false
    })
}

fn descriptor(schema: &Value) -> CapabilityDescriptor {
    let bytes = super::special::canonical_schema_bytes(schema);
    let mut fingerprint = [0_u8; 32];
    fingerprint.copy_from_slice(sha2::Sha256::digest(&bytes).as_slice());
    CapabilityDescriptor {
        id: CapabilityId {
            namespace: "aleph/test".into(),
            name: "special".into(),
        },
        kind: CapabilityKind::Terminal,
        schema: SchemaRef {
            version: 1,
            fingerprint,
        },
        revision: CapabilityRevision(0),
        lifetime: LifetimeScope::Runtime,
        visibility: VisibilityScope::default(),
        owner: OwnerRef::Runtime,
        services: Vec::new(),
        metadata: ProjectionMetadata::default(),
        facets: CapabilityFacets::new().with(super::descriptor::CapabilityFacet::Callable),
    }
}

struct CountingHandler {
    writes: Arc<AtomicUsize>,
}

#[async_trait]
impl CapabilityHandler for CountingHandler {
    async fn invoke(
        &self,
        input: Value,
        _call: CapabilityHandlerCall,
        _cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(input)
    }
}

fn admitted(capture: super::special::CapturedSpecialEntry) -> AdmittedSpecialCall {
    AdmittedSpecialCall {
        capture,
        input: json!({"value": 1}),
        call: CapabilityHandlerCall::new(
            CapabilityCallContext::from_trusted_parts(
                OwnerRef::Runtime,
                None,
                None,
                None,
                None,
                "special-test-call".into(),
            ),
            Vec::new(),
        ),
        cancel: CancellationToken::new(),
    }
}

#[tokio::test]
async fn special_capture_schema_and_aba_reach_real_handler() {
    let registry = CapabilityHandlerRegistry::new();
    let schema = schema();
    let writes = Arc::new(AtomicUsize::new(0));
    let id = registry
        .register(SpecialRegistration::new(
            descriptor(&schema),
            schema.clone(),
            Arc::new(CountingHandler {
                writes: Arc::clone(&writes),
            }),
            provider(),
        ))
        .expect("register special record");
    let capture = registry.capture(&id).expect("capture");
    assert_eq!(capture.schema(), &schema);
    assert_eq!(
        capture.canonical_schema(),
        super::special::canonical_schema_bytes(&schema)
    );
    registry
        .dispatch_admitted(admitted(capture.clone()))
        .await
        .expect("admitted call reaches handler");
    assert_eq!(writes.load(Ordering::SeqCst), 1);

    registry.unregister(&id).expect("unregister");
    registry
        .register(SpecialRegistration::new(
            descriptor(&schema),
            schema,
            Arc::new(CountingHandler {
                writes: Arc::clone(&writes),
            }),
            provider(),
        ))
        .expect("re-register same id");
    assert!(registry.dispatch_admitted(admitted(capture)).await.is_err());
    assert_eq!(writes.load(Ordering::SeqCst), 1);
}

#[test]
fn stale_special_registration_handle_cannot_remove_replacement() {
    let registry = CapabilityHandlerRegistry::new();
    let schema = schema();
    let old = registry
        .register_with_handle(SpecialRegistration::new(
            descriptor(&schema),
            schema.clone(),
            Arc::new(CountingHandler {
                writes: Arc::new(AtomicUsize::new(0)),
            }),
            provider(),
        ))
        .expect("initial handle");
    assert!(old.dispose());
    let replacement = registry
        .register_with_handle(SpecialRegistration::new(
            descriptor(&schema),
            schema,
            Arc::new(CountingHandler {
                writes: Arc::new(AtomicUsize::new(0)),
            }),
            provider(),
        ))
        .expect("replacement handle");
    assert!(!old.dispose());
    assert!(registry.capture(replacement.id()).is_some());
    assert!(replacement.dispose());
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn mounted_facade_admits_and_invokes_only_current_special_capture() {
    let registry = CapabilityHandlerRegistry::new();
    let tools = crate::tools::registry::ToolHandlerRegistry::new();
    let tree = Arc::new(OwnershipTree::new());
    let policy = Arc::new(
        super::policy::PolicyEngine::with_shared_tree(Arc::clone(&tree), Default::default())
            .expect("test policy"),
    );
    let facade = super::zahir_facade::ZahirFacade::new_with_specials_and_policy(
        tools.clone(),
        Arc::clone(&tree),
        registry.clone(),
        Arc::clone(&policy),
    );
    let schema = schema();
    let writes = Arc::new(AtomicUsize::new(0));
    let manager = crate::gateway::pty::manager();
    let generation = manager
        .activate(Arc::new(crate::gateway::event_bus::GatewayEventBus::new()))
        .expect("activate actual test manager");
    let registration = || {
        SpecialRegistration::new(
            descriptor(&schema),
            schema.clone(),
            Arc::new(CountingHandler {
                writes: Arc::clone(&writes),
            }),
            manager.provider_handle(),
        )
    };
    let id = registry
        .register(registration())
        .expect("register special record");
    tree.register(
        id.clone(),
        OwnerRef::Runtime,
        LifetimeScope::Runtime,
        VisibilityScope::default(),
    )
    .expect("bind special owner");
    let reference = Reference {
        id: id.clone(),
        visibility: VisibilityScope::default(),
    };
    let lease = facade
        .resolve_special(&reference)
        .expect("resolve special lease");
    // Missing actual source must not reach the otherwise current handler.
    let missing = invoke_counted_special(&facade, &lease).await;
    let missing_writes = writes.load(Ordering::SeqCst);
    let config = Arc::new(tokio::sync::RwLock::new(crate::Config::default()));
    config.write().await.policies.terminal.enabled = true;
    let source = Arc::new(super::tool_adapters::TerminalCapabilitySource::new(
        registry.clone(),
        policy,
        Arc::downgrade(&facade),
        config,
    ));
    facade
        .bind_terminal_source(Arc::downgrade(&source))
        .expect("bind actual source");
    source
        .bind_manager_handle(manager.provider_handle())
        .expect("bind actual provider");
    source.bind_handler().expect("bind actual handler");
    super::tool_adapters::register_canonical_adapters_with_tools_and_source(
        &registry,
        Some(&tools),
        Some(&source),
    )
    .expect("register actual stable owner");
    facade.reconcile_registry_bindings();
    let result = invoke_counted_special(&facade, &lease).await;
    let current_writes = writes.load(Ordering::SeqCst);

    registry.unregister(&id).expect("withdraw old capture");
    registry
        .register(registration())
        .expect("replace same capability id");
    let stale = invoke_counted_special(&facade, &lease).await;
    let replacement = facade
        .resolve_special(&reference)
        .expect("resolve replacement");
    registry.unregister(&id).expect("withdraw replacement");
    let withdrawn = invoke_counted_special(&facade, &replacement).await;
    let final_writes = writes.load(Ordering::SeqCst);

    // Exercise actual Tool wire input at the owning validation boundary, not
    // merely schema/serde decoding. No immutable manager source slot is rebound.
    let session = manager
        .spawn_at(
            generation,
            &crate::gateway::pty::SpawnOptions {
                command: Some(if cfg!(windows) { "cmd.exe" } else { "cat" }.into()),
                ..Default::default()
            },
        )
        .expect("spawn owned byte-boundary PTY");
    source
        .register_session(&session.session_id, None)
        .expect("publish actual session");
    let service = special_terminal_tool_service(facade, tools);
    let valid_list = crate::approval::with_call_identity(
        Some(crate::approval::CallIdentity {
            turn_id: crate::session::events::TurnId::nil(),
            call_id: "special-valid-list".into(),
        }),
        crate::tools::service::ToolService::execute_with_cancel(
            &service,
            "terminal_capability",
            json!({"operation": "list"}),
            CancellationToken::new(),
        ),
    )
    .await;
    let invalid_list = crate::approval::with_call_identity(
        Some(crate::approval::CallIdentity {
            turn_id: crate::session::events::TurnId::nil(),
            call_id: "special-invalid-list".into(),
        }),
        crate::tools::service::ToolService::execute_with_cancel(
            &service,
            "terminal_capability",
            json!({"operation": "list", "unexpected": true}),
            CancellationToken::new(),
        ),
    )
    .await;
    let before = manager
        .attach_snapshot_at(generation, &session.session_id)
        .expect("before input");
    let oversized = "界".repeat(MAX_PTY_INPUT_BYTES / "界".len() + 1);
    let expected_error = PtyInvokeParams {
        session_id: session.session_id.clone(),
        operation: PtyOperation::Input {
            data: oversized.clone(),
            base64: false,
        },
    }
    .validate()
    .err();
    let overflow = crate::approval::with_call_identity(Some(crate::approval::CallIdentity {
        turn_id: crate::session::events::TurnId::nil(), call_id: "special-byte-limit".into(),
    }), crate::tools::service::ToolService::execute_with_cancel(
        &service, "terminal_capability",
        json!({"operation": "input", "session_id": session.session_id, "data": oversized, "base64": false}),
        CancellationToken::new(),
    )).await;
    let after = manager
        .attach_snapshot_at(generation, &session.session_id)
        .expect("after input");
    manager
        .close_at(generation, &session.session_id)
        .expect("close owned PTY");
    source.withdraw_session(&session.session_id);
    manager.deactivate().await.expect("settle actual manager");

    assert!(matches!(
        missing,
        Err(super::special_dispatch::CapabilityInvocationError::Unavailable)
    ));
    assert_eq!(missing_writes, 0);
    assert_eq!(
        result.expect("invoke admitted special"),
        json!({"value": 9})
    );
    assert_eq!(current_writes, 1);
    assert!(stale.is_err());
    assert!(withdrawn.is_err());
    assert_eq!(final_writes, 1);
    assert!(
        valid_list.is_ok(),
        "actual registered List remains callable: {valid_list:?}"
    );
    assert!(
        matches!(invalid_list, Err(crate::tools::service::ToolError::ValidationFailed { ref cause, .. })
        if cause.contains("unexpected")),
        "actual registered parser must reject extra List field: {invalid_list:?}"
    );
    assert!(
        expected_error.is_some(),
        "UTF-8 bytes exceed the owning limit"
    );
    assert!(
        matches!(overflow, Err(crate::tools::service::ToolError::Execution { ref cause, .. })
        if expected_error.is_some_and(|expected| cause.contains(expected))),
        "actual owning byte validation: {overflow:?}"
    );
    assert_eq!(
        serde_json::to_value(before).expect("before screen"),
        serde_json::to_value(after).expect("after screen"),
        "overflow must have zero PTY effect"
    );
}

async fn invoke_counted_special(
    facade: &super::zahir_facade::ZahirFacade,
    lease: &super::facade::BackendLease,
) -> Result<Value, super::special_dispatch::CapabilityInvocationError> {
    facade
        .invoke_special(
            lease,
            json!({"value": 9}),
            CapabilityHandlerCall::new(
                CapabilityCallContext::from_trusted_parts(
                    OwnerRef::Runtime,
                    None,
                    None,
                    None,
                    None,
                    "facade-special-test".into(),
                ),
                Vec::new(),
            ),
            CapabilityInvocationOrigin::CanonicalIngress,
            ResourceUse::Existing,
            CancellationToken::new(),
        )
        .await
}

fn special_terminal_tool_service(
    facade: Arc<super::zahir_facade::ZahirFacade>,
    registry: crate::tools::registry::ToolHandlerRegistry,
) -> crate::tools::scoped::ScopedToolService {
    let registry = Arc::new(registry);
    let mut inner = crate::tools::runtime::LoopToolRegistry::new();
    inner.bind_canonical_registry(Arc::clone(&registry), Arc::new(|_| true));
    assert!(inner.bind_canonical_facade(facade));
    for entry in registry.entries_snapshot().values() {
        inner.register(Box::new(
            crate::tools::adapters::McpRegistryTool::from_registry_entry(
                Arc::clone(&entry.handler),
                &entry.descriptor,
            ),
        ));
    }
    crate::tools::scoped::ScopedToolService::new(Arc::new(inner), Default::default())
        .with_confirmation(Arc::new(SpecialTestApproval))
}

struct SpecialTestApproval;

#[async_trait]
impl crate::sandbox::exec_approval::gate::ApprovalRequester for SpecialTestApproval {
    async fn request_approval(
        &self,
        _: &crate::sandbox::exec_approval::action::ApprovalAction,
    ) -> crate::sandbox::exec_approval::gate::ApprovalResponse {
        crate::sandbox::exec_approval::gate::ApprovalOutcome::Approved.into()
    }
}

#[tokio::test]
async fn canonical_adapters_are_discoverable_without_executor_dispatch() {
    let registry = CapabilityHandlerRegistry::new();
    let ids = register_canonical_adapters(&registry).expect("canonical adapters");
    assert_eq!(ids.len(), 2);
    let described = registry.describe(&Default::default());
    assert_eq!(described.len(), 2);
    for id in [TERMINAL_ADAPTER_ID, UI_ADAPTER_ID] {
        let (namespace, name) = id.rsplit_once('/').expect("canonical id");
        let capture = described
            .iter()
            .find(|entry| entry.id().namespace == namespace && entry.id().name == name)
            .expect("adapter is discoverable");
        assert!(
            !capture
                .schema()
                .as_object()
                .expect("object schema")
                .is_empty()
        );
        assert!(capture.schema().get("additionalProperties").is_some());
    }
    let terminal = registry
        .capture(&CapabilityId {
            namespace: "aleph/tools".into(),
            name: "terminal_capability".into(),
        })
        .expect("terminal adapter capture");
    assert!(matches!(
        registry.dispatch_admitted(admitted(terminal)).await,
        Err(CapabilityDispatchError::Unavailable)
    ));
}

#[test]
fn canonical_adapters_publish_to_the_canonical_tool_catalog() {
    let specials = CapabilityHandlerRegistry::new();
    let tools = crate::tools::registry::ToolHandlerRegistry::new();
    register_canonical_adapters_with_tools(&specials, Some(&tools))
        .expect("canonical adapters publish atomically");
    let snapshot = tools.descriptor_snapshot();
    for name in ["terminal_capability", "ui_capability"] {
        let descriptor = snapshot.get(name).expect("canonical catalog entry");
        assert_eq!(descriptor.name, name);
        assert!(matches!(
            descriptor.replay_policy,
            crate::tools::descriptor::ReplayPolicy::Unsafe
        ));
        assert!(!descriptor.idempotent);
        let expected = match name {
            "terminal_capability" => {
                serde_json::to_value(schema_for!(aleph_protocol::capability::PtyToolParams))
            }
            "ui_capability" => serde_json::to_value(schema_for!(UiInvokeParams)),
            _ => unreachable!("only canonical names in the table"),
        }
        .expect("exact typed Tool schema");
        assert_eq!(descriptor.input_schema, expected, "{name}");
        assert_eq!(
            descriptor.requires_confirmation,
            name == "terminal_capability"
        );
    }
    for input in [
        json!({"operation": "spawn", "params": {}}),
        json!({"operation": "list"}),
        json!({"operation": "input", "session_id": "s", "data": "界", "base64": false}),
        json!({"operation": "attach", "session_id": "s"}),
        json!({"operation": "resize", "session_id": "s", "rows": 24, "cols": 80}),
        json!({"operation": "close", "session_id": "s"}),
    ] {
        assert!(
            serde_json::from_value::<aleph_protocol::capability::PtyToolParams>(input.clone())
                .is_ok(),
            "{input}"
        );
        let mut extra = input;
        extra["unexpected"] = json!(true);
        assert!(
            serde_json::from_value::<aleph_protocol::capability::PtyToolParams>(extra.clone())
                .is_err(),
            "{extra}"
        );
    }
    let nested_extra = json!({"operation": "spawn", "params": {"unexpected": true}});
    assert!(
        serde_json::from_value::<aleph_protocol::capability::PtyToolParams>(nested_extra).is_err()
    );
    for input in [
        json!({"surface_id": "s", "mount_generation": 1, "operation": {"operation": "display", "text": "界"}}),
        json!({"surface_id": "s", "mount_generation": 1, "operation": {"operation": "input"}}),
    ] {
        assert!(
            serde_json::from_value::<UiInvokeParams>(input.clone()).is_ok(),
            "{input}"
        );
        let mut extra = input.clone();
        extra["unexpected"] = json!(true);
        assert!(
            serde_json::from_value::<UiInvokeParams>(extra.clone()).is_err(),
            "{extra}"
        );
        let mut nested_extra = input;
        nested_extra["operation"]["unexpected"] = json!(true);
        assert!(
            serde_json::from_value::<UiInvokeParams>(nested_extra.clone()).is_err(),
            "{nested_extra}"
        );
    }
}

#[test]
fn ordinary_tool_same_id_cannot_shadow_special_cache_or_resolve() {
    let specials = CapabilityHandlerRegistry::new();
    let tools = crate::tools::registry::ToolHandlerRegistry::new();
    register_canonical_adapters_with_tools(&specials, Some(&tools))
        .expect("canonical adapter and Tool registration");
    let tree = Arc::new(OwnershipTree::new());
    let special_id = CapabilityId {
        namespace: "aleph/tools".into(),
        name: "terminal_capability".into(),
    };
    tree.register(
        special_id.clone(),
        OwnerRef::Runtime,
        LifetimeScope::Runtime,
        VisibilityScope::default(),
    )
    .expect("special owner binding");
    let facade =
        super::zahir_facade::ZahirFacade::new_with_specials(tools.clone(), tree, specials.clone());
    let reference = Reference {
        id: special_id.clone(),
        visibility: VisibilityScope::default(),
    };
    let direct = facade
        .lookup_descriptor(&reference)
        .expect("direct lookup sees special");
    assert_eq!(direct.id, special_id);
    let (cached, cacheable) = facade
        .lookup_descriptor_for_cache(&reference)
        .expect("special cache entry");
    assert!(cacheable);
    assert_eq!(cached.id, special_id);
    assert_eq!(
        facade.descriptor_revision(&reference),
        Some(cached.revision)
    );
    let lease = facade
        .resolve(reference.clone())
        .expect("special resolve wins");
    assert!(matches!(
        lease.source,
        BackendSourceEvidence::Special { .. }
    ));
    specials.unregister(&special_id).expect("withdraw special");
    let ordinary = facade
        .lookup_descriptor(&reference)
        .expect("ordinary Tool is visible after special withdrawal");
    assert_eq!(ordinary.id, special_id);
    assert!(matches!(
        facade.resolve(reference),
        Ok(crate::capability::facade::BackendLease {
            source: BackendSourceEvidence::Tool { .. },
            ..
        })
    ));
    assert_eq!(tools.descriptor_snapshot().len(), 2);
}

#[test]
fn rollback_tokens_fail_closed_across_claim_rotate_and_replacement() {
    let visibility = VisibilityScope::default();
    let tree = OwnershipTree::new();

    let claimed = CapabilityId {
        namespace: "aleph/race".into(),
        name: "claimed".into(),
    };
    let token = tree
        .register_if_absent_with_token(
            claimed.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            visibility.clone(),
        )
        .expect("token registration")
        .expect("new binding token");
    assert!(
        tree.claim(
            &claimed,
            &visibility,
            OwnerRef::Runtime,
            "claim-race".into()
        )
        .is_some()
    );
    assert!(!tree.rollback_if_matches(&token));
    assert_eq!(
        tree.owner_of(&claimed, &visibility),
        Some(OwnerRef::Runtime)
    );

    let rotated = CapabilityId {
        namespace: "aleph/race".into(),
        name: "rotated".into(),
    };
    let token = tree
        .register_if_absent_with_token(
            rotated.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            visibility.clone(),
        )
        .expect("token registration")
        .expect("new binding token");
    tree.rotate_capability(&rotated).expect("rotate binding");
    assert!(!tree.rollback_if_matches(&token));
    assert_eq!(
        tree.owner_of(&rotated, &visibility),
        Some(OwnerRef::Runtime)
    );

    let replaced = CapabilityId {
        namespace: "aleph/race".into(),
        name: "replaced".into(),
    };
    let token = tree
        .register_if_absent_with_token(
            replaced.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            visibility.clone(),
        )
        .expect("token registration")
        .expect("new binding token");
    tree.register(
        replaced.clone(),
        OwnerRef::Task(super::ownership::TaskId("replacement".into())),
        LifetimeScope::Task,
        visibility.clone(),
    )
    .expect("replace binding");
    assert!(!tree.rollback_if_matches(&token));
    assert_eq!(
        tree.owner_of(&replaced, &visibility),
        Some(OwnerRef::Task(super::ownership::TaskId(
            "replacement".into()
        )))
    );
}

#[test]
fn canonical_schemas_state_utf8_byte_bounds_and_reject_unknown_fields() {
    let pty_schema = serde_json::to_value(schema_for!(PtyInvokeParams)).expect("PTY schema JSON");
    let ui_schema = serde_json::to_value(schema_for!(UiInvokeParams)).expect("UI schema JSON");
    for schema in [&pty_schema, &ui_schema] {
        assert_eq!(
            schema.get("additionalProperties"),
            Some(&Value::Bool(false))
        );
        let text = schema.to_string();
        assert!(text.contains("UTF-8 byte length must be at most 65536"));
        assert!(!text.contains("\"maxLength\":65536"));
    }

    let exact = "é".repeat(MAX_PTY_INPUT_BYTES / 2);
    assert!(
        serde_json::from_value::<PtyInvokeParams>(json!({
            "session_id": "session",
            "operation": {"operation": "input", "data": exact}
        }))
        .expect("wire decode reaches owning validator")
        .validate()
        .is_ok()
    );
    let oversized = "é".repeat((MAX_PTY_INPUT_BYTES / 2) + 1);
    assert!(
        serde_json::from_value::<PtyInvokeParams>(json!({
            "session_id": "session",
            "operation": {"operation": "input", "data": oversized}
        }))
        .expect("wire decode reaches owning validator")
        .validate()
        .is_err()
    );
    assert!(
        serde_json::from_value::<PtyInvokeParams>(json!({
            "session_id": "session",
            "unknown": true,
            "operation": {"operation": "attach"}
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<UiInvokeParams>(json!({
            "surface_id": "surface",
            "mount_generation": 1,
            "operation": {"operation": "display", "text": "ok", "unknown": true}
        }))
        .is_err()
    );
    assert!(matches!(PtyOperation::Attach.validate(), Ok(())));
    assert!(matches!(UiOperation::Input {}.validate(), Ok(())));
}

#[test]
fn metadata_cursor_and_audited_lazy_paths_are_unchanged() {
    let mut stream = CapabilityChangeStream::from_snapshot(CapabilitySnapshot {
        capabilities: Vec::new(),
        committed_cursor: Cursor(7),
    });
    let id = CapabilityId {
        namespace: "aleph/test".into(),
        name: "cursor".into(),
    };
    assert!(!stream.append(
        Cursor(7),
        CapabilityChange::Registered {
            id: id.clone(),
            revision: CapabilityRevision(1),
        },
    ));
    assert!(stream.append(
        Cursor(8),
        CapabilityChange::Registered {
            id,
            revision: CapabilityRevision(1),
        },
    ));
    assert_eq!(stream.committed_cursor, Cursor(8));
    assert_eq!(stream.changes.len(), 1);
}

#[test]
fn generic_same_id_metadata_cannot_mint_special_admission() {
    let registry = CapabilityHandlerRegistry::new();
    let schema = schema();
    let id = registry
        .register(SpecialRegistration::new(
            descriptor(&schema),
            schema,
            Arc::new(CountingHandler {
                writes: Arc::new(AtomicUsize::new(0)),
            }),
            provider(),
        ))
        .expect("register special");
    let capture = registry.capture(&id).expect("special capture");
    let generic = BackendSourceEvidence::Provider {
        provider_id: ProviderId::new("generic").expect("provider id"),
        provider_generation: 1,
    };
    assert!(matches!(generic, BackendSourceEvidence::Provider { .. }));
    assert_eq!(capture.id(), &id);
}
