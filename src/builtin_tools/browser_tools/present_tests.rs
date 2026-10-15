use super::*;
use crate::executor::BuiltinToolConfig;
use crate::gateway::caller_identity::{CALLER_CONN_ID, CALLER_USER};
use crate::gateway::security::store::OWNER_USER_ID;
use crate::gateway::visibility::{owner_or_legacy, run_principal_with, RunPrincipal};
use crate::gateway::GatewayEventFrame;
use crate::routing::session_key::SessionKey;
use crate::scope::ScopeAttribution;
use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
use aleph_protocol::browser_live::{
    CompanionArrangement, CompanionLayout, CompanionScreen, CompanionStateHint,
    IssueViewerGrantRequest, ResourceBinding, ResourceDecision, ViewerOrigin, ViewerPermission,
    ViewportCssPx,
};
use tokio::sync::broadcast::error::TryRecvError;

use crate::browser::cdp_backend::{BrowserOperationKind, BrowserOperationRecord};
use crate::browser::live::tests::fixture as owner_fixture;

struct Fixture {
    producer: BrowserLiveProducer,
    resources: ResourceRegistry,
    grants: ViewerGrantStore,
    bus: Arc<GatewayEventBus>,
}

fn fixture() -> Fixture {
    let resources = ResourceRegistry::default();
    let grants = ViewerGrantStore::default();
    let bus = Arc::new(GatewayEventBus::default());
    let config = BuiltinToolConfig {
        browser_live_producer: Some(Arc::new(BrowserLiveProducer::new(
            resources.clone(),
            grants.clone(),
        ))),
        event_bus: Some(bus.clone()),
        ..Default::default()
    };
    Fixture {
        producer: config.browser_live_producer().unwrap(),
        resources,
        grants,
        bus,
    }
}

fn turn() -> TurnContext {
    TurnContext {
        session_key: SessionKey::main("producer-test"),
        run_id: "producer-run".into(),
        channel_id: "".into(),
        conversation_id: "".into(),
        caller_role: None,
        telecom_authority: None,
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
    }
}

async fn scoped<F: std::future::Future>(
    context: TurnContext,
    user: Option<&str>,
    connection: Option<&str>,
    future: F,
) -> F::Output {
    TURN_CONTEXT
        .scope(
            context,
            CALLER_USER.scope(
                user.map(str::to_string),
                CALLER_CONN_ID.scope(connection.map(str::to_string), future),
            ),
        )
        .await
}

/// The production shape: a run's tool call carries its TURN_CONTEXT plus the
/// run's persisted scope attribution (re-threaded around the run's task
/// tree) — never a caller connection identity. CALLER_USER/CALLER_CONN_ID
/// are RPC-boundary task-locals, dead past the spawn every run crosses.
async fn run_scoped<F: std::future::Future>(
    context: TurnContext,
    user: &str,
    future: F,
) -> F::Output {
    crate::scope::with_scope(
        Some(ScopeAttribution::personal(user)),
        TURN_CONTEXT.scope(context, future),
    )
    .await
}

fn target() -> BrowserTargetBinding {
    BrowserTargetBinding {
        profile: "producer-profile".into(),
        browser_session_id: "operation-browser-session".into(),
        target_id: "operation-target".into(),
    }
}

// A typed fake of a launcher owner's record, never a URL-derived target.
fn managed_operation() -> BrowserPresentOperation {
    BrowserPresentOperation::from_launcher_record(target(), "test-launcher".into(), 1234, 1000)
}

fn metadata() -> BrowserPresentation {
    BrowserPresentation {
        kind: PreviewKind::DesignCompanion,
        display: DisplayPolicy::ExplicitOnly,
        redaction: RedactionStatus::Safe,
        ttl: Duration::from_secs(60),
        companion_candidates: vec![CompanionCandidate {
            id: "candidate-one".into(),
            label: "First candidate".into(),
            layout: CompanionLayout {
                arrangement: CompanionArrangement::Single,
                screens: vec![CompanionScreen {
                    id: "desktop".into(),
                    label: "Desktop".into(),
                    viewport_css_px: ViewportCssPx {
                        width: 1280,
                        height: 720,
                    },
                }],
            },
            state: CompanionStateHint {
                active_screen_id: Some("desktop".into()),
                animation: None,
                prefers_reduced_motion: false,
            },
        }],
    }
}

