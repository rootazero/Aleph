//! Bounded loopback integration evidence, using the registered gateway and real credentials.
//! One executable/runtime owns the install-once projection host and all fixture resources.
use std::{net::SocketAddr, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use alephcore::{
    capability::{
        ownership::OwnershipTree,
        policy::PolicyEngine,
        projection_host::{set_projection_host, ProjectionHost},
        special::CapabilityHandlerRegistry,
    },
    gateway::{
        security::{DeviceTokenManager, SecurityStore},
        session_store::{file_backend::{FileSessionStore, FileSessionStoreConfig}, SessionStore},
        server::GatewayConfig, GatewayServer,
    },
    memory::runtime::{MemoryRuntime, MemoryRuntimeConfig},
    routing::SessionKey,
    tools::registry::ToolHandlerRegistry,
};
use futures_util::{FutureExt, SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::{net::TcpStream, sync::oneshot, task::JoinHandle};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Peer {
    socket: Socket,
    next_id: u64,
    saved: Vec<Value>,
}

impl Peer {
    async fn connect(addr: SocketAddr, credential: &str, device: &str) -> Self {
        let (socket, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        let mut peer = Self { socket, next_id: 0, saved: Vec::new() };
        let response = peer.rpc("connect", json!({"device_token":credential,"channel_kind":"desktop"})).await;
        assert_eq!(response["result"]["authorized"], true, "credential must authorize");
        // These markers are absent for the credential-less loopback fallback.
        let rendered = response["result"].to_string();
        assert!(rendered.contains(device), "validated device identity must be echoed: {rendered}");
        assert!(rendered.contains("owner"), "credential must bind the fixture owner");
        peer.rpc("events.subscribe", json!({"topics":["capabilities.ui.display"]})).await;
        peer
    }

    async fn send(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.socket.send(Message::Text(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string().into())).await.unwrap();
        id
    }

    async fn frame(&mut self) -> Value {
        loop {
            match self.socket.next().await.expect("socket remains open").expect("valid websocket frame") {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Ping(data) => self.socket.send(Message::Pong(data)).await.unwrap(),
                other => panic!("unexpected frame: {other:?}"),
            }
        }
    }

    async fn reply(&mut self, id: u64) -> Value {
        if let Some(index) = self.saved.iter().position(|v| v["id"] == id) {
            return self.saved.remove(index);
        }
        loop {
            let value = self.frame().await;
            if value["id"] == id { return value; }
            self.saved.push(value);
        }
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params).await;
        tokio::time::timeout(Duration::from_secs(2), self.reply(id)).await.expect("bounded ordinary RPC")
    }

    async fn display(&mut self) -> Value {
        let matches = |v: &Value| v.to_string().contains("capabilities.ui.display") && v.get("id").is_none();
        if let Some(index) = self.saved.iter().position(matches) { return self.saved.remove(index); }
        loop {
            let value = self.frame().await;
            if matches(&value) { return value; }
            self.saved.push(value);
        }
    }
}

struct Fixture {
    _scratch: tempfile::TempDir,
    host: Arc<ProjectionHost>,
    stop: Option<oneshot::Sender<()>>,
    server: JoinHandle<()>,
    a: Peer,
    b: Peer,
    session: String,
    a_credential: String,
    bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    pty: Option<String>,
}

impl Fixture {
    async fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        // Independent executable, serial test, before any install-once path consumer.
        std::env::set_var("ALEPH_HOME", scratch.path());
        let tree = Arc::new(OwnershipTree::new());
        let memory = MemoryRuntime::new(MemoryRuntimeConfig {
            metadata_path: scratch.path().join("memory.sqlite"),
            note_memory_dir: scratch.path().join("notes"),
        }, tree.clone());
        let policy = Arc::new(PolicyEngine::with_shared_tree(tree.clone(), Default::default()).unwrap());
        let host = ProjectionHost::mount_with_runtime(ToolHandlerRegistry::new(), tree, memory,
            CapabilityHandlerRegistry::new(), policy, false);
        assert!(host.wait_until_ready().await);
        set_projection_host(host.clone());
        let security = Arc::new(SecurityStore::in_memory().unwrap());
        let tokens = Arc::new(DeviceTokenManager::new(security.clone()));
        let make_token = |id: &str| {
            let ticket = tokens.create_bootstrap_ticket(None, None).unwrap();
            tokens.exchange_bootstrap_ticket(&ticket, Some(id.to_owned()), None, None).unwrap().device_token
        };
        let a_token = make_token("fix1-device-a");
        let b_token = make_token("fix1-device-b");
        let sessions = Arc::new(FileSessionStore::new(FileSessionStoreConfig {
            base_dir: scratch.path().join("sessions"), ..Default::default()
        }).unwrap());
        let key = SessionKey::main("main");
        sessions.get_or_create(&key).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut gateway = GatewayServer::with_config(addr, GatewayConfig {
            // Desktop falls back to shared: set BOTH pools so the real total is
            // one permit, with waiters on the same pool that the holder releases.
            lane: alephcore::gateway::lane::LaneConfig { desktop_execute_concurrency: 0, shared_execute_concurrency: 1, ..Default::default() },
            ..Default::default()
        });
        gateway.set_security_store(security);
        gateway.set_device_token_manager(tokens);
        gateway.set_session_store(sessions);
        // Production boot registers this baseline handler; the real connection
        // orchestrator, not this handler, validates credentials and stamps identity.
        let connect_context = Arc::new(alephcore::gateway::handlers::connect::ConnectContext {
            state_versions: Arc::new(alephcore::gateway::state_version::StateVersionTracker::new()),
            transport_policy: alephcore::gateway::handlers::auth::TransportPolicy::defaults(),
        });
        gateway.handlers_mut().register("connect", move |request| {
            let context = connect_context.clone();
            Box::pin(alephcore::gateway::handlers::connect::handle_connect(request, context))
        });
        let bus = gateway.event_bus().clone();
        let router = gateway.build_router();
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async { let _ = stopped.await; }).await.unwrap();
        });
        let a = Peer::connect(addr, &a_token, "fix1-device-a").await;
        let b = Peer::connect(addr, &b_token, "fix1-device-b").await;
        Self { _scratch: scratch, host, stop: Some(stop), server, a, b, session: key.to_key_string(), a_credential: a_token, bus, pty: None }
    }

    async fn close(mut self) {
        let _ = self.a.socket.close(None).await;
        let _ = self.b.socket.close(None).await;
        let _ = self.stop.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(8), &mut self.server).await.expect("fixture server must join").unwrap();
        if let Some(manager) = alephcore::gateway::pty::try_manager().filter(|manager| manager.lifecycle_snapshot().driver_active) {
            tokio::time::timeout(Duration::from_secs(8), manager.deactivate()).await.expect("owned PTY driver must join").expect("owned PTYs must close");
        }
        self.host.close_and_await().await;
    }
}

