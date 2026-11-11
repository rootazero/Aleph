use super::ui_surface::UiSurface;
use crate::acp::manager::SessionKey;
use crate::capability::ownership::OwnerRef;
use crate::capability::policy_runtime::CapabilityCallContext;
use crate::capability::special::CapabilityHandlerCall;
use crate::sync_primitives::Arc;
use aleph_protocol::capability::{
    UiAttachment, UiDisplayMessage, UiInputEvent, UiRenderAck, UiSubmitInput, MAX_UI_KEY_BYTES,
    MAX_UI_QUEUE, MAX_UI_TEXT_BYTES, UI_ACK_TIMEOUT_MS,
};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

fn owner(connection: &str, session: &str) -> CapabilityCallContext {
    CapabilityCallContext::from_trusted_parts(
        OwnerRef::Session(SessionKey::new("agent", session)),
        Some(format!("principal-{connection}")),
        Some(session.to_owned()),
        None,
        Some(connection.to_owned()),
        format!("call-{connection}"),
    )
}

fn call(connection: &str, session: &str) -> CapabilityHandlerCall {
    CapabilityHandlerCall::new(owner(connection, session), Vec::new())
}

fn surface(id: &str, connection: &str, messages: Arc<Mutex<Vec<UiDisplayMessage>>>) -> UiSurface {
    UiSurface::new(
        UiAttachment {
            surface_id: id.to_owned(),
            mount_generation: 1,
        },
        owner(connection, id),
        Arc::new(move |message| {
            messages.lock().unwrap().push(message);
            Ok(())
        }),
    )
}