fn assert_empty(rx: &mut tokio::sync::broadcast::Receiver<GatewayEventFrame>) {
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
}

fn read_resource(rx: &mut tokio::sync::broadcast::Receiver<GatewayEventFrame>) -> PreviewResource {
    match rx.try_recv().unwrap() {
        GatewayEventFrame::BrowserLiveResource { resource } => resource,
        other => panic!("expected resource, got {other:?}"),
    }
}

fn read_closed(rx: &mut tokio::sync::broadcast::Receiver<GatewayEventFrame>) -> BrowserLiveClosed {
    match rx.try_recv().unwrap() {
        GatewayEventFrame::BrowserLiveClosed { closed } => closed,
        other => panic!("expected closed, got {other:?}"),
    }
}

fn mint(f: &Fixture, resource: &PreviewResource) -> String {
    let binding = resource.viewer_binding().unwrap();
    f.grants
        .issue(
            binding.principal.clone(),
            binding.clone(),
            &resource.resource_id,
            &[ViewerPermission::Frames],
            ViewerOrigin {
                scheme: "https".into(),
                host: "viewer.test".into(),
                port: 443,
            },
            Duration::from_secs(60),
        )
        .unwrap()
        .grant_id
}

fn issuer(f: &Fixture) -> Arc<crate::browser::live::issuer::BrowserViewerIssuerState> {
    crate::browser::live::issuer::BrowserViewerIssuerState::new(
        f.grants.clone(),
        f.resources.clone(),
        "https://viewer.test",
        Duration::from_secs(30),
    )
    .unwrap()
}

fn frames_request(resource: &PreviewResource) -> IssueViewerGrantRequest {
    IssueViewerGrantRequest {
        resource_id: resource.resource_id.clone(),
        permissions: vec![ViewerPermission::Frames],
    }
}

/// The only way a resource becomes `Safe`: the owner's explicit decision CAS
/// on the exact current revision (never a producer-supplied redaction).
fn user_decides_safe(
    f: &Fixture,
    rx: &mut tokio::sync::broadcast::Receiver<GatewayEventFrame>,
    resource: &PreviewResource,
) -> PreviewResource {
    f.producer
        .lifecycle()
        .decide(
            "producer-user",
            &resource.resource_id,
            resource.revision,
            ResourceDecision::Safe,
        )
        .unwrap();
    let safe = read_resource(rx);
    assert_eq!(safe.redaction, RedactionStatus::Safe);
    assert_eq!(safe.revision, resource.revision + 1);
    safe
}

#[test]
fn producer_redaction_never_yields_safe() {
    assert_eq!(
        producer_redaction(RedactionStatus::Safe),
        RedactionStatus::Pending
    );
    assert_eq!(
        producer_redaction(RedactionStatus::Pending),
        RedactionStatus::Pending
    );
    assert_eq!(
        producer_redaction(RedactionStatus::Refused),
        RedactionStatus::Refused
    );
}

/// A producer (tool/LLM) asking for `Safe` must not bypass the user decision:
/// the published resource is `Pending`, no viewer grant can be issued, and
/// only the real decision CAS makes it viewable.
#[tokio::test]
async fn browser_present_safe_input_publishes_pending_and_issues_no_grant() {
    for input in [RedactionStatus::Safe, RedactionStatus::Pending] {
        let f = fixture();
        let mut rx = f.bus.subscribe_typed();
        let published = run_scoped(turn(), "producer-user", async {
            let mut presentation = metadata();
            presentation.redaction = input;
            f.producer
                .browser_present(Some(managed_operation()), presentation)
                .unwrap()
        })
        .await;
        let initial = read_resource(&mut rx);
        assert_eq!(initial, *published.resource());
        assert_eq!(initial.redaction, RedactionStatus::Pending);
        assert_eq!(
            f.resources
                .get_active(&initial.resource_id)
                .unwrap()
                .redaction,
            RedactionStatus::Pending
        );
        assert!(issuer(&f)
            .issue("producer-user", frames_request(&initial))
            .is_err());
        assert_empty(&mut rx);

        let safe = user_decides_safe(&f, &mut rx, &initial);
        assert!(issuer(&f)
            .issue("producer-user", frames_request(&safe))
            .is_ok());
    }
}

