//! Stage D resource matrices: Task 1 establishes real positive controls for the
//! Generic `WebhookChannel` and the test-only `PollingSocketChannel`; Tasks 2–3
//! add the shared lifecycle and failure matrices.
//!
//! Every receipt asserted here is recorded at a real resource boundary by the
//! shape that owns it; no count is derived from lifecycle state.

use super::binding::{BoundCompletion, ChannelInitialState};
use super::test_support::{
    register_bound, LoopbackPeer, LoopbackSocket, PollingReceipts, PollingSocketChannel,
    ResourceProbe, CACHE_CREATED, CACHE_DROP, DATABASE_CLOSED, DATABASE_DROP, DATABASE_OPENED,
    DATABASE_OPEN_ATTEMPT, SEND_ATTEMPT, SEND_COMPLETED, TCP_CONNECTED, TCP_CONNECT_ATTEMPT,
    TCP_DROP, TIMER_CREATED, TIMER_DROP, TIMER_TICK, WORKER_EXITED, WORKER_JOINED, WORKER_SPAWNED,
};
use super::{ChannelId, ChannelRegistry, OwnershipTree, VisibilityScope};
use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
use crate::sync_primitives::Arc;
use std::time::Duration;

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
    let bytes = arrived.await.unwrap();
    assert!(
        String::from_utf8_lossy(&bytes).contains("control payload"),
        "the loopback peer must receive the real outbound request"
    );
    peer.release.send(()).unwrap();
    assert!(matches!(
        completion.await.unwrap(),
        super::binding::BoundCompletion::Accepted(Ok(_))
    ));
    assert!(probe.count("request_attempt") >= 1);
    assert!(probe.count("request_ended") >= 1);
    peer.task.await.unwrap();

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
        "no client may survive the observable stop"
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
    peer.task.await.unwrap();
}
