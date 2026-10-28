//! Stage D resource matrices: Task 1 establishes real positive controls for the
//! Generic `WebhookChannel` and the test-only `PollingSocketChannel`; Tasks 2–3
//! add the shared lifecycle and failure matrices.
//!
//! Every receipt asserted here is recorded at a real resource boundary by the
//! shape that owns it; no count is derived from lifecycle state.

use super::binding::{
    BoundCompletion, ChannelBindingError, ChannelInitialState, ChannelRuntimePhase,
};
use super::test_support::{
    lifecycle_contract, register_bound, FailingStopWebhook, LoopbackPeer, LoopbackSocket,
    PollingReceipts, PollingSocketChannel, ResourceProbe, CACHE_CREATED, CACHE_DROP,
    DATABASE_CLOSED, DATABASE_DROP, DATABASE_OPENED, DATABASE_OPEN_ATTEMPT, SEND_ATTEMPT,
    SEND_COMPLETED, TCP_CONNECTED, TCP_CONNECT_ATTEMPT, TCP_DROP, TIMER_CREATED, TIMER_DROP,
    TIMER_TICK, WORKER_EXITED, WORKER_JOINED, WORKER_SPAWNED,
};
use super::{ChannelId, ChannelRegistry, OwnershipTree, VisibilityScope};
use crate::capability::ownership::{BindingValidationError, OwnerRef, RunId};
use crate::capability::provider::ProviderState;
use crate::gateway::channel::Channel;
use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
use crate::sync_primitives::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

async fn send_once(registry: &Arc<ChannelRegistry>, id: &ChannelId, text: &str) {
    let capture = registry.capture_bound_channel(id).await.unwrap();
    let observation = registry
        .resolve_bound_channel(capture, &VisibilityScope::default())
        .await
        .unwrap();
    let completion = registry
        .send_bound_once(observation, super::OutboundMessage::text("conv-1", text))
        .await
        .unwrap()
        .await
        .unwrap();
    assert!(
        matches!(completion, BoundCompletion::Accepted(Ok(_))),
        "the active fixture must complete the send it accepted"
    );
}

/// Resources owned by one polling incarnation, derived only from construction
/// and destruction receipts.
#[derive(Debug, PartialEq, Eq)]
struct Active {
    stream: usize,
    worker: usize,
    timer: usize,
    cache: usize,
    database: usize,
}

fn active(receipts: &PollingReceipts) -> Active {
    let count = |event: &str| receipts.count(event);
    let live = |created: usize, destroyed: usize| {
        created
            .checked_sub(destroyed)
            .expect("resource destruction without a matching construction")
    };
    Active {
        stream: live(count(TCP_CONNECTED), count(TCP_DROP)),
        worker: live(count(WORKER_SPAWNED), count(WORKER_JOINED)),
        timer: live(count(TIMER_CREATED), count(TIMER_DROP)),
        cache: live(count(CACHE_CREATED), count(CACHE_DROP)),
        database: live(
            count(DATABASE_OPENED),
            count(DATABASE_CLOSED) + count(DATABASE_DROP),
        ),
    }
}

/// Zero means "nothing was ever allocated" only if the same observer reports
/// real receipts when the shape is active; this control proves that first.
#[tokio::test]
async fn active_resource_observers_have_positive_controls() {
    webhook_positive_control().await;
    polling_positive_control().await;
}

async fn webhook_positive_control() {
    let peer = LoopbackPeer::new(200).await;
    let arrived = peer.arrived;
    let probe = ResourceProbe::default();
    let registry = Arc::new(ChannelRegistry::new());
    let tree = Arc::new(OwnershipTree::new());
    let id = register_bound(
        &registry,
        &tree,
        Box::new(WebhookChannel::observed(
            "control-webhook",
            WebhookChannelConfig {
                secret: "test-secret".into(),
                callback_url: peer.url.clone(),
                ..Default::default()
            },
            probe.clone(),
        )),
        ChannelInitialState::Cold,
    )
    .await;

    registry
        .start_bound_channel_compat(&id)
        .await
        .expect("the real webhook activates");
    assert!(
        probe.count("client_build_attempt") >= 1,
        "webhook start must attempt a real client build"
    );
    assert!(
        probe.count("client_built") >= 1,
        "webhook start must construct the client"
    );
    assert_eq!(
        probe.count("client_drop"),
        0,
        "an active webhook still owns its client"
    );

    let capture = registry.capture_bound_channel(&id).await.unwrap();
    let observation = registry
        .resolve_bound_channel(capture, &VisibilityScope::default())
        .await
        .unwrap();
    let completion = registry
        .send_bound_once(
            observation,
            super::OutboundMessage::text("conv-1", "control payload"),
        )
        .await
        .unwrap();
    let bytes = tokio::time::timeout(Duration::from_secs(10), arrived)
        .await
        .expect("the loopback peer must receive a request within ten seconds")
        .expect("the loopback peer received the request");
    assert!(
        String::from_utf8_lossy(&bytes).contains("control payload"),
        "the loopback peer must receive the real outbound request"
    );
    peer.release.send(()).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(10), completion)
            .await
            .expect("the webhook send must complete within ten seconds")
            .expect("the send completes"),
        super::binding::BoundCompletion::Accepted(Ok(_))
    ));
    assert!(probe.count("request_attempt") >= 1);
    assert!(probe.count("request_ended") >= 1);
    tokio::time::timeout(Duration::from_secs(10), peer.task)
        .await
        .expect("the webhook peer task must finish within ten seconds")
        .expect("the loopback peer task finishes");

    registry
        .stop_bound_channel_compat(&id)
        .await
        .expect("the active webhook stops");
    assert!(
        probe.count("client_drop") >= 1,
        "stop must destroy the real client"
    );
    assert_eq!(
        probe.count("client_built"),
        probe.count("client_drop"),
        "the owning channel must retain its client handle across the observable stop"
    );
}