/// `revise` goes through the same clamp: a producer-supplied `Safe` revision
/// is `Pending`, grants on the earlier (user-approved) revision are revoked,
/// and no new grant can be issued without a fresh user decision.
#[tokio::test]
async fn revise_safe_input_is_pending_revokes_grants_and_issues_none() {
    for input in [RedactionStatus::Safe, RedactionStatus::Pending] {
        let f = fixture();
        let mut rx = f.bus.subscribe_typed();
        run_scoped(turn(), "producer-user", async {
            f.producer
                .browser_present(Some(managed_operation()), metadata())
                .unwrap()
        })
        .await;
        let initial = read_resource(&mut rx);
        assert_eq!(initial.redaction, RedactionStatus::Pending);
        let safe = user_decides_safe(&f, &mut rx, &initial);
        let grant = issuer(&f)
            .issue("producer-user", frames_request(&safe))
            .unwrap();
        assert!(f.grants.grant_info(&grant.grant_id).is_some());

        let revised = run_scoped(turn(), "producer-user", async {
            let mut presentation = metadata();
            presentation.redaction = input;
            f.producer.revise(&safe, presentation).unwrap()
        })
        .await;
        assert_eq!(revised.redaction, RedactionStatus::Pending);
        assert_eq!(revised.revision, safe.revision + 1);
        assert_eq!(read_resource(&mut rx), revised);
        assert!(f.grants.grant_info(&grant.grant_id).is_none());
        assert!(issuer(&f)
            .issue("producer-user", frames_request(&revised))
            .is_err());
    }
}

#[tokio::test]
async fn with_event_bus_does_not_rebind_an_already_bound_producer() {
    let f = fixture();
    let other = Arc::new(GatewayEventBus::default());
    let mut original_rx = f.bus.subscribe_typed();
    let mut other_rx = other.subscribe_typed();
    let rebound = f.producer.with_event_bus(other.clone());
    let published = run_scoped(turn(), "producer-user", async {
        rebound
            .browser_present(Some(managed_operation()), metadata())
            .unwrap()
    })
    .await;
    assert_eq!(read_resource(&mut original_rx), *published.resource());
    assert_empty(&mut other_rx);
}