fn operation(attachment: &Value, text: &str) -> Value {
    json!({"surface_id":attachment["surface_id"],"mount_generation":attachment["mount_generation"],
        "operation":{"operation":"display","text":text}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn display_event_and_ack_progress_on_same_authenticated_connection() {
    let mut f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        alephcore::gateway::pty::attach_event_bus(f.bus.clone()).unwrap();
        f.pty = Some(alephcore::gateway::pty::manager().spawn(&alephcore::gateway::pty::SpawnOptions {
            command: Some(if cfg!(windows) { "cmd.exe" } else { "/bin/cat" }.into()),
            cwd: Some(f._scratch.path().to_string_lossy().into_owned()),
            rows: 24, cols: 80, created_by: Some("owner".into()), ..Default::default()
        }).expect("test-owned real PTY").session_id);
        pty_survives(&f, "FIX1_BEFORE_UI").await;
        let response = f.a.rpc("capabilities.ui.attach", json!({"session_key":f.session})).await;
        assert!(response.get("error").is_none(), "registered attachment must succeed: {response}");
        let attachment = response["result"].clone();
        assert!(attachment["surface_id"].as_str().is_some());
        let invoke = f.a.send("capabilities.ui.invoke", operation(&attachment, "before acknowledgement")).await;
        // Causal RED: old inline invoke cannot deliver this until its 5000ms timeout.
        let frame = tokio::time::timeout(Duration::from_millis(1500), f.a.display()).await
            .expect("DISPLAY must reach the same authenticated socket BEFORE the 5000ms ack timeout");
        let text = frame.to_string();
        assert!(text.contains("before acknowledgement"));
        let data = frame.get("params").and_then(|p| p.get("data")).or_else(|| frame.get("data")).expect("display payload");
        let ack = json!({"surface_id":data["surface_id"],"mount_generation":data["mount_generation"],
            "request_id":data["request_id"],"rendered":true});
        let foreign = f.b.rpc("capabilities.ui.ack", ack.clone()).await;
        assert!(foreign.get("error").is_some(), "foreign connection must not acknowledge");
        assert!(tokio::time::timeout(Duration::from_millis(100), f.b.display()).await.is_err(), "display content must not reach the independently authenticated second socket");
        let mut stale = ack.clone(); stale["mount_generation"] = json!(data["mount_generation"].as_u64().unwrap() + 1);
        assert!(f.a.rpc("capabilities.ui.ack", stale).await.get("error").is_some());
        let mut wrong_request = ack.clone(); wrong_request["request_id"] = json!("not-the-rendered-request");
        assert!(f.a.rpc("capabilities.ui.ack", wrong_request).await.get("error").is_some());
        let mut not_rendered = ack.clone(); not_rendered["rendered"] = json!(false);
        assert!(f.a.rpc("capabilities.ui.ack", not_rendered).await.get("error").is_some());
        let acknowledged = f.a.rpc("capabilities.ui.ack", ack).await;
        assert!(acknowledged.get("error").is_none(), "same socket ack progresses: {acknowledged}");
        let completed = tokio::time::timeout(Duration::from_secs(1), f.a.reply(invoke)).await.unwrap();
        assert!(completed.get("error").is_none(), "invoke completes only after ack: {completed}");
        let detached = f.a.rpc("capabilities.ui.detach", attachment).await;
        assert!(detached.get("error").is_none());
        pty_survives(&f, "FIX1_AFTER_DETACH").await;
        idempotent_display_waits_and_caches_null(&mut f).await;
        queued_ui_effect_rechecks_identity_and_is_bounded(&mut f).await;
        missing_renderer_timeout_disconnect_and_pty_survival(&mut f).await;
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

fn render_ack(frame: &Value) -> Value {
    let data = frame.get("params").and_then(|p| p.get("data")).or_else(|| frame.get("data")).expect("display data");
    json!({"surface_id":data["surface_id"],"mount_generation":data["mount_generation"],"request_id":data["request_id"],"rendered":true})
}

async fn attach(peer: &mut Peer, session: &str) -> Value {
    let response = peer.rpc("capabilities.ui.attach", json!({"session_key":session})).await;
    assert!(response.get("error").is_none(), "real attach: {response}");
    response["result"].clone()
}

async fn pty_survives(f: &Fixture, marker: &str) {
    let id = f.pty.as_ref().unwrap();
    let manager = alephcore::gateway::pty::manager();
    manager.write(id, format!("{marker}\n").as_bytes()).expect("UI teardown must not close the real PTY input");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let screen = manager.attach_snapshot(id).expect("same test-owned PTY still exists");
            if screen.patch.rows.iter().any(|row| row.runs.iter().any(|run| run.text.contains(marker))) { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("PTY child output must still reach the server-held screen");
}

async fn idempotent_display_waits_and_caches_null(f: &mut Fixture) {
    let surface = attach(&mut f.a, &f.session).await;
    let mut params = operation(&surface, "one idempotent display");
    params["idempotency_key"] = json!("fix1-display");
    let first = f.a.send("capabilities.ui.invoke", params.clone()).await;
    let frame = tokio::time::timeout(Duration::from_secs(1), f.a.display()).await.unwrap();
    let duplicate = f.a.send("capabilities.ui.invoke", params.clone()).await;
    assert!(f.a.rpc("capabilities.ui.ack", render_ack(&frame)).await.get("error").is_none());
    let first = tokio::time::timeout(Duration::from_secs(1), f.a.reply(first)).await.unwrap();
    let duplicate = tokio::time::timeout(Duration::from_secs(1), f.a.reply(duplicate)).await.unwrap();
    assert!(first.get("error").is_none() && duplicate.get("error").is_none(), "duplicate waiting must not starve ack or re-execute: {first} {duplicate}");
    assert_eq!(first.get("result"), Some(&Value::Null));
    assert_eq!(first.get("result"), duplicate.get("result"));
    let cached = f.a.rpc("capabilities.ui.invoke", params).await;
    assert!(cached.get("error").is_none(), "a cached JSON-null success must not re-execute display: {cached}");
    assert_eq!(cached.get("result"), Some(&Value::Null));
    assert!(f.a.rpc("capabilities.ui.detach", surface).await.get("error").is_none());
}

async fn queued_ui_effect_rechecks_identity_and_is_bounded(f: &mut Fixture) {
    let a = attach(&mut f.a, &f.session).await;
    let b = attach(&mut f.b, &f.session).await;
    let holding = f.a.send("capabilities.ui.invoke", operation(&a, "hold actual Execute permit")).await;
    let frame = tokio::time::timeout(Duration::from_secs(1), f.a.display()).await.unwrap();
    let mut queued = Vec::new();
    for _ in 0..36 { queued.push(f.b.send("capabilities.ui.invoke", operation(&b, "must not render after reauth")).await); }
    let rejected = f.b.send("capabilities.ui.invoke", operation(&b, "37th must be Busy")).await;
    let busy = tokio::time::timeout(Duration::from_secs(1), f.b.reply(rejected)).await.unwrap();
    assert_eq!(busy["id"], rejected, "queue error must preserve the original request id");
    assert!(busy["error"]["message"].as_str().is_some_and(|text| text.contains("Busy")), "bounded 4 running plus 32 queued: {busy}");
    let reauth = f.b.rpc("connect", json!({"device_token":f.a_credential,"channel_kind":"desktop"})).await;
    assert_eq!(reauth["result"]["authorized"], true, "real credential reauthentication");
    assert!(reauth["result"].to_string().contains("fix1-device-a"));
    assert!(f.a.rpc("capabilities.ui.ack", render_ack(&frame)).await.get("error").is_none(), "Mutate ack must progress while Execute is saturated");
    assert!(tokio::time::timeout(Duration::from_secs(1), f.a.reply(holding)).await.unwrap().get("error").is_none());
    for id in queued {
        let response = tokio::time::timeout(Duration::from_secs(2), f.b.reply(id)).await.unwrap();
        assert!(response.get("error").is_some(), "queued request cannot execute under the changed validated identity: {response}");
    }
    assert!(!f.b.saved.iter().any(|value| value.to_string().contains("must not render after reauth")), "no queued effect reaches renderer after identity change");
    assert!(f.a.rpc("capabilities.ui.detach", a).await.get("error").is_none());
    assert!(f.b.rpc("capabilities.ui.detach", b).await.get("error").is_none());
}

async fn missing_renderer_timeout_disconnect_and_pty_survival(f: &mut Fixture) {
    let surface = attach(&mut f.a, &f.session).await;
    let request = f.a.send("capabilities.ui.invoke", operation(&surface, "renderer intentionally does not ack")).await;
    let frame = tokio::time::timeout(Duration::from_secs(1), f.a.display()).await.unwrap();
    let timeout = tokio::time::timeout(Duration::from_secs(7), f.a.reply(request)).await.unwrap();
    assert!(timeout.get("error").is_some(), "renderer loss must never become detached success: {timeout}");
    assert!(f.a.rpc("capabilities.ui.ack", render_ack(&frame)).await.get("error").is_some(), "late ack after timeout is stale");
    pty_survives(f, "FIX1_AFTER_RENDER_TIMEOUT").await;
    let request = f.a.send("capabilities.ui.invoke", operation(&surface, "detach while waiting")).await;
    let frame = tokio::time::timeout(Duration::from_secs(1), f.a.display()).await.unwrap();
    assert!(f.a.rpc("capabilities.ui.detach", surface).await.get("error").is_none());
    assert!(tokio::time::timeout(Duration::from_secs(1), f.a.reply(request)).await.unwrap().get("error").is_some());
    assert!(f.a.rpc("capabilities.ui.ack", render_ack(&frame)).await.get("error").is_some());
    pty_survives(f, "FIX1_AFTER_PENDING_DETACH").await;

    let surface = attach(&mut f.b, &f.session).await;
    let id = alephcore::capability::descriptor::CapabilityId { namespace: "aleph/ui".into(), name: surface["surface_id"].as_str().unwrap().into() };
    let retained = f.host.specials().ui_surface(&id).expect("same registered Arc surface");
    f.b.send("capabilities.ui.invoke", operation(&surface, "disconnect while waiting")).await;
    tokio::time::timeout(Duration::from_secs(1), f.b.display()).await.unwrap();
    // One active display owns the only Execute permit; additional jobs include
    // real permit waiters and FIFO-queued idempotency ownership at disconnect.
    for index in 0..8 {
        let mut params = operation(&surface, "cancel permit and queue waits on close");
        if index == 7 { params["idempotency_key"] = json!("disconnect-queued-slot"); }
        f.b.send("capabilities.ui.invoke", params).await;
    }
    assert!(f.b.rpc("health", json!({})).await.get("error").is_none(), "Query barrier proves queued frames were ingressed while Execute is held");
    f.b.socket.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.host.specials().ui_surface(&id).is_some() { tokio::task::yield_now().await; }
    }).await.expect("actual disconnect must close/join and withdraw its registry surface");
    assert!(matches!(retained.input(), Err(alephcore::capability::ui::UiError::Closed)));
    pty_survives(f, "FIX1_AFTER_CONNECTION_CLOSE").await;
    // Reusing the queued request key from the still-live same-principal peer
    // proves disconnect discarded its ownership instead of stranding a waiter.
    let surface = attach(&mut f.a, &f.session).await;
    let mut params = operation(&surface, "queued ownership was settled");
    params["idempotency_key"] = json!("disconnect-queued-slot");
    let request = f.a.send("capabilities.ui.invoke", params).await;
    let frame = tokio::time::timeout(Duration::from_secs(1), f.a.display()).await.expect("queued idempotency slot released on disconnect");
    assert!(f.a.rpc("capabilities.ui.ack", render_ack(&frame)).await.get("error").is_none());
    assert!(tokio::time::timeout(Duration::from_secs(1), f.a.reply(request)).await.unwrap().get("error").is_none());
    assert!(f.a.rpc("capabilities.ui.detach", surface).await.get("error").is_none());
}