async fn polling_positive_control() {
    let peer = LoopbackSocket::new().await;
    let directory = tempfile::TempDir::new().unwrap();
    let (receipts, mut ticks) = PollingReceipts::new();
    let registry = Arc::new(ChannelRegistry::new());
    let tree = Arc::new(OwnershipTree::new());
    let id = register_bound(
        &registry,
        &tree,
        Box::new(PollingSocketChannel::new(
            "control-polling",
            peer.address,
            directory.path().join("polling.sqlite3"),
            receipts.clone(),
        )),
        ChannelInitialState::Cold,
    )
    .await;

    assert_eq!(
        active(&receipts),
        Active {
            stream: 0,
            worker: 0,
            timer: 0,
            cache: 0,
            database: 0,
        },
        "cold registration must allocate no provider resource"
    );

    registry
        .start_bound_channel_compat(&id)
        .await
        .expect("the polling fixture activates");
    assert_eq!(
        receipts.count(TCP_CONNECT_ATTEMPT),
        1,
        "start must attempt exactly one loopback connect"
    );
    assert_eq!(
        receipts.count(TCP_CONNECTED),
        1,
        "start must record the real loopback connection"
    );
    assert_eq!(
        receipts.count(WORKER_SPAWNED),
        1,
        "start must spawn a worker"
    );
    assert_eq!(receipts.count(TIMER_CREATED), 1, "start must own a timer");
    assert_eq!(receipts.count(CACHE_CREATED), 1, "start must own a cache");
    assert_eq!(
        receipts.count(DATABASE_OPEN_ATTEMPT),
        1,
        "start must attempt the real SQLite open"
    );
    assert_eq!(
        receipts.count(DATABASE_OPENED),
        1,
        "start must report the real SQLite open"
    );
    let ticked = tokio::time::timeout(Duration::from_secs(10), ticks.changed()).await;
    assert!(
        matches!(ticked, Ok(Ok(_))),
        "the worker must tick inside the bounded control window"
    );
    assert!(receipts.count(TIMER_TICK) >= 1, "a tick is a real receipt");
    assert_eq!(
        active(&receipts),
        Active {
            stream: 1,
            worker: 1,
            timer: 1,
            cache: 1,
            database: 1,
        },
        "an active polling incarnation owns every resource it reported"
    );

    send_once(&registry, &id, "control payload").await;
    assert_eq!(receipts.count(SEND_ATTEMPT), 1);
    assert_eq!(receipts.count(SEND_COMPLETED), 1);
    assert_eq!(receipts.payloads(), vec!["control payload".to_string()]);

    registry
        .stop_bound_channel_compat(&id)
        .await
        .expect("the active polling fixture stops");
    assert_eq!(
        receipts.count(WORKER_EXITED),
        1,
        "the worker must report its own normal exit"
    );
    assert_eq!(
        receipts.count(WORKER_JOINED),
        1,
        "stop must join the stored worker handle"
    );
    assert_eq!(
        receipts.count(DATABASE_CLOSED),
        1,
        "the worker must close SQLite explicitly"
    );
    assert_eq!(
        active(&receipts),
        Active {
            stream: 0,
            worker: 0,
            timer: 0,
            cache: 0,
            database: 0,
        },
        "measured active resources must return to zero after the join"
    );
    let mut closed = peer.closed;
    tokio::time::timeout(Duration::from_secs(10), &mut closed)
        .await
        .expect("the peer must observe the closed stream")
        .expect("the peer reports its own EOF");
    tokio::time::timeout(Duration::from_secs(10), peer.task)
        .await
        .expect("the polling peer task must finish within ten seconds")
        .expect("the loopback peer task finishes");
}

// --- Task 2: cold, active and replacement matrices for both shapes ---

/// The two real observability shapes. One is the production webhook channel,
/// the other a test-only polling transport; both are registered through the
/// same production binding APIs.
#[derive(Clone, Copy)]
enum Shape {
    RealWebhook,
    PollingSocket,
}

impl Shape {
    const BOTH: [Shape; 2] = [Shape::RealWebhook, Shape::PollingSocket];

    fn label(self) -> &'static str {
        match self {
            Self::RealWebhook => "real-webhook",
            Self::PollingSocket => "polling-socket",
        }
    }

    fn channel_id(self) -> &'static str {
        match self {
            Self::RealWebhook => "matrix-webhook",
            Self::PollingSocket => "matrix-polling",
        }
    }
}

/// Which ownership binding the registration carries. `WrongOwner` overwrites
/// the binding the registration minted, so the proof no longer matches.
#[derive(Clone, Copy)]
enum Ownership {
    Registered,
    Missing,
    WrongOwner,
}

fn state_label(initial: ChannelInitialState) -> &'static str {
    match initial {
        ChannelInitialState::Discovered => "discovered",
        ChannelInitialState::Cold => "cold",
        ChannelInitialState::Disabled => "disabled",
    }
}

fn provider_state(initial: ChannelInitialState) -> ProviderState {
    match initial {
        ChannelInitialState::Discovered => ProviderState::Discovered,
        ChannelInitialState::Cold => ProviderState::Cold,
        ChannelInitialState::Disabled => ProviderState::Disabled,
    }
}

/// Resolve failures are the contract; describe an unexpected success without
/// requiring `Debug` on an admitted observation, which carries a live handle.
fn unexpected<T>(result: &Result<T, ChannelBindingError>) -> String {
    match result {
        Ok(_) => "an admissible observation".to_string(),
        Err(error) => format!("{error:?}"),
    }
}

fn zero_active() -> Active {
    Active {
        stream: 0,
        worker: 0,
        timer: 0,
        cache: 0,
        database: 0,
    }
}

fn live_active() -> Active {
    Active {
        stream: 1,
        worker: 1,
        timer: 1,
        cache: 1,
        database: 1,
    }
}

/// Receipts and transport endpoints owned by one matrix row. The matrix body
/// never reads lifecycle state to decide whether a resource exists.
enum Observation {
    Webhook {
        probe: ResourceProbe,
        arrived: Option<oneshot::Receiver<Vec<u8>>>,
        release: Option<oneshot::Sender<()>>,
        task: Option<tokio::task::JoinHandle<()>>,
    },
    Polling {
        receipts: PollingReceipts,
        ticks: tokio::sync::watch::Receiver<u64>,
        closed: Option<oneshot::Receiver<()>>,
        task: Option<tokio::task::JoinHandle<()>>,
        /// Keeps the fixture's temporary SQLite directory alive.
        _directory: tempfile::TempDir,
    },
}