#[tokio::test]
async fn present_producer_preserves_turn_binding_and_service_exit() {
    let f = fixture();
    let mut rx = f.bus.subscribe_typed();
    let mut raw = f.bus.subscribe();
    let mut published = run_scoped(turn(), "producer-user", async {
        f.producer
            .browser_present(Some(managed_operation()), metadata())
            .unwrap()
    })
    .await;
    let initial = published.resource().clone();
    assert_eq!(read_resource(&mut rx), initial);
    assert_empty(&mut rx); // Exactly one publication for an explicit present.
                           // `metadata()` asks for Safe; a producer can only publish Pending.
    assert_eq!(initial.redaction, RedactionStatus::Pending);
    let event: serde_json::Value = serde_json::from_str(&raw.try_recv().unwrap()).unwrap();
    assert_eq!(event["topic"], aleph_protocol::browser_live::RESOURCE_TOPIC);
    assert_eq!(
        event["data"]["resource"],
        serde_json::to_value(&initial).unwrap()
    );
    let binding = initial.viewer_binding().unwrap();
    assert_eq!(
        binding.principal,
        IssuerIdentity::User {
            user_id: "producer-user".into()
        }
    );
    assert_eq!(
        binding.task,
        ConversationBinding {
            session_key: turn().session_key.to_key_string(),
            run_id: turn().run_id,
        }
    );
    assert_eq!(binding.target, target());
    assert!(matches!(initial.binding, ResourceBinding::Browser { .. }));
    assert!(initial.resource_id.starts_with("res_"));
    assert_eq!(initial.revision, 1);
    assert_eq!(initial.source, PreviewSource::ManagedTool);
    assert_eq!(initial.provenance, managed_operation().provenance);
    assert_eq!(
        f.resources.get_active(&initial.resource_id),
        Some(initial.clone())
    );

    // The actual issuer sees the same authority/record, not an independent
    // table: the Pending publication is not viewable until the user decides.
    assert!(issuer(&f)
        .issue("producer-user", frames_request(&initial))
        .is_err());

    run_scoped(turn(), "producer-user", async {
        let mut hmr = metadata();
        hmr.companion_candidates[0].id = "candidate-hmr".into();
        published.revise(hmr).unwrap();
        let revised = read_resource(&mut rx);
        assert_eq!(revised, *published.resource());
        assert_eq!(revised.resource_id, initial.resource_id);
        assert_eq!(revised.revision, 2);
        assert_eq!(revised.binding, initial.binding);
        assert_eq!(revised.provenance, initial.provenance);
        assert_eq!(revised.companion_candidates[0].id, "candidate-hmr");
        assert_eq!(revised.redaction, RedactionStatus::Pending);
        assert!(matches!(
            f.producer.revise(&initial, metadata()),
            Err(PresentError::Registry(ResourceError::StaleRevision))
        ));
        assert_empty(&mut rx);
    })
    .await;

    // Only the owner's decision CAS (on the exact current revision) makes the
    // resource viewable; the issued grant then sees the same binding.
    let safe = user_decides_safe(&f, &mut rx, published.resource());
    let grant = issuer(&f)
        .issue("producer-user", frames_request(&safe))
        .unwrap();
    let info = f.grants.grant_info(&grant.grant_id).unwrap();
    assert_eq!(info.binding, *binding);
    assert_eq!(info.issuer, binding.principal);
    assert_eq!(info.resource_id, initial.resource_id);

    // A real exit observation arrives after the task-local turn has ended.
    assert!(current_turn_context().is_none());
    let closed = published.service_exited().unwrap();
    assert_eq!(read_closed(&mut rx), closed);
    assert_eq!(closed.resource_id, initial.resource_id);
    assert_eq!(closed.binding, initial.binding);
    assert_eq!(closed.reason, ClosedReason::ServiceExited);
    assert_eq!(f.resources.closed(&initial.resource_id), Some(closed));
    assert!(f.resources.get_active(&initial.resource_id).is_none());
    assert!(f.grants.grant_info(&grant.grant_id).is_none());
    assert!(matches!(
        published.service_exited(),
        Err(PresentError::AlreadyClosed)
    ));
    assert_empty(&mut rx);
}

#[tokio::test]
async fn missing_turn_session_run_target_or_bus_publishes_nothing() {
    let f = fixture();
    let mut rx = f.bus.subscribe_typed();
    assert!(matches!(
        f.producer
            .browser_present(Some(managed_operation()), metadata()),
        Err(PresentError::MissingTurn)
    ));
    let mut no_run = turn();
    no_run.run_id.clear();
    let mut no_session = turn();
    no_session.session_key = SessionKey::Main {
        agent_id: "".into(),
        main_key: "".into(),
        epoch: 0,
    };
    for context in [no_run, no_session] {
        run_scoped(context, "producer-user", async {
            assert!(matches!(
                f.producer
                    .browser_present(Some(managed_operation()), metadata()),
                Err(PresentError::MissingTurn)
            ));
        })
        .await;
    }
    run_scoped(turn(), "producer-user", async {
        assert!(matches!(
            f.producer.browser_present(None, metadata()),
            Err(PresentError::MissingTarget)
        ));
        for axis in 0..3 {
            let mut target = target();
            match axis {
                0 => target.profile.clear(),
                1 => target.browser_session_id.clear(),
                _ => target.target_id.clear(),
            }
            assert!(matches!(
                f.producer.browser_present(
                    Some(BrowserPresentOperation::from_browser_operation(target)),
                    metadata()
                ),
                Err(PresentError::MissingTarget)
            ));
        }
        let no_bus = BrowserLiveProducer::new(f.resources.clone(), f.grants.clone());
        assert!(matches!(
            no_bus.browser_present(Some(managed_operation()), metadata()),
            Err(PresentError::NotConfigured)
        ));
        let mut invalid = metadata();
        invalid.kind = PreviewKind::Artifact;
        assert!(matches!(
            f.producer
                .browser_present(Some(managed_operation()), invalid),
            Err(PresentError::Registry(
                ResourceError::ArtifactNotPublishable
            ))
        ));
    })
    .await;
    assert_empty(&mut rx);
    assert!(BuiltinToolConfig::default()
        .browser_live_producer()
        .is_err());
    let mut no_bus = BuiltinToolConfig {
        browser_live_producer: Some(Arc::new(f.producer.clone())),
        ..Default::default()
    };
    assert!(no_bus.browser_live_producer().is_err());
    no_bus.event_bus = Some(f.bus.clone());
    no_bus.browser_live_producer = None;
    assert!(no_bus.browser_live_producer().is_err());
}

