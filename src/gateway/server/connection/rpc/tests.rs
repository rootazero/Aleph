use std::task::Poll;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Notify;

use super::*;
use crate::gateway::handlers::{self, HandlerRegistry};
use crate::gateway::panel_project_context::{PanelCallerContext, PanelProjectSelector};
use crate::gateway::protocol::{INVALID_PARAMS, RATE_LIMITED};
use crate::gateway::rate_limiter::RateLimitConfig;
use crate::gateway::security::store::{DeviceUpsertData, SecurityStore, UserRole};
use crate::projects::ProjectStore;

fn peer(user: &str) -> DispatchPeer {
    let mut state = ConnectionState::new("127.0.0.1".parse().unwrap(), false);
    state.first_message = false;
    state.caller_role = "member".into();
    state.caller_user = Some(user.into());
    state.verified_user = Some(user.into());
    state.device_id = Some(format!("device-{user}"));
    DispatchPeer {
        connections: Arc::new(RwLock::new(HashMap::from([(user.to_owned(), state)]))),
        conn_id: user.to_owned(),
        client_is_local: false,
    }
}

fn executor(handlers: HandlerRegistry, limiter: Arc<RateLimiter>) -> RpcExecutor {
    RpcExecutor {
        middleware: MiddlewareChain::new(Arc::new(handlers), limiter),
        lanes: Arc::new(LaneManager::new(Default::default())),
        idempotency: Arc::new(IdempotencyGuard::new(Duration::from_secs(60))),
        class: ChannelClass::Desktop,
    }
}

async fn invoke(executor: &RpcExecutor, peer: &DispatchPeer, req: JsonRpcRequest) -> Value {
    let start = CallerSnapshot::read(&peer.connections, &peer.conn_id, peer.client_is_local).await;
    let plan = match prepare(&req, &start, true) {
        Ok(plan) => plan,
        Err(response) => return serde_json::to_value(response).unwrap(),
    };
    let text = serde_json::to_string(&req).unwrap();
    serde_json::from_str(&executor.execute(&text, &req, plan, peer, &start).await).unwrap()
}

#[tokio::test]
async fn ordinary_rpc_uses_one_inflight_slot() {
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let mut handlers = HandlerRegistry::new();
    handlers.register("chat.send", {
        let hits = hits.clone();
        let release = release.clone();
        let entered = entered.clone();
        move |req| {
            let hits = hits.clone();
            let release = release.clone();
            let entered = entered.clone();
            Box::pin(async move {
                let marker = hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if marker == 1 {
                    entered.notify_one();
                    release.notified().await;
                }
                JsonRpcResponse::success(req.id, json!({"marker": marker}))
            })
        }
    });
    let executor = executor(handlers, Arc::new(RateLimiter::new(Default::default())));
    let alice = peer("alice");
    let bob = peer("bob");
    let request = |id| {
        JsonRpcRequest::with_id(
            "chat.send",
            Some(json!({"idempotency_key":"attempt"})),
            json!(id),
        )
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        let producer = invoke(&executor, &alice, request(11));
        tokio::pin!(producer);
        assert!(matches!(futures::poll!(&mut producer), Poll::Pending));
        entered.notified().await;
        let duplicate = invoke(&executor, &alice, request(12));
        tokio::pin!(duplicate);
        assert!(matches!(futures::poll!(&mut duplicate), Poll::Pending));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        release.notify_one();
        let first = producer.await;
        let second = duplicate.await;
        let cached = invoke(&executor, &alice, request(13)).await;
        for (id, response) in [(11, first), (12, second), (13, cached)] {
            assert_eq!(response["id"], id);
            assert_eq!(response["result"]["marker"], 1);
        }
        let distinct = invoke(&executor, &bob, request(14)).await;
        assert_eq!(distinct["id"], 14);
        assert_eq!(distinct["result"]["marker"], 2);

        for (id, params, code) in [
            (15, None, crate::gateway::protocol::IDEMPOTENCY_KEY_REQUIRED),
            (
                16,
                Some(json!({"idempotency_key":"bad\u{1f}key"})),
                INVALID_PARAMS,
            ),
        ] {
            let denied = invoke(
                &executor,
                &alice,
                JsonRpcRequest::with_id("chat.send", params, json!(id)),
            )
            .await;
            assert_eq!(denied["id"], id);
            assert_eq!(denied["error"]["code"], code);
        }
        let health = invoke(
            &executor,
            &alice,
            JsonRpcRequest::with_id("health", None, json!(17)),
        )
        .await;
        assert_eq!(health["result"]["status"], "healthy");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    })
    .await
    .expect("producer and duplicate must settle");
}