async fn build(shape: Shape, id: &str, live: bool) -> (Box<dyn Channel>, Observation) {
    match shape {
        Shape::RealWebhook => {
            let peer = if live {
                Some(LoopbackPeer::new(200).await)
            } else {
                None
            };
            let url = peer
                .as_ref()
                .map(|peer| peer.url.clone())
                .unwrap_or_else(|| "http://127.0.0.1:1/callback".to_string());
            let probe = ResourceProbe::default();
            let channel = WebhookChannel::observed(
                id,
                WebhookChannelConfig {
                    secret: "matrix-secret".into(),
                    callback_url: url,
                    ..Default::default()
                },
                probe.clone(),
            );
            let observation = match peer {
                Some(LoopbackPeer {
                    arrived,
                    release,
                    task,
                    ..
                }) => Observation::Webhook {
                    probe,
                    arrived: Some(arrived),
                    release: Some(release),
                    task: Some(task),
                },
                None => Observation::Webhook {
                    probe,
                    arrived: None,
                    release: None,
                    task: None,
                },
            };
            (Box::new(channel), observation)
        }
        Shape::PollingSocket => {
            let peer = if live {
                Some(LoopbackSocket::new().await)
            } else {
                None
            };
            let (address, closed, task) = match peer {
                Some(LoopbackSocket {
                    address,
                    closed,
                    task,
                }) => (address, Some(closed), Some(task)),
                None => (
                    "127.0.0.1:1"
                        .parse()
                        .expect("static loopback address parses"),
                    None,
                    None,
                ),
            };
            let directory = tempfile::TempDir::new().unwrap();
            let (receipts, ticks) = PollingReceipts::new();
            let channel = PollingSocketChannel::new(
                id,
                address,
                directory.path().join("matrix.sqlite3"),
                receipts.clone(),
            );
            (
                Box::new(channel),
                Observation::Polling {
                    receipts,
                    ticks,
                    closed,
                    task,
                    _directory: directory,
                },
            )
        }
    }
}

impl Observation {
    /// Cold means zero *attempts* as well as zero held resources: an attempted
    /// connect or open is already evidence of a resource the caller acquired.
    fn assert_cold(&self, stage: &str) {
        match self {
            Self::Webhook { probe, .. } => {
                for event in [
                    "client_build_attempt",
                    "client_built",
                    "client_drop",
                    "request_attempt",
                    "request_ended",
                ] {
                    assert_eq!(
                        probe.count(event),
                        0,
                        "{stage}: a cold operation must not record {event}"
                    );
                }
            }
            Self::Polling { receipts, .. } => {
                for event in [
                    TCP_CONNECT_ATTEMPT,
                    TCP_CONNECTED,
                    TCP_DROP,
                    DATABASE_OPEN_ATTEMPT,
                    DATABASE_OPENED,
                    DATABASE_CLOSED,
                    DATABASE_DROP,
                    WORKER_SPAWNED,
                    WORKER_EXITED,
                    WORKER_JOINED,
                    TIMER_CREATED,
                    TIMER_TICK,
                    TIMER_DROP,
                    CACHE_CREATED,
                    CACHE_DROP,
                    SEND_ATTEMPT,
                    SEND_COMPLETED,
                ] {
                    assert_eq!(
                        receipts.count(event),
                        0,
                        "{stage}: a cold operation must not record {event}"
                    );
                }
                assert_eq!(
                    active(receipts),
                    zero_active(),
                    "{stage}: a cold operation must own no resource"
                );
            }
        }
    }

    fn assert_active(&self, stage: &str) {
        match self {
            Self::Webhook { probe, .. } => {
                assert!(
                    probe.count("client_build_attempt") == 1,
                    "{stage}: start must attempt exactly one real client build"
                );
                assert!(
                    probe.count("client_built") == 1,
                    "{stage}: start must construct exactly one client"
                );
                assert_eq!(
                    probe.count("client_drop"),
                    0,
                    "{stage}: an active webhook still owns its client"
                );
            }
            Self::Polling { receipts, .. } => {
                assert_eq!(
                    receipts.count(TCP_CONNECT_ATTEMPT),
                    1,
                    "{stage}: start must attempt exactly one loopback connect"
                );
                assert_eq!(
                    receipts.count(DATABASE_OPEN_ATTEMPT),
                    1,
                    "{stage}: start must attempt the real SQLite open"
                );
                assert!(
                    receipts.count(TIMER_TICK) >= 1,
                    "{stage}: the active worker must have ticked"
                );
                assert_eq!(
                    active(receipts),
                    live_active(),
                    "{stage}: an active polling incarnation owns every resource it reported"
                );
            }
        }
    }

    fn assert_no_send(&self, stage: &str) {
        match self {
            Self::Webhook { probe, .. } => {
                assert_eq!(
                    probe.count("request_attempt"),
                    0,
                    "{stage}: start must not attempt a request"
                );
                assert_eq!(
                    probe.count("request_ended"),
                    0,
                    "{stage}: start must not end a request"
                );
            }
            Self::Polling { receipts, .. } => {
                assert_eq!(
                    receipts.count(SEND_ATTEMPT),
                    0,
                    "{stage}: start must not attempt a send"
                );
                assert_eq!(
                    receipts.count(SEND_COMPLETED),
                    0,
                    "{stage}: start must not complete a send"
                );
            }
        }
    }

    fn assert_cleaned(&self, stage: &str) {
        match self {
            Self::Webhook { probe, .. } => {
                assert!(
                    probe.count("client_built") >= 1,
                    "{stage}: cleanup is only evidence after a real build"
                );
                assert_eq!(
                    probe.count("client_drop"),
                    probe.count("client_built"),
                    "{stage}: the owning channel must release its client handle on stop"
                );
                assert_eq!(
                    probe.count("request_attempt"),
                    probe.count("request_ended"),
                    "{stage}: no request may survive stop"
                );
            }
            Self::Polling { receipts, .. } => {
                assert_eq!(
                    receipts.count(WORKER_EXITED),
                    1,
                    "{stage}: the worker must report its own normal exit"
                );
                assert_eq!(
                    receipts.count(WORKER_JOINED),
                    1,
                    "{stage}: stop must join the stored worker handle"
                );
                assert_eq!(
                    receipts.count(DATABASE_CLOSED),
                    1,
                    "{stage}: the worker must close SQLite explicitly"
                );
                assert_eq!(
                    active(receipts),
                    zero_active(),
                    "{stage}: measured active resources must return to zero"
                );
            }
        }
    }

    /// Wait until the live shape has proven it is running, using the transport
    /// it owns rather than registry state.
    async fn await_live(&mut self) {
        if let Self::Polling {
            receipts, ticks, ..
        } = self
        {
            let ticked = tokio::time::timeout(Duration::from_secs(10), ticks.changed()).await;
            assert!(
                matches!(ticked, Ok(Ok(_))),
                "the polling worker must tick inside the bounded window"
            );
            assert!(receipts.count(TIMER_TICK) >= 1, "a tick is a real receipt");
        }
    }