async fn wait_for_message(messages: &Arc<Mutex<Vec<UiDisplayMessage>>>) -> UiDisplayMessage {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Some(message) = messages.lock().unwrap().last().cloned() {
                return message;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("display message should be emitted")
}

fn ack(message: &UiDisplayMessage, rendered: bool) -> UiRenderAck {
    UiRenderAck {
        surface_id: message.surface_id.clone(),
        mount_generation: message.mount_generation,
        request_id: message.request_id.clone(),
        rendered,
    }
}

struct StageUiFixture {
    facade: Arc<super::zahir_facade::ZahirFacade>,
    tools: crate::tools::registry::ToolHandlerRegistry,
    surface: Arc<UiSurface>,
    messages: Arc<Mutex<Vec<UiDisplayMessage>>>,
}

impl StageUiFixture {
    fn new(
        rules: crate::config::types::policies::capability_constraints::CapabilityConstraintsConfig,
    ) -> Self {
        use super::{
            ownership::{OwnershipTree, VisibilityScope},
            policy::PolicyEngine,
            provider::{ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport},
            special::{CapabilityHandlerRegistry, SpecialRegistration},
        };
        let specials = CapabilityHandlerRegistry::new();
        let tools = crate::tools::registry::ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let policy = Arc::new(PolicyEngine::with_shared_tree(tree.clone(), rules).unwrap());
        let facade = super::zahir_facade::ZahirFacade::new_with_specials_and_policy(
            tools.clone(),
            tree,
            specials.clone(),
            policy,
        );
        specials.bind_domain_facade(&facade).unwrap();
        super::tool_adapters::register_canonical_adapters_with_tools(&specials, Some(&tools))
            .unwrap();
        facade.reconcile_registry_bindings();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let surface = Arc::new(surface("stage-surface", "connection", messages.clone()));
        let mut descriptor = super::ui::descriptor();
        descriptor.id.name = "stage-surface".into();
        descriptor.lifetime = super::ownership::LifetimeScope::Session;
        descriptor.visibility = VisibilityScope {
            principal: Some("principal-connection".into()),
            ..Default::default()
        };
        facade
            .bind_terminal_session(descriptor.id.clone(), descriptor.visibility.clone())
            .unwrap();
        let provider = ProviderHandle::new(ProviderLifecycle::ready(
            ProviderId::new("stage-ui-actual").unwrap(),
            ProviderTransport::InProcess,
        ));
        specials
            .register(
                SpecialRegistration::new(
                    descriptor,
                    super::ui::schema(),
                    surface.clone(),
                    provider,
                )
                .with_ui_surface(surface.clone()),
            )
            .unwrap();
        Self {
            facade,
            tools,
            surface,
            messages,
        }
    }

    async fn invoke(
        &self,
        operation: aleph_protocol::capability::UiOperation,
    ) -> Result<serde_json::Value, super::special_dispatch::CapabilityInvocationError> {
        self.facade
            .invoke_ui_gateway(
                aleph_protocol::capability::UiInvokeParams {
                    surface_id: "stage-surface".into(),
                    mount_generation: 1,
                    operation,
                },
                owner("connection", "stage-surface"),
                super::special_dispatch::CapabilityInvocationOrigin::GatewayDomain,
                CancellationToken::new(),
            )
            .await
    }

    fn service(&self) -> crate::tools::scoped::ScopedToolService {
        let registry = Arc::new(self.tools.clone());
        let mut inner = crate::tools::runtime::LoopToolRegistry::new();
        inner.bind_canonical_registry(registry.clone(), Arc::new(|_| true));
        assert!(inner.bind_canonical_facade(self.facade.clone()));
        for entry in registry.entries_snapshot().values() {
            inner.register(Box::new(
                crate::tools::adapters::McpRegistryTool::from_registry_entry(
                    entry.handler.clone(),
                    &entry.descriptor,
                ),
            ));
        }
        crate::tools::scoped::ScopedToolService::new(Arc::new(inner), Default::default())
            .with_confirmation(Arc::new(StageUiApproval))
    }
}

struct StageUiApproval;
#[async_trait::async_trait]
impl crate::sandbox::exec_approval::gate::ApprovalRequester for StageUiApproval {
    async fn request_approval(
        &self,
        _: &crate::sandbox::exec_approval::action::ApprovalAction,
    ) -> crate::sandbox::exec_approval::gate::ApprovalResponse {
        crate::sandbox::exec_approval::gate::ApprovalOutcome::Approved.into()
    }
}

async fn stage_ui_tool(
    service: &crate::tools::scoped::ScopedToolService,
    params: aleph_protocol::capability::UiInvokeParams,
) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
    stage_ui_tool_as(service, params, "principal-connection", "connection").await
}

async fn stage_ui_tool_as(
    service: &crate::tools::scoped::ScopedToolService,
    params: aleph_protocol::capability::UiInvokeParams,
    principal: &str,
    connection: &str,
) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
    use crate::tools::service::ToolService;
    crate::gateway::caller_identity::CALLER_USER
        .scope(
            Some(principal.into()),
            crate::gateway::caller_identity::CALLER_CONN_ID.scope(
                Some(connection.into()),
                crate::approval::with_call_identity(
                    Some(crate::approval::CallIdentity {
                        turn_id: crate::session::events::TurnId::nil(),
                        call_id: "stage-canonical-ui".into(),
                    }),
                    service.execute_with_cancel(
                        "ui_capability",
                        serde_json::to_value(params).unwrap(),
                        CancellationToken::new(),
                    ),
                ),
            ),
        )
        .await
}

