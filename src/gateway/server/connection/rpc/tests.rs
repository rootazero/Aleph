use std::collections::HashMap;
use std::task::Poll;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{Notify, RwLock};

use super::*;
use crate::gateway::handlers::{self, HandlerRegistry};
use crate::gateway::panel_project_context::{PanelCallerContext, PanelProjectSelector};
use crate::gateway::protocol::{INVALID_PARAMS, RATE_LIMITED};
use crate::gateway::rate_limiter::RateLimitConfig;
use crate::gateway::security::store::{DeviceUpsertData, SecurityStore, UserRole};
use crate::gateway::server::ConnectionState;
use crate::projects::ProjectStore;

mod bound_tests;

fn fixture_state(user: &str) -> ConnectionState {
    let mut state = ConnectionState::new("127.0.0.1".parse().unwrap(), false);
    state.first_message = false;
    state.caller_role = "member".into();
    state.caller_user = Some(user.into());
    state.verified_user = Some(user.into());
    state.device_id = Some(format!("device-{user}"));
    state
}

fn peer(user: &str) -> DispatchPeer {
    DispatchPeer::legacy(
        Arc::new(RwLock::new(HashMap::from([(
            user.to_owned(),
            fixture_state(user),
        )]))),
        user.to_owned(),
        false,
    )
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
    let start = peer.snapshot().await.unwrap();
    execute(executor, peer, req, &start).await
}

async fn execute(
    executor: &RpcExecutor,
    peer: &DispatchPeer,
    req: JsonRpcRequest,
    start: &CallerSnapshot,
) -> Value {
    let plan = match prepare(&req, start, true) {
        Ok(plan) => plan,
        Err(response) => return serde_json::to_value(response).unwrap(),
    };
    let text = serde_json::to_string(&req).unwrap();
    serde_json::from_str(&executor.execute(&text, &req, plan, peer, start).await).unwrap()
}