    /// Drive one accepted send to completion and prove it arrived at the real
    /// transport the shape owns.
    async fn deliver(&mut self, completion: oneshot::Receiver<BoundCompletion>, payload: &str) {
        match self {
            Self::Webhook {
                probe,
                arrived,
                release,
                ..
            } => {
                let bytes = tokio::time::timeout(
                    Duration::from_secs(10),
                    arrived
                        .take()
                        .expect("the live webhook fixture owns exactly one peer request"),
                )
                .await
                .expect("the loopback peer must receive a request within ten seconds")
                .expect("the loopback peer received the request");
                assert!(
                    String::from_utf8_lossy(&bytes).contains(payload),
                    "the loopback peer must receive the real outbound body"
                );
                release
                    .take()
                    .expect("the live webhook fixture owns the peer release")
                    .send(())
                    .expect("the loopback peer is still waiting for its response");
                assert!(matches!(
                    tokio::time::timeout(Duration::from_secs(10), completion)
                        .await
                        .expect("the webhook send must complete within ten seconds")
                        .expect("the send completes"),
                    BoundCompletion::Accepted(Ok(_))
                ));
                assert!(probe.count("request_attempt") >= 1);
                assert!(probe.count("request_ended") >= 1);
            }
            Self::Polling { receipts, .. } => {
                assert!(matches!(
                    tokio::time::timeout(Duration::from_secs(10), completion)
                        .await
                        .expect("the polling send must complete within ten seconds")
                        .expect("the send completes"),
                    BoundCompletion::Accepted(Ok(_))
                ));
                assert_eq!(
                    receipts.count(SEND_ATTEMPT),
                    1,
                    "the fixture records the attempted payload"
                );
                assert_eq!(receipts.count(SEND_COMPLETED), 1);
                assert_eq!(receipts.payloads(), vec![payload.to_string()]);
            }
        }
    }

    /// Join the loopback peer so the test never leaks its task or socket.
    async fn finish(&mut self) {
        match self {
            Self::Webhook { task, .. } => {
                if let Some(task) = task.take() {
                    tokio::time::timeout(Duration::from_secs(10), task)
                        .await
                        .expect("the webhook peer task must finish within ten seconds")
                        .expect("the loopback peer task finishes");
                }
            }
            Self::Polling { closed, task, .. } => {
                if let Some(closed) = closed.take() {
                    tokio::time::timeout(Duration::from_secs(10), closed)
                        .await
                        .expect("the peer must observe the closed stream")
                        .expect("the peer reports its own EOF");
                }
                if let Some(task) = task.take() {
                    tokio::time::timeout(Duration::from_secs(10), task)
                        .await
                        .expect("the polling peer task must finish within ten seconds")
                        .expect("the loopback peer task finishes");
                }
            }
        }
    }
}

struct Fixture {
    registry: Arc<ChannelRegistry>,
    tree: Arc<OwnershipTree>,
    id: ChannelId,
    observation: Observation,
}

async fn fixture(
    shape: Shape,
    initial: ChannelInitialState,
    live: bool,
    ownership: Ownership,
) -> Fixture {
    let id = ChannelId::new(shape.channel_id());
    let (channel, observation) = build(shape, id.as_str(), live).await;
    let registry = Arc::new(ChannelRegistry::new());
    let tree = Arc::new(OwnershipTree::new());
    let registered = match ownership {
        Ownership::Registered => register_bound(&registry, &tree, channel, initial).await,
        Ownership::Missing => registry
            .register_bound_channel(channel, lifecycle_contract(), tree.clone(), initial)
            .await
            .unwrap(),
        Ownership::WrongOwner => {
            let registered = registry
                .register_bound_channel(channel, lifecycle_contract(), tree.clone(), initial)
                .await
                .unwrap();
            let capture = registry.capture_bound_channel(&registered).await.unwrap();
            tree.register(
                capture.descriptor.id.clone(),
                OwnerRef::Run(RunId("matrix-wrong-owner".into())),
                capture.descriptor.lifetime,
                capture.descriptor.visibility.clone(),
            )
            .unwrap();
            registered
        }
    };
    assert_eq!(
        registered,
        id,
        "{}: registration keeps the channel id",
        shape.label()
    );
    Fixture {
        registry,
        tree,
        id,
        observation,
    }
}

/// M9/M8: every read and admission attempt on a cold or never-started source
/// allocates nothing — no TCP connect attempt, no SQLite open attempt, no
/// client build attempt — for both shapes and all three initial states.
#[tokio::test]
async fn cold_operations_allocate_no_resources_for_either_shape() {
    for shape in Shape::BOTH {
        for initial in [
            ChannelInitialState::Discovered,
            ChannelInitialState::Cold,
            ChannelInitialState::Disabled,
        ] {
            cold_reads_and_admission_fail_closed(shape, initial).await;
            cold_missing_ownership_fails_closed(shape, initial).await;
            cold_wrong_ownership_fails_closed(shape, initial).await;
        }
    }
}

async fn cold_reads_and_admission_fail_closed(shape: Shape, initial: ChannelInitialState) {
    let stage = |step: &str| format!("{} {}: {step}", shape.label(), state_label(initial));
    let fixture = fixture(shape, initial, false, Ownership::Registered).await;
    let expected = provider_state(initial);
    fixture.observation.assert_cold(&stage("register"));

    let listed = fixture.registry.list().await;
    assert_eq!(listed.len(), 1, "{}", stage("list"));
    assert_eq!(listed[0].id, fixture.id, "{}", stage("list id"));
    assert_eq!(
        fixture.registry.status_summary().await.total,
        1,
        "{}",
        stage("status summary")
    );
    assert_eq!(
        fixture.registry.health_states().await.len(),
        1,
        "{}",
        stage("health states")
    );
    let (descriptors, _) = fixture
        .registry
        .snapshot_bound_channels(&VisibilityScope::default())
        .await;
    assert_eq!(descriptors.len(), 1, "{}", stage("discovery"));
    assert_eq!(descriptors[0].id.name, fixture.id.as_str());
    fixture.observation.assert_cold(&stage("read projections"));

    let capture = fixture
        .registry
        .capture_bound_channel(&fixture.id)
        .await
        .unwrap();
    assert_eq!(
        capture.provider.state(),
        expected,
        "{}",
        stage("capture keeps the declared initial state")
    );
    fixture.observation.assert_cold(&stage("capture"));

    match fixture
        .registry
        .resolve_bound_channel(capture.clone(), &VisibilityScope::default())
        .await
    {
        Err(ChannelBindingError::Provider(error)) => {
            assert_eq!(
                error.state,
                expected,
                "{}",
                stage("cold resolve reports its real provider state")
            );
            assert_ne!(
                error.state,
                ProviderState::Ready,
                "{}: a never-started source is not Ready",
                stage("cold resolve")
            );
        }
        other => panic!(
            "{}: cold resolve must fail provider admission, got {}",
            stage("cold resolve"),
            unexpected(&other)
        ),
    }
    assert_eq!(
        capture.provider.state(),
        expected,
        "{}: a rejected admission must not start the channel",
        stage("cold resolve")
    );
    fixture.observation.assert_cold(&stage("cold resolve"));

    let wrong_scope = VisibilityScope {
        principal: Some("somebody-else".into()),
        ..VisibilityScope::default()
    };
    match fixture
        .registry
        .resolve_bound_channel(capture.clone(), &wrong_scope)
        .await
    {
        Err(ChannelBindingError::WrongVisibility) => {}
        other => panic!(
            "{}: wrong visibility must fail closed, got {}",
            stage("wrong visibility"),
            unexpected(&other)
        ),
    }
    fixture.observation.assert_cold(&stage("wrong visibility"));

    fixture
        .registry
        .unregister_bound_channel(&fixture.id)
        .await
        .unwrap();
    assert!(
        fixture
            .registry
            .capture_bound_channel(&fixture.id)
            .await
            .is_err(),
        "{}: an unregistered source cannot be captured",
        stage("unregister")
    );
    match fixture
        .registry
        .resolve_bound_channel(capture, &VisibilityScope::default())
        .await
    {
        Err(ChannelBindingError::NotFound) => {}
        other => panic!(
            "{}: an old proof must not resolve an unregistered source, got {}",
            stage("old proof"),
            unexpected(&other)
        ),
    }
    fixture.observation.assert_cold(&stage("old proof"));
    assert_eq!(
        fixture.registry.list().await.len(),
        0,
        "{}",
        stage("unregistered list")
    );
    fixture.observation.assert_cold(&stage("final"));
}