/// Task 20 S3: the publish principal is the RUN's principal
/// (`gateway::visibility::run_principal`), derived from the persisted run
/// scope that is re-threaded around the run's task tree — never the
/// RPC-boundary caller task-locals, which are dead past the spawn every run
/// crosses. No CALLER_USER/CALLER_CONN_ID is scoped here at all.
#[tokio::test]
async fn run_scope_principal_publishes_without_any_caller_identity() {
    let f = fixture();
    let mut rx = f.bus.subscribe_typed();
    let published = run_scoped(turn(), "run-owner", async {
        f.producer
            .browser_present(Some(managed_operation()), metadata())
            .unwrap()
    })
    .await;
    let resource = read_resource(&mut rx);
    assert_eq!(resource, *published.resource());
    assert_eq!(
        resource.viewer_binding().unwrap().principal,
        IssuerIdentity::User {
            user_id: "run-owner".into()
        }
    );
    assert_empty(&mut rx);
}

/// A foreign CALLER_USER live on the same task (reachable only at the RPC
/// boundary in production) must neither override nor rescue the run's own
/// principal: the publication names the run scope's person, and a later
/// revise under the same caller noise still matches that binding.
#[tokio::test]
async fn foreign_caller_user_cannot_override_the_run_principal() {
    let f = fixture();
    let mut rx = f.bus.subscribe_typed();
    let mut published = crate::scope::with_scope(
        Some(ScopeAttribution::personal("run-owner")),
        TURN_CONTEXT.scope(
            turn(),
            CALLER_USER.scope(Some("foreign-caller".to_string()), async {
                f.producer
                    .browser_present(Some(managed_operation()), metadata())
                    .unwrap()
            }),
        ),
    )
    .await;
    let resource = read_resource(&mut rx);
    assert_eq!(
        resource.viewer_binding().unwrap().principal,
        IssuerIdentity::User {
            user_id: "run-owner".into()
        },
        "the caller connection identity must never stamp a publication"
    );
    crate::scope::with_scope(
        Some(ScopeAttribution::personal("run-owner")),
        TURN_CONTEXT.scope(
            turn(),
            CALLER_USER.scope(Some("foreign-caller".to_string()), async {
                published.revise(metadata()).unwrap();
            }),
        ),
    )
    .await;
    read_resource(&mut rx);
    assert_empty(&mut rx);
}

/// Legacy verdict (no person attached, single-user install): the machine
/// owner. `owner_or_legacy(None)` IS `OWNER_USER_ID`, and the websocket
/// default user resolves to the very same constant
/// (`gateway::handlers::connect::resolve_connection_identity` returns
/// `Some(OWNER_USER_ID)` for loopback and for unbound devices) — one
/// constant, no drift, so publishing as the owner invents no identity.
#[tokio::test]
async fn single_user_run_without_attached_person_publishes_as_the_owner() {
    assert_eq!(owner_or_legacy(None), OWNER_USER_ID);
    let f = fixture();
    let mut rx = f.bus.subscribe_typed();
    TURN_CONTEXT
        .scope(turn(), async {
            let published = f
                .producer
                .browser_present(Some(managed_operation()), metadata())
                .unwrap();
            assert_eq!(
                published.resource().viewer_binding().unwrap().principal,
                IssuerIdentity::User {
                    user_id: OWNER_USER_ID.to_string()
                }
            );
        })
        .await;
    read_resource(&mut rx);
    assert_empty(&mut rx);
}

