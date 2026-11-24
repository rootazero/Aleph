use super::{execute, executor, fixture_state, invoke, peer};
use crate::cluster::PendingInvokes;
use crate::gateway::event_bus::TopicSubscription;
use crate::gateway::handlers::{session::handle_list_db, HandlerRegistry};
use crate::gateway::lane::{ChannelClass, LaneConfig, LaneManager};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, PERMISSION_DENIED};
use crate::gateway::rate_limiter::RateLimiter;
use crate::gateway::router::SessionKey;
use crate::gateway::security::store::OWNER_USER_ID;
use crate::gateway::server::connection::{cleanup, ConnectionContext};
use crate::gateway::server::GatewayServer;
use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
use crate::gateway::session_store::SessionStore;
use crate::scope::{with_scope, ScopeAttribution};
use serde_json::{json, Value};
use std::net::IpAddr;
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::{Notify, RwLock};

use super::super::{check_origin_rate, DispatchPeer, RpcExecutor};

fn file_store(root: &Path) -> Arc<dyn SessionStore> {
    Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: root.join("sessions"),
            ..Default::default()
        })
        .unwrap(),
    )
}

pub(super) fn expect_denied(r: &Value, id: u32, code: i32) {
    assert_eq!(r["id"], id);
    assert_eq!(r["error"]["code"], code, "response: {r}");
    assert!(r.get("result").is_none(), "response: {r}");
}

pub(super) async fn bind(legacy: &DispatchPeer) -> DispatchPeer {
    let expected = legacy.snapshot().await.unwrap();
    DispatchPeer::bound(legacy.connections.clone(), legacy.conn_id.clone(), expected)
}

// This is a test entrance, not a second transport or a route registration.
pub(super) async fn invoke_bound(
    executor: &RpcExecutor,
    peer: &DispatchPeer,
    req: JsonRpcRequest,
    origin: Option<(&RateLimiter, IpAddr)>,
) -> Value {
    assert!(matches!(
        req.method.as_str(),
        "health"
            | "sessions.list"
            | "config.patch"
            | "chat.send"
            | aleph_protocol::panel::PANEL_PROJECT_SELECT_METHOD
    ));
    if let Some((limiter, ip)) = origin {
        if let Err(response) = check_origin_rate(limiter, Some(ip), &req) {
            return serde_json::to_value(response).unwrap();
        }
    }
    let Some(start) = peer.snapshot().await else {
        return serde_json::from_str(&super::super::dispatch::viewer_denied_response(req.id))
            .unwrap();
    };
    execute(executor, peer, req, &start).await
}