#[tokio::test]
async fn stage_a_stable_ui_deny_has_zero_input_effect() {
    let positive = StageUiFixture::new(Default::default());
    assert!(
        positive
            .invoke(aleph_protocol::capability::UiOperation::Input {})
            .await
            .is_ok(),
        "same real surface ingress must be reachable without the deny"
    );
    positive.surface.close().await.unwrap();
    let rules = toml::from_str(
        "[\"aleph/tools/ui_capability\".required_conditions]\nprovider_ready = false\n",
    )
    .unwrap();
    let fixture = StageUiFixture::new(rules);
    fixture
        .surface
        .submit_input(
            "connection",
            UiSubmitInput {
                surface_id: "stage-surface".into(),
                mount_generation: 1,
                event: UiInputEvent::KeyDown {
                    key: "retained".into(),
                },
            },
        )
        .unwrap();
    let result = fixture
        .invoke(aleph_protocol::capability::UiOperation::Input {})
        .await;
    for operation in [
        aleph_protocol::capability::UiOperation::Input {},
        aleph_protocol::capability::UiOperation::Display {
            text: "stable deny must not render".into(),
        },
    ] {
        let denied = stage_ui_tool(
            &fixture.service(),
            aleph_protocol::capability::UiInvokeParams {
                surface_id: "stage-surface".into(),
                mount_generation: 1,
                operation,
            },
        )
        .await;
        assert!(
            denied.is_err(),
            "stable policy must also guard the actual canonical Tool"
        );
        assert!(fixture.messages.lock().unwrap().is_empty());
    }
    let retained = fixture.surface.input().unwrap();
    fixture.surface.close().await.unwrap();
    assert!(
        result.is_err(),
        "stable UI deny must reach exact-surface ingress: {result:?}"
    );
    assert!(
        retained.is_some(),
        "denied Input must not consume the actual queue"
    );
}

#[tokio::test]
async fn stage_a_registered_ui_tool_reaches_ack_and_retains_both_active_guards() {
    use aleph_protocol::capability::{UiInvokeParams, UiOperation};
    for target in ["aleph/tools/ui_capability", "aleph/ui/stage-surface"] {
        let rules = toml::from_str(&format!("[\"{target}\".resource]\nunit = \"active_calls\"\nmax_active = 1\n[\"aleph/ui/stage-surface\".required_conditions]\nattachment_ready = true\n")).unwrap();
        let fixture = Arc::new(StageUiFixture::new(rules));
        for (principal, connection, mount_generation) in [
            ("foreign-principal", "connection", 1),
            ("principal-connection", "foreign-connection", 1),
            ("principal-connection", "connection", 2),
        ] {
            let denied = stage_ui_tool_as(
                &fixture.service(),
                UiInvokeParams {
                    surface_id: "stage-surface".into(),
                    mount_generation,
                    operation: UiOperation::Display {
                        text: "must not reach renderer".into(),
                    },
                },
                principal,
                connection,
            )
            .await;
            assert!(
                denied.is_err(),
                "foreign owner/connection/mount must fail closed"
            );
            assert!(
                fixture.messages.lock().unwrap().is_empty(),
                "denied canonical Tool has zero renderer effect"
            );
        }
        let mut pending = tokio::spawn({
            let fixture = fixture.clone();
            async move {
                stage_ui_tool(
                    &fixture.service(),
                    UiInvokeParams {
                        surface_id: "stage-surface".into(),
                        mount_generation: 1,
                        operation: UiOperation::Display {
                            text: "actual canonical delivery".into(),
                        },
                    },
                )
                .await
            }
        });
        let message = tokio::select! {
            result = &mut pending => panic!("healthy registered canonical UI Tool must reach renderer before settling: {result:?}"),
            message = wait_for_message(&fixture.messages) => message,
        };
        assert!(
            !pending.is_finished(),
            "canonical Tool completion requires matching ack"
        );
        fixture
            .surface
            .submit_input(
                "connection",
                UiSubmitInput {
                    surface_id: "stage-surface".into(),
                    mount_generation: 1,
                    event: UiInputEvent::KeyDown {
                        key: "retained".into(),
                    },
                },
            )
            .unwrap();
        let denied = fixture.invoke(UiOperation::Input {}).await;
        assert!(
            denied.is_err(),
            "pending display must occupy configured {target}, not Existing"
        );
        let mut wrong = ack(&message, true);
        wrong.mount_generation += 1;
        assert!(fixture.surface.ack("connection", wrong).is_err());
        assert!(!pending.is_finished());
        fixture
            .surface
            .ack("connection", ack(&message, true))
            .unwrap();
        assert!(
            pending.await.unwrap().is_ok(),
            "matching ack must settle actual canonical Tool"
        );
        assert!(
            fixture
                .invoke(UiOperation::Input {})
                .await
                .unwrap()
                .is_object(),
            "ack releases guard and denied Input has not consumed the queue"
        );
        fixture.surface.close().await.unwrap();
        assert!(
            stage_ui_tool(
                &fixture.service(),
                UiInvokeParams {
                    surface_id: "stage-surface".into(),
                    mount_generation: 1,
                    operation: UiOperation::Input {}
                }
            )
            .await
            .is_err(),
            "closed attachment must remain unavailable"
        );
    }
}