/// A connection id is transport, not identity: with only CALLER_CONN_ID in
/// scope the publication is still the run principal's (Legacy → the owner on
/// this store-less, single-user test process) and never an
/// `IssuerIdentity::Connection`.
#[tokio::test]
async fn a_connection_scope_is_never_an_identity() {
    let f = fixture();
    let mut rx = f.bus.subscribe_typed();
    scoped(turn(), None, Some("transport-connection"), async {
        let published = f
            .producer
            .browser_present(Some(managed_operation()), metadata())
            .unwrap();
        assert_eq!(
            published.resource().viewer_binding().unwrap().principal,
            IssuerIdentity::User {
                user_id: OWNER_USER_ID.to_string()
            },
            "no caller user and no run person on a single-user install is the owner, not the connection"
        );
    })
    .await;
    read_resource(&mut rx);
    assert_empty(&mut rx);
}

/// Unattached — nobody attached on a MULTI-user server — is never
/// permission (判据 §8): the verdict exists at the source,
/// `run_principal_with(None, true)`, and present.rs maps it to
/// `PresentError::MissingIdentity` with no caller fallback. The lib test
/// process installs no users store, so `slot::multi_user()` is always false
/// here and the verdict itself can only be exercised at the source; the
/// present.rs arm is pinned textually by
/// `present_identity_comes_only_from_the_run_scope`.
#[test]
fn unattached_verdict_has_no_publishable_principal() {
    assert_eq!(run_principal_with(None, true), RunPrincipal::Unattached);
    assert_eq!(run_principal_with(None, false), RunPrincipal::Legacy);
    assert_eq!(
        run_principal_with(Some("u-alice".to_string()), true),
        RunPrincipal::Person("u-alice".to_string())
    );
}

/// Census guard over present.rs production code (comments and literals
/// stripped): the publish principal comes only from the run-scope verdict —
/// no caller task-local reads, no `IssuerIdentity::Connection` minting.
#[test]
fn present_identity_comes_only_from_the_run_scope() {
    let code = crate::utils::source_scan::code_text(include_str!("present.rs"));
    for forbidden in [
        "current_caller_user(",
        "current_caller_conn_id(",
        "CALLER_CONN_ID",
        "IssuerIdentity::Connection",
    ] {
        assert!(
            !code.contains(forbidden),
            "present.rs must not take identity from the caller connection: found `{forbidden}`"
        );
    }
    for required in [
        "run_principal",
        "RunPrincipal::Unattached",
        "owner_or_legacy",
    ] {
        assert!(
            code.contains(required),
            "present.rs must derive the principal from the run-scope verdict: missing `{required}`"
        );
    }
}

#[tokio::test]
async fn refusal_publish_and_revision_close_and_revoke_without_resource_event() {
    for revise in [false, true] {
        let f = fixture();
        let mut rx = f.bus.subscribe_typed();
        run_scoped(turn(), "producer-user", async {
            let mut published = f
                .producer
                .browser_present(Some(managed_operation()), metadata())
                .unwrap();
            let initial = read_resource(&mut rx);
            assert_eq!(initial.redaction, RedactionStatus::Pending);
            let grant = mint(&f, &initial);
            let mut refused = metadata();
            refused.redaction = RedactionStatus::Refused;
            let error = if revise {
                published.revise(refused).unwrap_err()
            } else {
                match f
                    .producer
                    .browser_present(Some(managed_operation()), refused)
                {
                    Err(error) => error,
                    Ok(_) => panic!("refused publication succeeded"),
                }
            };
            assert!(matches!(
                error,
                PresentError::Registry(ResourceError::Refused { closed: Some(_) })
            ));
            let closed = read_closed(&mut rx);
            assert_eq!(closed.resource_id, initial.resource_id);
            assert_eq!(closed.binding, initial.binding);
            assert_eq!(closed.reason, ClosedReason::Revoked);
            assert_eq!(f.resources.closed(&initial.resource_id), Some(closed));
            assert!(f.grants.grant_info(&grant).is_none());
            assert_empty(&mut rx);
        })
        .await;
    }
}