#[tokio::test]
async fn bound_and_legacy_reach_same_scoped_session_handler() {
    let root = tempfile::tempdir().unwrap();
    let sessions = file_store(root.path());
    let alice_key = SessionKey::main("alice-fixture");
    let bob_key = SessionKey::main("bob-fixture");
    for (user, key) in [("alice", &alice_key), ("bob", &bob_key)] {
        let metadata = with_scope(
            Some(ScopeAttribution::personal(user)),
            sessions.get_or_create(key),
        )
        .await
        .unwrap();
        assert_eq!(metadata.owner_user_id.as_deref(), Some(user));
    }
    let hits = Arc::new(AtomicUsize::new(0));
    let mut handlers = HandlerRegistry::new();
    handlers.register("sessions.list", move |request| {
        handle_list_db(request, sessions.clone())
    });
    handlers.register("config.patch", {
        let hits = hits.clone();
        move |request| {
            hits.fetch_add(1, Ordering::SeqCst);
            std::future::ready(JsonRpcResponse::success(request.id, json!("unexpected")))
        }
    });
    let limiter = Arc::new(RateLimiter::new(Default::default()));
    let executor = executor(handlers, limiter.clone());
    let legacy = peer("alice");
    let bound = bind(&legacy).await;
    let origin = Some((limiter.as_ref(), "192.0.2.42".parse().unwrap()));
    let list = |id| JsonRpcRequest::with_id("sessions.list", None, json!(id));
    let patch = |id| {
        JsonRpcRequest::with_id(
            "config.patch",
            Some(json!({"idempotency_key":"member-attempt"})),
            json!(id),
        )
    };
    for (id, response) in [
        (1, invoke(&executor, &legacy, list(1)).await),
        (3, invoke_bound(&executor, &bound, list(3), origin).await),
    ] {
        assert_eq!(response["id"], id);
        let rows = response["result"]["sessions"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["key"], alice_key.to_key_string());
    }
    for (id, response) in [
        (2, invoke(&executor, &legacy, patch(2)).await),
        (4, invoke_bound(&executor, &bound, patch(4), origin).await),
    ] {
        expect_denied(&response, id, AUTH_REQUIRED);
        assert_eq!(
            response["error"]["message"],
            crate::gateway::protocol::ADMIN_REQUIRED_MESSAGE
        );
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bound_work_rejects_missing_revoked_or_replaced_record() {
    let hits = Arc::new(AtomicUsize::new(0));
    let mut handlers = HandlerRegistry::new();
    for method in ["health", "config.patch"] {
        handlers.register(method, {
            let hits = hits.clone();
            move |request| {
                hits.fetch_add(1, Ordering::SeqCst);
                std::future::ready(JsonRpcResponse::success(
                    request.id,
                    json!({
                        "reached": true,
                        "loopback": crate::gateway::caller_identity::current_caller_is_loopback(),
                    }),
                ))
            }
        });
    }
    let mut executor = executor(handlers, Arc::new(RateLimiter::new(Default::default())));
    executor.lanes = Arc::new(LaneManager::new(LaneConfig {
        desktop_mutate_concurrency: 0,
        shared_mutate_concurrency: 1,
        ..Default::default()
    }));
    let legacy = peer(OWNER_USER_ID);
    let bound = bind(&legacy).await;
    let request = |id| JsonRpcRequest::with_id("health", None, json!(id));
    for id in 1..=6 {
        {
            let mut table = legacy.connections.write().await;
            let mut state = fixture_state(OWNER_USER_ID);
            match id {
                1 => state.first_message = true,
                2 => state.caller_role = "guest".into(),
                3 => state.verified_user = Some("replaced-user".into()),
                4 => state.device_id = None,
                5 => state.caller_user = Some("replaced-user".into()),
                _ => {}
            }
            if id == 6 {
                table.remove(&legacy.conn_id);
            } else {
                table.insert(legacy.conn_id.clone(), state);
            }
        }
        expect_denied(
            &invoke_bound(&executor, &bound, request(id), None).await,
            id,
            PERMISSION_DENIED,
        );
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    let admin = peer("alice");
    for (id, denial) in [(7, AUTH_REQUIRED), (8, PERMISSION_DENIED)] {
        admin
            .connections
            .write()
            .await
            .get_mut(&admin.conn_id)
            .unwrap()
            .caller_role = "operator".into();
        let admin_bound = bind(&admin).await;
        let permit = executor
            .lanes
            .acquire("config.patch", ChannelClass::Desktop)
            .await
            .unwrap();
        let patch = invoke_bound(
            &executor,
            &admin_bound,
            JsonRpcRequest::with_id(
                "config.patch",
                Some(json!({"idempotency_key":format!("held-lane-{id}")})),
                json!(id),
            ),
            None,
        );
        tokio::pin!(patch);
        assert!(matches!(futures::poll!(&mut patch), Poll::Pending));
        {
            let mut table = admin.connections.write().await;
            let live = table.get_mut(&admin.conn_id).unwrap();
            if denial == AUTH_REQUIRED {
                live.caller_role = "member".into();
            } else {
                live.device_id = Some("replaced-device".into());
            }
        }
        drop(permit);
        expect_denied(
            &tokio::time::timeout(Duration::from_secs(2), patch)
                .await
                .unwrap(),
            id,
            denial,
        );
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let valid = bind(&peer("bob")).await;
    assert_eq!(
        invoke_bound(&executor, &valid, request(9), None).await["result"],
        json!({"reached": true, "loopback": false})
    );
}

#[tokio::test]
async fn bound_idempotency_waiting_and_cached_results_revalidate_device() {
    let hits = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let mut handlers = HandlerRegistry::new();
    handlers.register("chat.send", {
        let hits = hits.clone();
        let release = release.clone();
        move |request| {
            let hits = hits.clone();
            let release = release.clone();
            async move {
                if hits.fetch_add(1, Ordering::SeqCst) == 0 {
                    release.notified().await;
                }
                JsonRpcResponse::success(request.id, json!("fixture-result"))
            }
        }
    });
    let executor = executor(handlers, Arc::new(RateLimiter::new(Default::default())));
    let connections = Arc::new(RwLock::new(
        ["producer", "waiter", "cache-reader"]
            .into_iter()
            .map(|id| (id.to_owned(), fixture_state("alice")))
            .collect(),
    ));
    let legacy = |id: &str| DispatchPeer::legacy(connections.clone(), id.to_owned(), false);
    let producer = bind(&legacy("producer")).await;
    let waiter = bind(&legacy("waiter")).await;
    let reader = bind(&legacy("cache-reader")).await;
    let request = |id| {
        JsonRpcRequest::with_id(
            "chat.send",
            Some(json!({"idempotency_key":"same"})),
            json!(id),
        )
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        let first = invoke_bound(&executor, &producer, request(1), None);
        tokio::pin!(first);
        assert!(matches!(futures::poll!(&mut first), Poll::Pending));
        let joined = invoke_bound(&executor, &waiter, request(2), None);
        tokio::pin!(joined);
        assert!(matches!(futures::poll!(&mut joined), Poll::Pending));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        connections
            .write()
            .await
            .get_mut("waiter")
            .unwrap()
            .device_id = Some("other-device".into());
        release.notify_one();
        assert_eq!(first.await["result"], "fixture-result");
        let joined = joined.await;

        let start = reader.snapshot().await.unwrap();
        connections
            .write()
            .await
            .get_mut("cache-reader")
            .unwrap()
            .device_id = Some("other-device".into());
        let from_cache = execute(&executor, &reader, request(3), &start).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        expect_denied(&joined, 2, PERMISSION_DENIED);
        expect_denied(&from_cache, 3, PERMISSION_DENIED);
        assert_eq!(
            invoke_bound(&executor, &producer, request(4), None).await["result"],
            "fixture-result"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn bound_connection_cleanup_is_local() {
    let root = tempfile::tempdir().unwrap();
    let sessions = file_store(root.path());
    let key = SessionKey::main("retained");
    sessions.get_or_create(&key).await.unwrap();
    let owner = peer(OWNER_USER_ID);
    let bound = bind(&owner).await;
    owner
        .connections
        .write()
        .await
        .insert("bob".into(), fixture_state("bob"));
    let gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    let executor = executor(
        HandlerRegistry::new(),
        Arc::new(RateLimiter::new(Default::default())),
    );
    let subscriptions = gateway.subscription_manager.clone();
    let ctx = ConnectionContext {
        middleware_chain: executor.middleware.clone(),
        event_bus: gateway.event_bus.clone(),
        connections: owner.connections.clone(),
        subscription_manager: subscriptions.clone(),
        presence: gateway.presence.clone(),
        state_versions: gateway.state_versions.clone(),
        rate_limiter: gateway.rate_limiter.clone(),
        lane_manager: gateway.lane_manager.clone(),
        idempotency_guard: gateway.idempotency_guard.clone(),
        event_scope_guard: gateway.event_scope_guard.clone(),
        channel_class: ChannelClass::Desktop,
        ping_interval_secs: 15,
        idle_timeout_secs: 60,
        require_idempotency_key: true,
        security_store: None,
        device_token_mgr: None,
        client_ip: "127.0.0.1".parse().unwrap(),
        client_is_local: true,
        node_registry: gateway.node_registry.clone(),
        exec_approval_manager: None,
        audit_log: None,
        session_store: Some(sessions.clone()),
        team_store: None,
        event_visibility: gateway.event_visibility.clone(),
    };
    let mine = Arc::new(PendingInvokes::new());
    let theirs = PendingInvokes::new();
    let (_, mine_result) = mine.register();
    let (other_id, other_result) = theirs.register();
    let topic = TopicSubscription::pattern_only("session.*");
    for conn_id in [&owner.conn_id[..], "bob"] {
        subscriptions
            .add_subscriptions(conn_id, vec![topic.clone()])
            .await;
    }
    cleanup::run_cleanup(&owner.conn_id, &ctx, &mine).await;
    assert!(!owner.connections.read().await.contains_key(&owner.conn_id));
    assert!(owner.connections.read().await.contains_key("bob"));
    assert!(subscriptions
        .get_subscriptions(&owner.conn_id)
        .await
        .is_empty());
    assert_eq!(subscriptions.get_subscriptions("bob").await, vec![topic]);
    assert!(tokio::time::timeout(Duration::from_secs(1), mine_result)
        .await
        .unwrap()
        .is_err());
    let id = json!(other_id);
    theirs.resolve(
        &id,
        JsonRpcResponse::success(Some(id.clone()), json!("kept")),
    );
    let delivered = tokio::time::timeout(Duration::from_secs(1), other_result)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivered.result, Some(json!("kept")));
    assert!(sessions.get_metadata(&key).await.unwrap().is_some());
    let denied = invoke_bound(
        &executor,
        &bound,
        JsonRpcRequest::with_id("health", None, json!(1)),
        None,
    )
    .await;
    expect_denied(&denied, 1, PERMISSION_DENIED);
}