#[tokio::test]
async fn shell_origin_cannot_be_replayed_or_rebound() {
    assert_shell_origin(false).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires a delegated Linux supervisor subgroup and ALEPH_TEST_FIXED_JSONL_SERVER"]
async fn paired_shell_approval_reaches_fixed_consumer() {
    assert!(std::env::var_os("ALEPH_TEST_FIXED_JSONL_SERVER").is_some());
    assert_shell_origin(true).await;
}

async fn assert_shell_origin(_native: bool) {
    use crate::gateway::security::DeviceTokenManager;
    use crate::gateway::server::connection::dispatch::current_paired_shell_origin;

    let store = Arc::new(SecurityStore::in_memory().unwrap());
    store
        .create_user("alice", "Alice", UserRole::Member)
        .unwrap();
    let manager = DeviceTokenManager::new(store.clone());
    let ticket = manager
        .create_bootstrap_ticket(None, Some("alice"))
        .unwrap();
    let paired = manager
        .exchange_bootstrap_ticket(&ticket, Some("device-alice".into()), None, None)
        .unwrap();
    let credential = manager
        .validate_device_token(&paired.device_token)
        .unwrap()
        .unwrap();
    let mut state = fixture_state("alice");
    state.verified_device_token_id = Some(credential.token_id.clone());
    let connections = Arc::new(RwLock::new(HashMap::from([("first".into(), state)])));
    let panel = DispatchPeer::panel(
        connections.clone(),
        "first".into(),
        false,
        Some(store.clone()),
    );
    let mut handlers = HandlerRegistry::new();
    let saved_origin = Arc::new(std::sync::Mutex::new(None));
    for method in ["chat.send", "agent.run"] {
        let saved_origin = saved_origin.clone();
        #[cfg(target_os = "linux")]
        let account_store = store.clone();
        handlers.register(method, move |req| {
            let saved_origin = saved_origin.clone();
            #[cfg(target_os = "linux")]
            let account_store = account_store.clone();
            async move {
                let captured = current_paired_shell_origin();
                let bound = match captured {
                    Some(binding) => {
                        *saved_origin.lock().unwrap() = Some(binding.clone());
                        #[cfg(target_os = "linux")]
                        if req.id.as_ref().and_then(serde_json::Value::as_str) == Some("attached") {
                            crate::gateway::execution_engine::assert_paired_shell_effect_order(
                                binding.clone(),
                                &account_store,
                                _native,
                            )
                            .await;
                        }
                        tokio::spawn(async move { binding.still_connected().await })
                            .await
                            .unwrap()
                    }
                    None => false,
                };
                assert!(
                    !tokio::spawn(async { current_paired_shell_origin().is_some() })
                        .await
                        .unwrap()
                );
                JsonRpcResponse::success(req.id, json!({"bound": bound}))
            }
        });
    }
    let executor = executor(handlers, Arc::new(RateLimiter::new(Default::default())));
    let request = |method, key| {
        JsonRpcRequest::with_id(
            method,
            Some(json!({"idempotency_key": key, "verified_device_token_id": credential.token_id})),
            json!(key),
        )
    };
    let attached = invoke(&executor, &panel, request("chat.send", "attached")).await;
    assert_eq!(attached["result"]["bound"], true, "{attached}");
    let fallback = invoke(&executor, &panel, request("agent.run", "fallback")).await;
    assert_eq!(fallback["result"]["bound"], false, "{fallback}");

    #[cfg(target_os = "linux")]
    {
        let origin = saved_origin.lock().unwrap().clone().unwrap();
        let guard = origin.try_effect_guard().unwrap();
        assert_eq!(guard.role_at_effect(), Some(UserRole::Member));
        assert!(connections.try_write().is_err());
    }

    let mut table = connections.write().await;
    let state = table.remove("first").unwrap();
    table.insert("second".into(), state);
    drop(table);
    let old_origin = saved_origin.lock().unwrap().take().unwrap();
    assert!(!old_origin.still_connected().await);
    #[cfg(target_os = "linux")]
    assert!(matches!(
        old_origin.try_effect_guard(),
        Err(crate::sandbox::shell_input::ShellEffectDenied::Stale)
    ));
    let untrusted = DispatchPeer::legacy(connections.clone(), "second".into(), true);
    let legacy = invoke(&executor, &untrusted, request("chat.send", "legacy")).await;
    assert_eq!(legacy["result"]["bound"], false, "{legacy}");

    assert!(store.revoke_device_token(&credential.token_id).unwrap());
    assert!(store.is_device_approved(&paired.device_id).unwrap());
    let second = DispatchPeer::panel(connections, "second".into(), false, Some(store));
    let revoked = invoke(&executor, &second, request("chat.send", "revoked")).await;
    assert_eq!(revoked["result"]["bound"], false, "{revoked}");
}

#[tokio::test]
async fn chat_models_list_open_to_members_without_admin_exposure() {
    use crate::config::{Config, ProviderConfig};
    use crate::gateway::protocol::{AUTH_REQUIRED, SERVICE_UNAVAILABLE};
    use crate::providers::session_model_handle::{
        with_test_selectable_models, SelectableModelsSnapshot,
    };
    use std::collections::BTreeSet;

    let method = "chat.models.list";
    assert!(super::super::v1_mounted(method));
    assert_eq!(Lane::for_method(method), Lane::Query);
    let executor = executor(
        HandlerRegistry::new(),
        Arc::new(RateLimiter::new(RateLimitConfig::default())),
    );
    let peer = peer("model-picker-member");
    let request = || JsonRpcRequest::with_id(method, None, json!(1));
    let mut config = Config::default();
    config.providers.clear();
    for (provider, enabled) in [
        ("relay-a", true),
        ("relay-b", true),
        ("hidden", false),
        ("unauthed", true),
    ] {
        let mut entry = ProviderConfig::test_config("org/model");
        entry.enabled = enabled;
        config.providers.insert(provider.into(), entry);
    }
    let ready: BTreeSet<_> = config.providers.keys().cloned().collect();
    let snapshot = SelectableModelsSnapshot::from_config(&config, ready.clone(), &ready, |name| {
        name != "unauthed"
    });
    with_test_selectable_models(Some(snapshot), async {
        let response = invoke(&executor, &peer, request()).await;
        let typed: aleph_protocol::providers::ChatModelsListResult =
            serde_json::from_value(response["result"].clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), response["result"]);
        assert_eq!(response["result"], json!({"items": [
            {"provider": "relay-a", "model": "org/model", "provider_label": "relay-a", "model_label": "org/model"},
            {"provider": "relay-b", "model": "org/model", "provider_label": "relay-b", "model_label": "org/model"}
        ]}));
        for admin in ["providers.catalog", "providers.modelsRefresh"] {
            let params = (admin == "providers.modelsRefresh").then(|| json!({"idempotency_key": uuid::Uuid::new_v4()}));
            let response = invoke(&executor, &peer, JsonRpcRequest::with_id(admin, params, json!(2))).await;
            assert_eq!(response["error"]["code"], AUTH_REQUIRED, "{admin}: {response}");
        }
    }).await;
    with_test_selectable_models(None, async {
        let response = invoke(&executor, &peer, request()).await;
        assert_eq!(response["error"]["code"], SERVICE_UNAVAILABLE);
    })
    .await;
    with_test_selectable_models(Some(SelectableModelsSnapshot::default()), async {
        let response = invoke(&executor, &peer, request()).await;
        assert_eq!(response["result"], json!({"items": []}));
    })
    .await;
}