#[tokio::test]
async fn foreign_principal_or_turn_cannot_revise_or_refuse_resource() {
    let f = fixture();
    let mut rx = f.bus.subscribe_typed();
    let published = run_scoped(turn(), "producer-user", async {
        f.producer
            .browser_present(Some(managed_operation()), metadata())
            .unwrap()
    })
    .await;
    read_resource(&mut rx);
    for (context, user) in [
        (turn(), "another-user"),
        (
            {
                let mut t = turn();
                t.run_id = "other-run".into();
                t
            },
            "producer-user",
        ),
    ] {
        run_scoped(context, user, async {
            let mut refused = metadata();
            refused.redaction = RedactionStatus::Refused;
            assert!(matches!(
                f.producer.revise(published.resource(), refused),
                Err(PresentError::BindingMismatch)
            ));
        })
        .await;
    }
    assert!(matches!(
        f.producer.revise(published.resource(), metadata()),
        Err(PresentError::MissingTurn)
    ));
    assert_eq!(
        f.resources.get_active(&published.resource().resource_id),
        Some(published.resource().clone())
    );
    assert_empty(&mut rx);
}

#[tokio::test]
async fn expiry_sweep_and_expired_revision_revoke_and_emit_exact_closed() {
    for revise in [false, true] {
        let f = fixture();
        let mut rx = f.bus.subscribe_typed();
        let mut published = run_scoped(turn(), "producer-user", async {
            let mut short = metadata();
            short.ttl = Duration::from_millis(10);
            f.producer
                .browser_present(Some(managed_operation()), short)
                .unwrap()
        })
        .await;
        read_resource(&mut rx);
        // Mint directly with a longer deadline to prove sweep explicitly revokes
        // associated authority instead of relying on its independent TTL.
        let grant = mint(&f, published.resource());
        tokio::time::sleep(Duration::from_millis(30)).await;
        if revise {
            run_scoped(turn(), "producer-user", async {
                assert!(matches!(
                    published.revise(metadata()),
                    Err(PresentError::Registry(ResourceError::Expired))
                ));
            })
            .await;
        } else {
            assert_eq!(f.producer.sweep_expired().unwrap(), 1);
        }
        let closed = read_closed(&mut rx);
        assert_eq!(closed.resource_id, published.resource().resource_id);
        assert_eq!(closed.binding, published.resource().binding);
        assert_eq!(closed.reason, ClosedReason::Expired);
        assert!(f.grants.grant_info(&grant).is_none());
        assert_eq!(f.producer.sweep_expired().unwrap(), 0);
        assert_empty(&mut rx);
    }
}

/// A producer wired to an owner registry (the runtime shape) plus a real
/// fake-engine owner and its operation record for target `T1`.
struct OwnedRig {
    f: Fixture,
    owners: LiveOwnerRegistry,
    owner: Arc<crate::browser::live::LiveProfileOwner>,
    record: BrowserOperationRecord,
    _server: aleph_cdp::testkit::FakeCdpServer,
}

async fn owned_rig() -> OwnedRig {
    let (server, owner, handle) = owner_fixture().await;
    let resources = ResourceRegistry::default();
    let grants = ViewerGrantStore::default();
    let bus = Arc::new(GatewayEventBus::default());
    let owners = LiveOwnerRegistry::default();
    let lifecycle =
        BrowserLiveLifecycle::new(resources.clone(), grants.clone()).with_event_bus(bus.clone());
    let producer = BrowserLiveProducer::new_with_lifecycle(lifecycle, owners.clone());
    let record = BrowserOperationRecord::capture_for_test(
        &owner,
        &handle,
        "T1".into(),
        BrowserOperationKind::Open,
    );
    OwnedRig {
        f: Fixture {
            producer,
            resources,
            grants,
            bus,
        },
        owners,
        owner,
        record,
        _server: server,
    }
}

#[tokio::test]
async fn browser_present_with_a_live_owner_route_publishes() {
    let rig = owned_rig().await;
    let mut rx = rig.f.bus.subscribe_typed();
    let published = run_scoped(turn(), "producer-user", async {
        rig.f
            .producer
            .browser_present(
                Some(BrowserPresentOperation::from_operation_record(&rig.record)),
                metadata(),
            )
            .unwrap()
    })
    .await;
    assert_eq!(read_resource(&mut rx), *published.resource());
    assert!(rig
        .f
        .resources
        .get_active(&published.resource().resource_id)
        .is_some());
    assert_empty(&mut rx);
}