#[tokio::test]
async fn two_surfaces_ack_input_and_unmount_are_isolated() {
    let messages_a = Arc::new(Mutex::new(Vec::new()));
    let messages_b = Arc::new(Mutex::new(Vec::new()));
    let surface_a = Arc::new(surface("surface-a", "connection-a", messages_a.clone()));
    let surface_b = Arc::new(surface("surface-b", "connection-b", messages_b.clone()));

    let display_a = tokio::spawn({
        let surface = surface_a.clone();
        async move {
            surface
                .display(
                    "alpha".into(),
                    call("connection-a", "surface-a"),
                    CancellationToken::new(),
                )
                .await
        }
    });
    let display_b = tokio::spawn({
        let surface = surface_b.clone();
        async move {
            surface
                .display(
                    "beta".into(),
                    call("connection-b", "surface-b"),
                    CancellationToken::new(),
                )
                .await
        }
    });
    let message_a = wait_for_message(&messages_a).await;
    let message_b = wait_for_message(&messages_b).await;

    assert!(surface_a
        .ack(
            "connection-b",
            UiRenderAck {
                surface_id: message_a.surface_id.clone(),
                mount_generation: message_a.mount_generation,
                request_id: message_a.request_id.clone(),
                rendered: true,
            },
        )
        .is_err());
    assert!(surface_a
        .ack(
            "connection-a",
            UiRenderAck {
                surface_id: message_a.surface_id.clone(),
                mount_generation: message_a.mount_generation,
                request_id: message_a.request_id.clone(),
                rendered: false,
            },
        )
        .is_err());
    assert!(!display_a.is_finished());
    assert!(!display_b.is_finished());

    surface_a
        .submit_input(
            "connection-a",
            UiSubmitInput {
                surface_id: "surface-a".into(),
                mount_generation: 1,
                event: UiInputEvent::KeyDown { key: "a".into() },
            },
        )
        .unwrap();
    surface_b
        .submit_input(
            "connection-b",
            UiSubmitInput {
                surface_id: "surface-b".into(),
                mount_generation: 1,
                event: UiInputEvent::MouseDown {
                    x: 10,
                    y: 20,
                    button: 1,
                },
            },
        )
        .unwrap();
    assert_eq!(
        surface_a.input().unwrap(),
        Some(UiInputEvent::KeyDown { key: "a".into() })
    );
    assert_eq!(surface_a.input().unwrap(), None);
    assert_eq!(
        surface_b.input().unwrap(),
        Some(UiInputEvent::MouseDown {
            x: 10,
            y: 20,
            button: 1
        })
    );
    assert_eq!(surface_b.input().unwrap(), None);

    surface_a
        .ack("connection-a", ack(&message_a, true))
        .unwrap();
    surface_b
        .ack("connection-b", ack(&message_b, true))
        .unwrap();
    assert!(display_a.await.unwrap().is_ok());
    assert!(display_b.await.unwrap().is_ok());

    surface_a.close().await.unwrap();
    assert!(surface_a
        .ack("connection-a", ack(&message_a, true))
        .is_err());
}