async fn cold_missing_ownership_fails_closed(shape: Shape, initial: ChannelInitialState) {
    let stage = |step: &str| format!("{} {}: {step}", shape.label(), state_label(initial));
    let fixture = fixture(shape, initial, false, Ownership::Missing).await;
    fixture.observation.assert_cold(&stage("register"));

    let capture = fixture
        .registry
        .capture_bound_channel(&fixture.id)
        .await
        .unwrap();
    match fixture
        .registry
        .resolve_bound_channel(capture.clone(), &VisibilityScope::default())
        .await
    {
        Err(ChannelBindingError::Owner(BindingValidationError::MissingBinding)) => {}
        other => panic!(
            "{}: missing ownership must fail closed, got {}",
            stage("missing ownership"),
            unexpected(&other)
        ),
    }
    fixture.observation.assert_cold(&stage("missing ownership"));

    fixture
        .registry
        .unregister_bound_channel(&fixture.id)
        .await
        .unwrap();
    fixture.observation.assert_cold(&stage("unregister"));
    match fixture
        .registry
        .resolve_bound_channel(capture, &VisibilityScope::default())
        .await
    {
        Err(ChannelBindingError::NotFound) => {}
        other => panic!(
            "{}: an old proof must not resolve an unregistered source, got {}",
            stage("old proof"),
            unexpected(&other)
        ),
    }
    fixture.observation.assert_cold(&stage("old proof"));
}

async fn cold_wrong_ownership_fails_closed(shape: Shape, initial: ChannelInitialState) {
    let stage = |step: &str| format!("{} {}: {step}", shape.label(), state_label(initial));
    let fixture = fixture(shape, initial, false, Ownership::WrongOwner).await;
    fixture.observation.assert_cold(&stage("register"));

    let capture = fixture
        .registry
        .capture_bound_channel(&fixture.id)
        .await
        .unwrap();
    match fixture
        .registry
        .resolve_bound_channel(capture.clone(), &VisibilityScope::default())
        .await
    {
        Err(ChannelBindingError::Owner(BindingValidationError::BindingMismatch)) => {}
        other => panic!(
            "{}: a mismatched owner must fail closed, got {}",
            stage("wrong ownership"),
            unexpected(&other)
        ),
    }
    fixture.observation.assert_cold(&stage("wrong ownership"));

    fixture
        .registry
        .unregister_bound_channel(&fixture.id)
        .await
        .unwrap();
    fixture.observation.assert_cold(&stage("unregister"));
    match fixture
        .registry
        .resolve_bound_channel(capture, &VisibilityScope::default())
        .await
    {
        Err(ChannelBindingError::NotFound) => {}
        other => panic!(
            "{}: an old proof must not resolve an unregistered source, got {}",
            stage("old proof"),
            unexpected(&other)
        ),
    }
    fixture.observation.assert_cold(&stage("old proof"));
}

/// The active half of the matrix: real activation, the shape's observed send
/// path, real source-owned stop with zero observed active resources, and an old
/// proof that cannot outlive its provider.
#[tokio::test]
async fn active_send_stop_and_old_proof_contracts_hold_for_either_shape() {
    for shape in Shape::BOTH {
        let stage = |step: &str| format!("{}: {step}", shape.label());
        let mut fixture = fixture(
            shape,
            ChannelInitialState::Cold,
            true,
            Ownership::Registered,
        )
        .await;
        fixture.observation.assert_cold(&stage("cold baseline"));

        fixture
            .registry
            .start_bound_channel_compat(&fixture.id)
            .await
            .expect("the shape activates");
        fixture.observation.await_live().await;
        fixture.observation.assert_active(&stage("start"));
        fixture.observation.assert_no_send(&stage("start"));

        let capture = fixture
            .registry
            .capture_bound_channel(&fixture.id)
            .await
            .unwrap();
        let observation = fixture
            .registry
            .resolve_bound_channel(capture.clone(), &VisibilityScope::default())
            .await
            .expect("an active provider admits a correct proof");
        let completion = fixture
            .registry
            .send_bound_once(
                observation,
                super::OutboundMessage::text("conv-1", "matrix payload"),
            )
            .await
            .expect("the active source admits one send");
        fixture
            .observation
            .deliver(completion, "matrix payload")
            .await;
        fixture.observation.assert_active(&stage("send"));

        fixture
            .registry
            .stop_bound_channel_compat(&fixture.id)
            .await
            .expect("the shape stops");
        fixture.observation.assert_cleaned(&stage("stop"));
        fixture.observation.finish().await;
        fixture.observation.assert_cleaned(&stage("finish"));

        match fixture
            .registry
            .resolve_bound_channel(capture, &VisibilityScope::default())
            .await
        {
            Err(ChannelBindingError::Provider(_)) => {}
            other => panic!(
                "{}: a stopped provider must reject the old proof, got {}",
                stage("post-stop proof"),
                unexpected(&other)
            ),
        }
        fixture
            .registry
            .unregister_bound_channel(&fixture.id)
            .await
            .expect("the stopped source unregisters");
        assert!(matches!(
            fixture.registry.capture_bound_channel(&fixture.id).await,
            Err(ChannelBindingError::NotFound)
        ));
        fixture.observation.assert_cleaned(&stage("unregister"));
    }
}