#[tokio::test]
async fn route_lost_after_publish_closes_the_resource_and_reports_owner_gone() {
    let rig = owned_rig().await;
    let f = &rig.f;
    let mut rx = f.bus.subscribe_typed();
    let (lease, fresh) = rig.owners.register_operation_fresh(&rig.record).unwrap();
    assert!(fresh);
    let operation = BrowserPresentOperation::from_operation_record(&rig.record);
    let presentation = metadata();
    let resource = run_scoped(turn(), "producer-user", async {
        let draft = ResourceDraft {
            binding: current_binding(operation.target.clone()).unwrap(),
            kind: presentation.kind,
            display: presentation.display,
            source: operation.source,
            provenance: operation.provenance.clone(),
            redaction: producer_redaction(presentation.redaction),
            ttl: presentation.ttl,
            companion_candidates: presentation.companion_candidates.clone(),
        };
        f.producer.lifecycle().publish(draft).unwrap()
    })
    .await;
    assert_eq!(read_resource(&mut rx), resource);
    let grant = mint(f, &resource);

    // Control: while the route still resolves, completion keeps the resource.
    let kept = f
        .producer
        .complete_publication(Some((&rig.owners, &lease, true)), resource.clone())
        .unwrap();
    assert_eq!(*kept.resource(), resource);
    assert_empty(&mut rx);
    assert!(f.grants.grant_info(&grant).is_some());

    // The owner ends in the publish window: no terminal sink is installed on
    // this registry, so nothing else would ever close the resource.
    rig.owner.retire();
    assert!(rig.owners.accepts(&lease).is_err());
    let result = f
        .producer
        .complete_publication(Some((&rig.owners, &lease, true)), resource.clone());
    assert!(
        matches!(result, Err(PresentError::OwnerGone)),
        "{:?}",
        result.err()
    );
    let closed = read_closed(&mut rx);
    assert_eq!(closed.resource_id, resource.resource_id);
    // Owner ended (not a target destroy): same reason the terminal sink uses.
    assert_eq!(closed.reason, ClosedReason::EngineExited);
    // The fresh registration was compare-retired: no ended owner left behind.
    assert_eq!(rig.owners.active_count(), 0);
    assert!(f.resources.get_active(&resource.resource_id).is_none());
    assert!(f.grants.grant_info(&grant).is_none());
    assert_empty(&mut rx);
}

#[tokio::test]
async fn pre_existing_registration_is_not_retired_when_the_route_is_lost() {
    let rig = owned_rig().await;
    let f = &rig.f;
    let (lease, fresh) = rig.owners.register_operation_fresh(&rig.record).unwrap();
    assert!(fresh);
    let operation = BrowserPresentOperation::from_operation_record(&rig.record);
    let presentation = metadata();
    let resource = run_scoped(turn(), "producer-user", async {
        let draft = ResourceDraft {
            binding: current_binding(operation.target.clone()).unwrap(),
            kind: presentation.kind,
            display: presentation.display,
            source: operation.source,
            provenance: operation.provenance.clone(),
            redaction: producer_redaction(presentation.redaction),
            ttl: presentation.ttl,
            companion_candidates: presentation.companion_candidates.clone(),
        };
        f.producer.lifecycle().publish(draft).unwrap()
    })
    .await;
    rig.owner.retire();
    let result = f
        .producer
        .complete_publication(Some((&rig.owners, &lease, false)), resource);
    assert!(matches!(result, Err(PresentError::OwnerGone)));
    // Not fresh: the pre-existing registration stays (only a fresh one is retired).
    assert_eq!(rig.owners.active_count(), 1);
}

#[tokio::test]
async fn runtime_owner_registration_failure_is_owner_gone_not_not_configured() {
    let rig = owned_rig().await;
    let mut rx = rig.f.bus.subscribe_typed();
    // The owner already ended: the exact route cannot register.
    rig.owner.retire();
    run_scoped(turn(), "producer-user", async {
        assert!(matches!(
            rig.f.producer.browser_present(
                Some(BrowserPresentOperation::from_operation_record(&rig.record)),
                metadata(),
            ),
            Err(PresentError::OwnerGone)
        ));
    })
    .await;
    assert_eq!(rig.owners.active_count(), 0);
    assert_empty(&mut rx);
}