#[tokio::test(start_paused = true)]
async fn surface_bounds_timeout_and_disconnect_are_explicit() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let surface = Arc::new(surface("surface", "connection", messages.clone()));

    let accepted = "é".repeat(MAX_UI_TEXT_BYTES / "é".len());
    let accepted_task = tokio::spawn({
        let surface = surface.clone();
        async move {
            surface
                .display(
                    accepted,
                    call("connection", "surface"),
                    CancellationToken::new(),
                )
                .await
        }
    });
    let accepted_message = wait_for_message(&messages).await;
    surface
        .ack("connection", ack(&accepted_message, true))
        .unwrap();
    assert!(accepted_task.await.unwrap().is_ok());
    assert!(surface
        .display(
            "x".repeat(MAX_UI_TEXT_BYTES + 1),
            call("connection", "surface"),
            CancellationToken::new()
        )
        .await
        .is_err());

    let key_256 = "é".repeat(MAX_UI_KEY_BYTES / 2);
    surface
        .submit_input(
            "connection",
            UiSubmitInput {
                surface_id: "surface".into(),
                mount_generation: 1,
                event: UiInputEvent::KeyDown {
                    key: key_256.clone(),
                },
            },
        )
        .unwrap();
    assert_eq!(
        surface.input().unwrap(),
        Some(UiInputEvent::KeyDown {
            key: key_256.clone()
        })
    );
    assert!(
        matches!(
            surface.submit_input(
                "connection",
                UiSubmitInput {
                    surface_id: "surface".into(),
                    mount_generation: 1,
                    event: UiInputEvent::KeyDown {
                        key: format!("{key_256}x")
                    }
                }
            ),
            Err(super::ui::UiError::InvalidInput(_))
        ),
        "257 UTF-8 bytes must fail with InvalidInput, not a full queue"
    );
    for button in [0, 4] {
        let event = UiInputEvent::MouseDown { x: 0, y: 0, button };
        surface
            .submit_input(
                "connection",
                UiSubmitInput {
                    surface_id: "surface".into(),
                    mount_generation: 1,
                    event: event.clone(),
                },
            )
            .unwrap();
        assert_eq!(surface.input().unwrap(), Some(event));
    }
    assert!(matches!(
        surface.submit_input(
            "connection",
            UiSubmitInput {
                surface_id: "surface".into(),
                mount_generation: 1,
                event: UiInputEvent::MouseDown {
                    x: 0,
                    y: 0,
                    button: 5
                }
            }
        ),
        Err(super::ui::UiError::InvalidInput(_))
    ));

    for _ in 0..MAX_UI_QUEUE {
        surface
            .submit_input(
                "connection",
                UiSubmitInput {
                    surface_id: "surface".into(),
                    mount_generation: 1,
                    event: UiInputEvent::KeyDown { key: "k".into() },
                },
            )
            .unwrap();
    }
    assert!(matches!(
        surface.submit_input(
            "connection",
            UiSubmitInput {
                surface_id: "surface".into(),
                mount_generation: 1,
                event: UiInputEvent::KeyDown { key: "k".into() },
            },
        ),
        Err(super::ui::UiError::QueueFull)
    ));
    assert!(surface
        .submit_input(
            "connection",
            UiSubmitInput {
                surface_id: "surface".into(),
                mount_generation: 1,
                event: UiInputEvent::KeyDown {
                    key: "é".repeat(MAX_UI_KEY_BYTES / 2 + 1)
                },
            },
        )
        .is_err());

    let timeout_task = tokio::spawn({
        let surface = surface.clone();
        async move {
            surface
                .display(
                    "timeout".into(),
                    call("connection", "surface"),
                    CancellationToken::new(),
                )
                .await
        }
    });
    let timeout_message = wait_for_message(&messages).await;
    assert!(surface
        .display(
            "busy".into(),
            call("connection", "surface"),
            CancellationToken::new()
        )
        .await
        .is_err());
    tokio::time::advance(std::time::Duration::from_millis(UI_ACK_TIMEOUT_MS)).await;
    assert!(timeout_task.await.unwrap().is_err());
    assert!(surface
        .ack("connection", ack(&timeout_message, true))
        .is_err());

    let cancel = CancellationToken::new();
    let cancelled_task = tokio::spawn({
        let surface = surface.clone();
        let cancel = cancel.clone();
        async move {
            surface
                .display("cancel".into(), call("connection", "surface"), cancel)
                .await
        }
    });
    let cancelled_message = wait_for_message(&messages).await;
    cancel.cancel();
    assert!(cancelled_task.await.unwrap().is_err());
    assert!(surface
        .ack("connection", ack(&cancelled_message, true))
        .is_err());

    let disconnect_task = tokio::spawn({
        let surface = surface.clone();
        async move {
            surface
                .display(
                    "disconnect".into(),
                    call("connection", "surface"),
                    CancellationToken::new(),
                )
                .await
        }
    });
    let disconnect_message = wait_for_message(&messages).await;
    surface.close().await.unwrap();
    assert!(disconnect_task.await.unwrap().is_err());
    assert!(surface
        .ack("connection", ack(&disconnect_message, true))
        .is_err());
}