/// Replacement retires the incumbent's resources and publishes a fresh
/// same-ID source whose own proof is the only one that works.
#[tokio::test]
async fn replacement_cleans_old_resources_and_rejects_old_provider_proof() {
    for shape in Shape::BOTH {
        let stage = |step: &str| format!("{}: {step}", shape.label());
        let mut old = fixture(
            shape,
            ChannelInitialState::Cold,
            true,
            Ownership::Registered,
        )
        .await;
        old.observation.assert_cold(&stage("old cold baseline"));
        old.registry
            .start_bound_channel_compat(&old.id)
            .await
            .expect("the old incarnation activates");
        old.observation.await_live().await;
        old.observation.assert_active(&stage("old start"));
        old.observation.assert_no_send(&stage("old start"));

        let old_capture = old.registry.capture_bound_channel(&old.id).await.unwrap();
        let old_observation = old
            .registry
            .resolve_bound_channel(old_capture.clone(), &VisibilityScope::default())
            .await
            .expect("the old incarnation admits a correct proof");
        let completion = old
            .registry
            .send_bound_once(
                old_observation.clone(),
                super::OutboundMessage::text("conv-1", "old payload"),
            )
            .await
            .expect("the old incarnation admits one send");
        old.observation.deliver(completion, "old payload").await;
        old.observation.assert_active(&stage("old send"));

        let (candidate, mut fresh) = build(shape, old.id.as_str(), true).await;
        fresh.assert_cold(&stage("fresh candidate"));
        let replacement = old
            .registry
            .replace_bound_channel(
                &old.id,
                candidate,
                lifecycle_contract(),
                old.tree.clone(),
                ChannelInitialState::Cold,
            )
            .await
            .expect("replacement retires the incumbent and publishes the candidate");
        assert_eq!(replacement, old.id, "{}", stage("replacement id"));

        old.observation.assert_cleaned(&stage("retired resources"));
        old.observation.finish().await;

        match old
            .registry
            .resolve_bound_channel(old_capture.clone(), &VisibilityScope::default())
            .await
        {
            Err(ChannelBindingError::StaleEntry) => {}
            other => panic!(
                "{}: the old capture must not resolve the replacement, got {}",
                stage("old resolve"),
                unexpected(&other)
            ),
        }
        match old
            .registry
            .send_bound_once(
                old_observation,
                super::OutboundMessage::text("conv-1", "stale call"),
            )
            .await
        {
            Err(ChannelBindingError::StaleEntry) => {}
            other => panic!(
                "{}: the old proof must not call the replacement, got {}",
                stage("old call"),
                unexpected(&other)
            ),
        }
        fresh.assert_cold(&stage("rejected old proof"));

        let cold_capture = old.registry.capture_bound_channel(&old.id).await.unwrap();
        assert!(
            !old_capture.provider.same_instance(&cold_capture.provider),
            "{}: the replacement must own a fresh provider instance",
            stage("fresh provider")
        );
        assert_ne!(
            old_capture.entry_revision,
            cold_capture.entry_revision,
            "{}",
            stage("fresh entry revision")
        );
        assert_eq!(
            cold_capture.provider.state(),
            ProviderState::Cold,
            "{}: the candidate starts cold",
            stage("candidate state")
        );
        fresh.assert_cold(&stage("candidate cold"));

        old.registry
            .start_bound_channel_compat(&old.id)
            .await
            .expect("the replacement activates");
        fresh.await_live().await;
        fresh.assert_active(&stage("replacement start"));
        fresh.assert_no_send(&stage("replacement start"));
        match old
            .registry
            .resolve_bound_channel(cold_capture, &VisibilityScope::default())
            .await
        {
            Err(ChannelBindingError::Provider(_)) => {}
            other => panic!(
                "{}: activation must reject a cold-generation proof, got {}",
                stage("stale cold proof"),
                unexpected(&other)
            ),
        }
        // Activation advances provider generation; capture the proof after
        // start, just as production callers must, instead of reusing a cold one.
        let new_capture = old.registry.capture_bound_channel(&old.id).await.unwrap();
        let observation = old
            .registry
            .resolve_bound_channel(new_capture, &VisibilityScope::default())
            .await
            .expect("the replacement admits its own proof");
        let completion = old
            .registry
            .send_bound_once(
                observation,
                super::OutboundMessage::text("conv-1", "replacement payload"),
            )
            .await
            .expect("the replacement admits one send");
        fresh.deliver(completion, "replacement payload").await;
        fresh.assert_active(&stage("replacement send"));

        old.registry
            .stop_bound_channel_compat(&old.id)
            .await
            .expect("the replacement stops");
        fresh.assert_cleaned(&stage("replacement stop"));
        fresh.finish().await;
        fresh.assert_cleaned(&stage("replacement finish"));

        old.registry
            .unregister_bound_channel(&old.id)
            .await
            .expect("the replacement unregisters");
        assert!(matches!(
            old.registry.capture_bound_channel(&old.id).await,
            Err(ChannelBindingError::NotFound)
        ));
        fresh.assert_cleaned(&stage("replacement unregister"));
    }
}

// --- Task 3: activation and cleanup failure evidence ---

/// A failure that happens before the first real acquisition must not fabricate
/// a receipt, and a failure that happens after one must release it.
#[tokio::test]
async fn start_failures_do_not_fabricate_resources_or_cleanup() {
    webhook_invalid_config_fails_before_client_attempt().await;
    polling_closed_port_fails_without_a_connection().await;
    polling_partial_activation_drops_the_connected_stream().await;
    polling_schema_failure_drops_the_open_database().await;
}

fn expect_start_failure(result: Result<(), ChannelBindingError>, stage: &str) -> String {
    match result {
        Err(ChannelBindingError::Runtime {
            phase,
            error,
            residual,
        }) => {
            assert_eq!(
                phase,
                ChannelRuntimePhase::Start,
                "{stage}: a failed activation reports its own phase, got {error}"
            );
            assert_eq!(
                residual.active_work, 0,
                "{stage}: failed activation must not retain active work"
            );
            assert!(
                !residual.unjoined_forwarder,
                "{stage}: failed activation must not retain a forwarder"
            );
            // Start admission pre-clears these fields. Keep them as regression
            // guards, not as evidence of an acquired residual resource.
            error
        }
        other => panic!(
            "{stage}: activation must fail closed, got {}",
            unexpected(&other)
        ),
    }
}