#[tokio::test]
async fn origin_and_principal_windows_remain_distinct() {
    let mut config = RateLimitConfig::default();
    config.rpc_default.max_requests = 1;
    let limiter = Arc::new(RateLimiter::new(config));
    let executor = executor(HandlerRegistry::new(), limiter.clone());
    let alice = peer("alice");
    let bob = peer("bob");
    let request = |id| JsonRpcRequest::with_id("health", None, json!(id));
    // The forwarded address can look local without being a trusted loopback transport.
    let origin_a = Some("127.0.0.1".parse().unwrap());
    let origin_b = Some("192.0.2.2".parse().unwrap());
    let origin_c = Some("192.0.2.3".parse().unwrap());
    check_origin_rate(&limiter, origin_a, &request(1)).unwrap();
    assert_eq!(
        invoke(&executor, &alice, request(1)).await["result"]["status"],
        "healthy"
    );
    let denied =
        serde_json::to_value(check_origin_rate(&limiter, origin_a, &request(2)).unwrap_err())
            .unwrap();
    assert_eq!(denied["id"], 2);
    assert_eq!(denied["error"]["code"], RATE_LIMITED);
    assert!(denied["error"]["data"]["retry_after_ms"].as_u64().unwrap() > 0);
    check_origin_rate(&limiter, origin_b, &request(3)).unwrap();
    let denied = invoke(&executor, &alice, request(3)).await;
    assert_eq!(denied["id"], 3);
    assert_eq!(denied["error"]["code"], RATE_LIMITED);
    check_origin_rate(&limiter, origin_c, &request(4)).unwrap();
    assert_eq!(
        invoke(&executor, &bob, request(4)).await["result"]["status"],
        "healthy"
    );
}

#[tokio::test]
async fn project_select_executes_again_after_intervening_selection() {
    let root = tempfile::tempdir().unwrap();
    let projects = Arc::new(ProjectStore::new(
        rusqlite::Connection::open_in_memory().unwrap(),
    ));
    projects.create_schema().unwrap();
    let a = projects
        .create("A", Some("alice"), Some(root.path()))
        .unwrap();
    let b = projects
        .create("B", Some("alice"), Some(&root.path().join("B")))
        .unwrap();
    let security = Arc::new(SecurityStore::in_memory().unwrap());
    security
        .create_user("alice", "Alice", UserRole::Member)
        .unwrap();
    security
        .upsert_device(&DeviceUpsertData {
            device_id: "device-alice",
            device_name: "Panel",
            device_type: Some("panel"),
            public_key: &[1; 32],
            fingerprint: "fixture-alice",
            role: "member",
            scopes: &[],
            user_id: Some("alice"),
        })
        .unwrap();
    let selector = Arc::new(PanelProjectSelector::new(projects, security));
    let mut handlers = HandlerRegistry::new();
    let method = aleph_protocol::panel::PANEL_PROJECT_SELECT_METHOD;
    handlers.register(method, handlers::panel_capabilities::handle_select);
    let mut executor = executor(handlers, Arc::new(RateLimiter::new(Default::default())));
    executor.middleware = executor
        .middleware
        .with_panel_projects(Some(selector.clone()));
    let peer = peer("alice");
    let caller =
        PanelCallerContext::authenticated(Some("alice".into()), Some("device-alice".into()));
    for (id, target, key) in [
        (1, &a, "attempt"),
        (2, &b, "intervening"),
        (3, &a, "attempt"),
    ] {
        let request = JsonRpcRequest::with_id(
            method,
            Some(json!({"project_id":target.id,"idempotency_key":key})),
            json!(id),
        );
        let response = invoke(&executor, &peer, request).await;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["project_id"], target.id);
        assert_eq!(
            selector
                .resolve(&caller)
                .unwrap()
                .selection
                .project_id
                .as_deref(),
            Some(target.id.as_str())
        );
    }
}