struct ActiveCallFacts(crate::capability::policy_runtime::PolicyFactSourceKey);
impl crate::capability::policy_runtime::ConstraintFactsSource for ActiveCallFacts {
    fn source_key(&self) -> crate::capability::policy_runtime::PolicyFactSourceKey {
        self.0.clone()
    }
    fn supported_conditions(&self) -> &'static [&'static str] {
        &[]
    }
    fn supported_units(
        &self,
    ) -> &'static [crate::config::types::policies::capability_constraints::ResourceUnit] {
        &[crate::config::types::policies::capability_constraints::ResourceUnit::ActiveCalls]
    }
    fn snapshot(
        &self,
        _: &CapabilityCallContext,
        _: Option<crate::config::types::policies::capability_constraints::ResourceUnit>,
        active: usize,
    ) -> Result<
        crate::capability::constraints::ConstraintContext,
        crate::capability::constraints::ConstraintViolation,
    > {
        Ok(
            crate::capability::constraints::ConstraintContext::at_hour(12)
                .unwrap()
                .with_active_resources(active),
        )
    }
}

struct GuardFixture {
    policy: crate::capability::policy::PolicyEngine,
    lease: crate::capability::facade::BackendLease,
    attachment: crate::capability::policy_runtime::PolicyAttachment,
}
impl GuardFixture {
    fn new() -> Self {
        use crate::capability::{
            facade::{BackendLease, BackendSourceEvidence},
            ownership::OwnershipTree,
            policy::PolicyEngine,
            policy_runtime::PolicyFactSourceKey,
            provider::{ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport},
        };
        let descriptor = super::ui::descriptor();
        let tree = Arc::new(OwnershipTree::new());
        tree.register(
            descriptor.id.clone(),
            descriptor.owner.clone(),
            descriptor.lifetime.clone(),
            descriptor.visibility.clone(),
        )
        .unwrap();
        let provider = ProviderHandle::new(ProviderLifecycle::ready(
            ProviderId::new("runtime/ui-test").unwrap(),
            ProviderTransport::InProcess,
        ));
        let generation = tree
            .generation(&descriptor.id, &descriptor.visibility)
            .unwrap();
        let lease = BackendLease {
            descriptor,
            owner_generation: generation,
            owner: OwnerRef::Runtime,
            source: BackendSourceEvidence::Provider {
                provider_id: provider.provider_id(),
                provider_generation: provider.generation(),
            },
            required_resources: vec![],
        };
        let rules = toml::from_str(
            "[\"aleph/ui/surface\".resource]\nunit = \"active_calls\"\nmax_active = 1\n",
        )
        .unwrap();
        let policy = PolicyEngine::with_shared_tree(tree, rules).unwrap();
        let facts = Arc::new(ActiveCallFacts(PolicyFactSourceKey::for_source(
            &provider,
            &lease.descriptor.id,
            1,
        )));
        let attachment = policy.prepare_attachment(&lease, facts).unwrap();
        Self {
            policy,
            lease,
            attachment,
        }
    }
    fn admit(
        &self,
    ) -> Result<
        crate::capability::policy_runtime::RunningCapabilityCall,
        crate::capability::policy::PolicyDenial,
    > {
        self.policy.admit_runtime(
            &self.lease,
            Some(&self.attachment),
            &owner("connection", "surface"),
            crate::capability::policy_runtime::ResourceUse::Allocate,
        )
    }
}