/// Generic Webhook binds no socket during `start`, so an invalid real config is
/// the honest pre-acquisition failure: nothing is attempted and nothing is
/// destroyed.
async fn webhook_invalid_config_fails_before_client_attempt() {
    let stage = "real-webhook invalid config";
    let probe = ResourceProbe::default();
    let registry = Arc::new(ChannelRegistry::new());
    let tree = Arc::new(OwnershipTree::new());
    let id = register_bound(
        &registry,
        &tree,
        Box::new(WebhookChannel::observed(
            "failing-webhook",
            WebhookChannelConfig::default(),
            probe.clone(),
        )),
        ChannelInitialState::Cold,
    )
    .await;
    assert_cold_probe(&probe, stage);

    let error = expect_start_failure(registry.start_bound_channel_compat(&id).await, stage);
    assert!(
        error.contains("secret is required"),
        "{stage}: the failure must be real config validation, got {}",
        error
    );
    assert_cold_probe(&probe, stage);

    registry
        .stop_bound_channel_compat(&id)
        .await
        .expect("a failed activation joins its own cleanup path");
    assert_cold_probe(&probe, stage);
    registry
        .unregister_bound_channel(&id)
        .await
        .expect("a failed source unregisters after its cleanup");
}

fn assert_cold_probe(probe: &ResourceProbe, stage: &str) {
    for event in [
        "client_build_attempt",
        "client_built",
        "client_drop",
        "request_attempt",
        "request_ended",
    ] {
        assert_eq!(
            probe.count(event),
            0,
            "{stage}: a failed activation before any acquisition must not record {event}"
        );
    }
}

/// A real connect against a released loopback address records the attempt and
/// nothing else: the failure is the connection, so no later resource exists.
async fn polling_closed_port_fails_without_a_connection() {
    let stage = "polling closed port";
    // Bind then release a loopback port so the target is a real address with no
    // listener, rather than a privileged port whose behavior varies by host.
    let address = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };
    let directory = tempfile::TempDir::new().unwrap();
    let (receipts, _ticks) = PollingReceipts::new();
    let registry = Arc::new(ChannelRegistry::new());
    let tree = Arc::new(OwnershipTree::new());
    let id = register_bound(
        &registry,
        &tree,
        Box::new(PollingSocketChannel::new(
            "failing-connect-polling",
            address,
            directory.path().join("closed-port.sqlite3"),
            receipts.clone(),
        )),
        ChannelInitialState::Cold,
    )
    .await;
    assert_eq!(
        active(&receipts),
        zero_active(),
        "{stage}: cold registration owns no resource"
    );

    let error = expect_start_failure(registry.start_bound_channel_compat(&id).await, stage);
    assert!(
        error.contains("polling loopback connect failed"),
        "{stage}: the failure must cross the real connect boundary, got {}",
        error
    );
    assert_eq!(
        receipts.count(TCP_CONNECT_ATTEMPT),
        1,
        "{stage}: the failed start still records the real connect attempt"
    );
    assert_eq!(
        receipts.count(TCP_CONNECTED),
        0,
        "{stage}: a refused connect must not report a connection"
    );
    for event in [
        DATABASE_OPEN_ATTEMPT,
        DATABASE_OPENED,
        WORKER_SPAWNED,
        TIMER_CREATED,
        CACHE_CREATED,
    ] {
        assert_eq!(
            receipts.count(event),
            0,
            "{stage}: a failed connect must not reach {event}"
        );
    }
    assert_eq!(
        active(&receipts),
        zero_active(),
        "{stage}: measured active resources stay zero"
    );

    registry
        .stop_bound_channel_compat(&id)
        .await
        .expect("a failed activation joins its own cleanup path");
    registry
        .unregister_bound_channel(&id)
        .await
        .expect("the failed source unregisters");
    assert_eq!(active(&receipts), zero_active(), "{stage}: final");
}

/// A real TCP connection followed by a real SQLite failure must release the
/// stream it already holds and never publish a worker, cache or timer.
async fn polling_partial_activation_drops_the_connected_stream() {
    let stage = "polling partial activation";
    let peer = LoopbackSocket::new().await;
    let directory = tempfile::TempDir::new().unwrap();
    // A missing parent directory makes the real SQLite open fail after the real
    // connection succeeded, so the mid-activation boundary is genuinely crossed.
    let unopenable = directory.path().join("missing").join("polling.sqlite3");
    let (receipts, _ticks) = PollingReceipts::new();
    let registry = Arc::new(ChannelRegistry::new());
    let tree = Arc::new(OwnershipTree::new());
    let id = register_bound(
        &registry,
        &tree,
        Box::new(PollingSocketChannel::new(
            "partial-polling",
            peer.address,
            unopenable,
            receipts.clone(),
        )),
        ChannelInitialState::Cold,
    )
    .await;
    assert_eq!(active(&receipts), zero_active(), "{stage}: cold baseline");

    let error = expect_start_failure(registry.start_bound_channel_compat(&id).await, stage);
    assert!(
        error.contains("polling sqlite open failed"),
        "{stage}: the failure must cross the real SQLite boundary, got {}",
        error
    );
    assert_eq!(receipts.count(TCP_CONNECT_ATTEMPT), 1, "{stage}");
    assert_eq!(
        receipts.count(TCP_CONNECTED),
        1,
        "{stage}: the connect really succeeded before the open failed"
    );
    assert_eq!(
        receipts.count(TCP_DROP),
        1,
        "{stage}: the already-connected stream must be released before the error escapes"
    );
    assert_eq!(
        receipts.count(DATABASE_OPEN_ATTEMPT),
        1,
        "{stage}: the open was attempted"
    );
    assert_eq!(
        receipts.count(DATABASE_OPENED),
        0,
        "{stage}: the open must not report success"
    );
    for event in [
        DATABASE_CLOSED,
        DATABASE_DROP,
        WORKER_SPAWNED,
        WORKER_EXITED,
        WORKER_JOINED,
        TIMER_CREATED,
        CACHE_CREATED,
        SEND_ATTEMPT,
    ] {
        assert_eq!(
            receipts.count(event),
            0,
            "{stage}: a partial activation must not record {event}"
        );
    }
    assert_eq!(
        active(&receipts),
        zero_active(),
        "{stage}: measured active resources must return to zero"
    );

    // Peer-observed EOF: the stream was really closed, not merely uncounted.
    let mut closed = peer.closed;
    tokio::time::timeout(Duration::from_secs(10), &mut closed)
        .await
        .expect("the peer must observe the closed stream within ten seconds")
        .expect("the peer reports its own EOF");
    tokio::time::timeout(Duration::from_secs(10), peer.task)
        .await
        .expect("the polling peer task must finish within ten seconds")
        .expect("the loopback peer task finishes");

    registry
        .stop_bound_channel_compat(&id)
        .await
        .expect("a failed activation joins its own cleanup path");
    registry
        .unregister_bound_channel(&id)
        .await
        .expect("the failed source unregisters");
    assert_eq!(active(&receipts), zero_active(), "{stage}: final");
}