#[tokio::test]
async fn mounted_graph_reads_prepare_without_an_idempotency_key() {
    let peer = peer("graph-reader");
    let caller = peer.snapshot().await.unwrap();
    for method in ["graph.query", "graph.search", "graph.node_detail"] {
        assert!(super::super::v1_mounted(method), "{method}");
        assert_eq!(Lane::for_method(method), Lane::Query, "{method}");
        let request = JsonRpcRequest::with_id(method, None, json!(1));
        assert!(prepare(&request, &caller, true).is_ok(), "{method}");
    }
    for method in [
        "graph.update_note",
        "graph.rename_note",
        "graph.delete_note",
    ] {
        assert!(!super::super::v1_mounted(method), "{method}");
    }
}

#[tokio::test]
async fn v1_mounts_questionnaire_reads_and_guarded_writes() {
    let caller = peer("questionnaire-member").snapshot().await.unwrap();
    let pending = "clarification.pending";
    assert!(super::super::v1_mounted(pending));
    assert_eq!(Lane::for_method(pending), Lane::Query);
    assert!(prepare(
        &JsonRpcRequest::with_id(pending, None, json!(1)),
        &caller,
        true
    )
    .is_ok());

    let resolve = "clarification.resolve";
    assert!(super::super::v1_mounted(resolve));
    assert!(Lane::for_method(resolve).needs_idempotency());
    assert!(prepare(
        &JsonRpcRequest::with_id(resolve, None, json!(2)),
        &caller,
        true
    )
    .is_err());
    let params = json!({"idempotency_key": uuid::Uuid::new_v4()});
    assert!(prepare(
        &JsonRpcRequest::with_id(resolve, Some(params), json!(3)),
        &caller,
        true
    )
    .is_ok());
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
    let bound = bound_tests::bind(&peer).await;
    for (id, target, key, connection) in [
        (1, &a, "attempt", &peer),
        (2, &b, "intervening", &bound),
        (3, &a, "attempt", &peer),
    ] {
        let request = JsonRpcRequest::with_id(
            method,
            Some(json!({"project_id":target.id,"idempotency_key":key})),
            json!(id),
        );
        let response = if id == 2 {
            bound_tests::invoke_bound(&executor, connection, request, None).await
        } else {
            invoke(&executor, connection, request).await
        };
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
    peer.connections
        .write()
        .await
        .get_mut(&peer.conn_id)
        .unwrap()
        .device_id = None;
    let response = bound_tests::invoke_bound(
        &executor,
        &bound,
        JsonRpcRequest::with_id(
            method,
            Some(json!({"project_id": b.id,"idempotency_key":"stale"})),
            json!(4),
        ),
        None,
    )
    .await;
    bound_tests::expect_denied(&response, 4, crate::gateway::protocol::PERMISSION_DENIED);
    assert_eq!(
        selector
            .resolve(&caller)
            .unwrap()
            .selection
            .project_id
            .as_deref(),
        Some(a.id.as_str())
    );
}