#[tokio::test(start_paused = true)]
async fn real_active_call_guard_survives_waiter_drop_until_each_resolution() {
    for resolution in ["ack", "cancel", "timeout", "close"] {
        let fixture = GuardFixture::new();
        let guard = fixture
            .admit()
            .expect("real configured active_calls admission");
        assert!(
            fixture.admit().is_err(),
            "positive control: one real guard exhausts max_active=1"
        );
        let admission = CapabilityHandlerCall::new(owner("connection", "surface"), vec![guard]);
        assert_eq!(admission.running_call_count(), 1);
        let messages = Arc::new(Mutex::new(Vec::new()));
        let surface = Arc::new(surface("surface", "connection", messages.clone()));
        let cancel = CancellationToken::new();
        let waiter = tokio::spawn({
            let surface = surface.clone();
            let cancel = cancel.clone();
            async move {
                surface
                    .display("retained admission".into(), admission, cancel)
                    .await
            }
        });
        let message = wait_for_message(&messages).await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            fixture.admit().is_err(),
            "dropping the result waiter must NOT release {resolution} operation admission"
        );
        let mut wrong = ack(&message, true);
        wrong.mount_generation += 1;
        assert!(surface.ack("connection", wrong).is_err());
        assert!(surface
            .ack("foreign-connection", ack(&message, true))
            .is_err());
        assert!(surface.ack("connection", ack(&message, false)).is_err());
        assert!(
            fixture.admit().is_err(),
            "negative acks cannot release a real running guard"
        );
        match resolution {
            "ack" => surface.ack("connection", ack(&message, true)).unwrap(),
            "cancel" => cancel.cancel(),
            "timeout" => {
                tokio::time::advance(std::time::Duration::from_millis(UI_ACK_TIMEOUT_MS)).await
            }
            "close" => surface.close().await.unwrap(),
            _ => unreachable!(),
        }
        let mut restored = false;
        for _ in 0..500 {
            tokio::task::yield_now().await;
            if let Ok(guard) = fixture.admit() {
                drop(guard);
                restored = true;
                break;
            }
        }
        assert!(restored, "real admission must restore after {resolution}");
        assert!(
            surface.ack("connection", ack(&message, true)).is_err(),
            "late ack is stale after {resolution}"
        );
        surface.close().await.unwrap();
    }
}

#[tokio::test]
async fn missing_renderer_releases_real_admission_without_success() {
    let fixture = GuardFixture::new();
    let admission = CapabilityHandlerCall::new(
        owner("connection", "surface"),
        vec![fixture.admit().unwrap()],
    );
    let surface = UiSurface::new(
        UiAttachment {
            surface_id: "surface".into(),
            mount_generation: 1,
        },
        owner("connection", "surface"),
        Arc::new(|_| Err(super::ui::UiError::Unavailable)),
    );
    assert!(surface
        .display("no renderer".into(), admission, CancellationToken::new())
        .await
        .is_err());
    assert!(
        fixture.admit().is_ok(),
        "failed delivery must release its actual admission"
    );
    surface.close().await.unwrap();
}