/// A real SQLite schema error occurs after a successful open. The guard must
/// record both boundaries so this failure cannot look like an inactive observer.
async fn polling_schema_failure_drops_the_open_database() {
    let stage = "polling schema failure";
    let peer = LoopbackSocket::new().await;
    let directory = tempfile::TempDir::new().unwrap();
    let database_path = directory.path().join("schema.sqlite3");
    // Test infrastructure pre-creates the fixture table. The subsequent
    // provider-owned open succeeds, while its real CREATE TABLE fails.
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    connection
        .execute_batch("CREATE TABLE polling_ticks (tick INTEGER NOT NULL)")
        .unwrap();
    connection.close().unwrap();
    let (receipts, _ticks) = PollingReceipts::new();
    let registry = Arc::new(ChannelRegistry::new());
    let tree = Arc::new(OwnershipTree::new());
    let id = register_bound(
        &registry,
        &tree,
        Box::new(PollingSocketChannel::new(
            "schema-failure",
            peer.address,
            database_path,
            receipts.clone(),
        )),
        ChannelInitialState::Cold,
    )
    .await;
    assert_eq!(active(&receipts), zero_active(), "{stage}: cold");

    let error = expect_start_failure(registry.start_bound_channel_compat(&id).await, stage);
    assert!(
        error.contains("polling sqlite schema failed"),
        "{stage}: the failure must cross the real schema boundary, got {error}"
    );
    for event in [
        TCP_CONNECT_ATTEMPT,
        TCP_CONNECTED,
        TCP_DROP,
        DATABASE_OPEN_ATTEMPT,
        DATABASE_OPENED,
        DATABASE_DROP,
    ] {
        assert_eq!(receipts.count(event), 1, "{stage}: {event}");
    }
    assert_eq!(receipts.count(DATABASE_CLOSED), 0, "{stage}: no close");
    for event in [
        WORKER_SPAWNED,
        WORKER_EXITED,
        WORKER_JOINED,
        TIMER_CREATED,
        CACHE_CREATED,
        SEND_ATTEMPT,
    ] {
        assert_eq!(receipts.count(event), 0, "{stage}: no {event}");
    }
    assert_eq!(active(&receipts), zero_active(), "{stage}: cleaned");

    let mut closed = peer.closed;
    tokio::time::timeout(Duration::from_secs(10), &mut closed)
        .await
        .expect("the peer must observe schema-failure EOF within ten seconds")
        .expect("the peer reports its own EOF");
    tokio::time::timeout(Duration::from_secs(10), peer.task)
        .await
        .expect("the schema-failure peer task must finish within ten seconds")
        .expect("the loopback peer task finishes");
    registry
        .stop_bound_channel_compat(&id)
        .await
        .expect("a failed schema activation joins its cleanup path");
    registry
        .unregister_bound_channel(&id)
        .await
        .expect("the failed source unregisters");
}

/// A failed stop is not repaired on the caller's behalf: the owning channel
/// retains its client handle and the incumbent entry survives, every retry
/// stays `Busy`, and only the owning registry's drop releases the handle.
#[tokio::test]
async fn failed_stop_preserves_real_residual_until_owner_drop() {
    let probe = ResourceProbe::default();
    let registry = Arc::new(ChannelRegistry::new());
    let tree = Arc::new(OwnershipTree::new());
    let id = register_bound(
        &registry,
        &tree,
        Box::new(FailingStopWebhook::new(
            "failing-stop-webhook",
            "http://127.0.0.1:1/callback".into(),
            probe.clone(),
        )),
        ChannelInitialState::Cold,
    )
    .await;

    registry
        .start_bound_channel_compat(&id)
        .await
        .expect("the real webhook activates behind the failing-stop fixture");
    assert_eq!(
        probe.count("client_build_attempt"),
        1,
        "start builds a client"
    );
    assert_eq!(probe.count("client_built"), 1, "start owns one real client");
    assert!(
        probe.count("client_built") > probe.count("client_drop"),
        "an active channel still owns the client it built"
    );

    let residual = match registry.stop_bound_channel_compat(&id).await {
        Err(ChannelBindingError::Runtime {
            phase,
            residual,
            error,
        }) => {
            assert_eq!(
                phase,
                ChannelRuntimePhase::Stop,
                "a failed stop reports its own phase, got {error}"
            );
            residual
        }
        other => panic!(
            "the injected stop failure must surface, got {}",
            unexpected(&other)
        ),
    };
    assert_eq!(
        residual.active_work, 0,
        "failed stop must not retain active work"
    );
    assert!(
        !residual.unjoined_forwarder,
        "failed stop must not retain an unjoined forwarder"
    );
    // This is a conservative residual diagnostic, while client_drop==0 below
    // is the measured proof that the owning channel still holds its client.
    assert!(
        residual.transport_may_be_live,
        "failed stop retains the live-transport diagnostic"
    );
    assert_eq!(
        probe.count("client_drop"),
        0,
        "a failed stop must not release the client handle"
    );

    assert!(
        matches!(
            registry.stop_bound_channel_compat(&id).await,
            Err(ChannelBindingError::Busy)
        ),
        "retry stays Busy under the current lifecycle contract"
    );
    assert!(
        matches!(
            registry.unregister_bound_channel(&id).await,
            Err(ChannelBindingError::Busy)
        ),
        "the retained incumbent cannot be unregistered"
    );
    assert_eq!(
        probe.count("client_drop"),
        0,
        "neither a Busy retry nor a Busy unregister cleans up"
    );

    // Only dropping the owning registry destroys the real client, and it records
    // the same receipt a successful stop would have recorded.
    drop(registry);
    assert_eq!(
        probe.count("client_drop"),
        1,
        "the owner drop must release the channel's client handle exactly once"
    );
    assert_eq!(
        probe.count("client_build_attempt"),
        1,
        "the owner drop must not build a second client"
    );
    assert_eq!(
        probe.count("client_built"),
        probe.count("client_drop"),
        "the owning channel must have no client handle after owner drop"
    );
    assert_eq!(
        probe.count("request_attempt"),
        0,
        "no request was ever made"
    );
    assert_eq!(
        probe.count("request_ended"),
        0,
        "no request was ever closed"
    );
}
