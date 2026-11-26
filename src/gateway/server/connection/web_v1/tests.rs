use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    Router,
};
use tower::ServiceExt;

use crate::gateway::{
    security::{
        store::{SecurityStore, UserRole, UserStatus},
        DeviceTokenManager,
    },
    server::{GatewayConfig, GatewayServer},
};

fn fixture(tls_enabled: bool) -> (GatewayServer, Arc<SecurityStore>, Arc<DeviceTokenManager>) {
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    store
        .create_user("member-fixture", "Member fixture", UserRole::Member)
        .unwrap();
    let manager = Arc::new(DeviceTokenManager::new(store.clone()));
    let mut gateway = GatewayServer::with_config(
        "127.0.0.1:0".parse().unwrap(),
        GatewayConfig {
            tls_enabled,
            ..Default::default()
        },
    );
    gateway.set_security_store(store.clone());
    gateway.set_device_token_manager(manager.clone());
    (gateway, store, manager)
}

async fn exchange(
    app: Router,
    origin: Option<&str>,
    body: serde_json::Value,
) -> axum::response::Response {
    let mut request = Request::post("/v1/session")
        .header(header::HOST, "localhost:8443")
        .header(header::CONTENT_TYPE, "application/json")
        .extension(ConnectInfo(
            "127.0.0.1:45000".parse::<SocketAddr>().unwrap(),
        ));
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    app.oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn exchange_is_explicit_and_native_tls_only() {
    let (gateway, _, manager) = fixture(true);
    let app = gateway.native_tls_router(
        gateway.build_shared_state(),
        tokio_util::sync::CancellationToken::new(),
    );
    let ticket = manager
        .create_bootstrap_ticket(None, Some("member-fixture"))
        .unwrap();
    let origin = Some("https://localhost:8443");
    let body = serde_json::json!({"bootstrap_ticket": ticket});
    // `build_router` can be served by a wrapper without TLS, even when the
    // config intends native TLS. Only the TLS listener mounts these routes.
    assert_ne!(
        exchange(gateway.build_router(), origin, body.clone())
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let response = exchange(app.clone(), origin, body.clone()).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("__Host-aleph-v1="));
    for attribute in [
        "Secure",
        "HttpOnly",
        "SameSite=Strict",
        "Path=/",
        "Max-Age=300",
    ] {
        assert!(cookie
            .split(';')
            .skip(1)
            .any(|field| field.trim() == attribute));
    }
    assert!(!cookie.contains("Domain="));
    assert_eq!(
        exchange(app.clone(), origin, body.clone()).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        exchange(app.clone(), None, body.clone()).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        exchange(app.clone(), Some("https://localhost:8444"), body.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        exchange(
            app.clone(),
            origin,
            serde_json::json!({"bootstrap_ticket":"a", "device_token":"b"})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        exchange(
            app,
            origin,
            serde_json::json!({"bootstrap_ticket":ticket, "device_token":null})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let (plaintext, _, _) = fixture(false);
    assert_ne!(
        exchange(plaintext.build_router(), origin, body)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn web_pairing_requires_an_explicit_active_user_without_changing_legacy_pairing() {
    let (gateway, store, manager) = fixture(true);
    let app = gateway.native_tls_router(
        gateway.build_shared_state(),
        tokio_util::sync::CancellationToken::new(),
    );
    let origin = Some("https://localhost:8443");
    let unbound = manager.create_bootstrap_ticket(None, None).unwrap();
    let inactive = manager
        .create_bootstrap_ticket(None, Some("member-fixture"))
        .unwrap();
    store
        .update_user("member-fixture", None, None, Some(UserStatus::Deactivated))
        .unwrap();
    for ticket in [&unbound, &inactive] {
        let response = exchange(
            app.clone(),
            origin,
            serde_json::json!({"bootstrap_ticket": ticket}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }
    // Rejecting the unbound ticket did not consume it or alter old pairing.
    let device = manager
        .exchange_bootstrap_ticket(&unbound, None, None, None)
        .unwrap();
    let row = store.get_device(&device.device_id).unwrap().unwrap();
    assert_eq!(
        row.user_id.as_deref(),
        Some(crate::gateway::security::store::OWNER_USER_ID)
    );
}

#[test]
fn web_binding_tracks_exact_token_device_user_and_deadline() {
    let (_, store, manager) = fixture(true);
    let ticket = manager
        .create_bootstrap_ticket(None, Some("member-fixture"))
        .unwrap();
    let paired = manager
        .exchange_bootstrap_ticket(&ticket, None, None, None)
        .unwrap();
    let token = manager
        .validate_device_token(&paired.device_token)
        .unwrap()
        .unwrap();
    let binding = super::WebBinding {
        store: store.clone(),
        origin: "https://localhost:8443".into(),
        token_id: token.token_id.clone(),
        device_id: token.device_id.clone(),
        user: "member-fixture".into(),
        expires: tokio::time::Instant::now() + super::TTL,
    };
    assert_eq!(binding.current_role(), Some("member"));
    let mut expired = binding.clone();
    expired.expires = tokio::time::Instant::now();
    assert_eq!(expired.current_role(), None);
    let mut drifted = binding.clone();
    drifted.user = "u-owner".into();
    assert_eq!(drifted.current_role(), None);
    drifted.user = binding.user.clone();
    drifted.device_id = "another-device".into();
    assert_eq!(drifted.current_role(), None);
    assert_eq!(
        store
            .update_user(&binding.user, None, Some(UserRole::Admin), None)
            .unwrap(),
        1
    );
    assert_eq!(binding.current_role(), Some("operator"));
    assert_eq!(
        store
            .update_user(&binding.user, None, None, Some(UserStatus::Deactivated))
            .unwrap(),
        1
    );
    assert_eq!(binding.current_role(), None);
    assert_eq!(
        store
            .update_user(
                &binding.user,
                None,
                Some(UserRole::Member),
                Some(UserStatus::Active)
            )
            .unwrap(),
        1
    );
    assert_eq!(binding.current_role(), Some("member"));
    assert!(store.revoke_device_token(&token.token_id).unwrap());
    assert_eq!(binding.current_role(), None);
}

#[tokio::test]
#[ignore = "requires local Chrome and the workspace Playwright package"]
async fn web_socket_requires_hello_and_keeps_member_scope() {
    use crate::{
        gateway::{
            handlers::{connect, session::handle_list_db},
            router::SessionKey,
            session_store::{
                file_backend::{FileSessionStore, FileSessionStoreConfig},
                SessionStore,
            },
        },
        scope::{with_scope, ScopeAttribution},
    };
    use std::time::Duration;
    use tokio::time::{sleep, timeout};
    use tokio_util::sync::CancellationToken;

    super::super::super::install_ring_provider();
    let root = tempfile::tempdir().unwrap();
    let (mut gateway, store, manager) = fixture(true);
    gateway.config.max_connections = 2;
    let ticket = manager
        .create_bootstrap_ticket(None, Some("member-fixture"))
        .unwrap();
    let paired = manager
        .exchange_bootstrap_ticket(&ticket, None, None, None)
        .unwrap();
    let token = manager
        .validate_device_token(&paired.device_token)
        .unwrap()
        .unwrap();
    let sessions = Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: root.path().join("sessions"),
            ..Default::default()
        })
        .unwrap(),
    );
    let own = SessionKey::main("web-member-fixture");
    let other = SessionKey::main("web-other-fixture");
    for (user, key) in [("member-fixture", &own), ("u-owner", &other)] {
        let metadata = with_scope(
            Some(ScopeAttribution::personal(user)),
            sessions.get_or_create(key),
        )
        .await
        .unwrap();
        assert_eq!(metadata.owner_user_id.as_deref(), Some(user));
    }
    gateway
        .handlers_mut()
        .register("sessions.list", move |request| {
            handle_list_db(request, sessions.clone())
        });
    for method in [
        "graph.query",
        "graph.search",
        "graph.node_detail",
        "graph.update_note",
        "graph.rename_note",
        "graph.delete_note",
    ] {
        gateway
            .handlers_mut()
            .register(method, |request| async move {
                crate::gateway::protocol::JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"entered": request.method}),
                )
            });
    }
    let context = Arc::new(connect::ConnectContext {
        state_versions: gateway.state_versions.clone(),
        transport_policy: crate::gateway::handlers::auth::TransportPolicy::defaults(),
    });
    gateway.handlers_mut().register("connect", move |request| {
        connect::handle_connect(request, context.clone())
    });

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let cert =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".into(), "localhost".into()]).unwrap();
    let pem = cert.cert.pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        pem.as_bytes().to_vec(),
        cert.signing_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let alternate = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    alternate.set_nonblocking(true).unwrap();
    let alternate_origin = format!("https://{}", alternate.local_addr().unwrap());
    let stop = CancellationToken::new();

    // Chromium exposes rejected upgrades as generic socket errors. Observe the
    // HTTP result around the real router without logging credentials.
    #[derive(Default)]
    struct Observed {
        issued: Option<uuid::Uuid>,
        refused_capacity: bool,
        refused_origin_with_cookie: bool,
    }
    let observed = Arc::new(std::sync::Mutex::new(Observed::default()));
    let router = gateway
        .native_tls_router(gateway.build_shared_state(), stop.clone())
        .layer(axum::middleware::from_fn({
            let observed = observed.clone();
            let alternate_origin = alternate_origin.clone();
            move |request: Request<Body>, next: axum::middleware::Next| {
                let observed = observed.clone();
                let alternate_origin = alternate_origin.clone();
                async move {
                    let session = request.uri().path() == "/v1/session";
                    let ws = request.uri().path() == "/v1/ws";
                    let foreign = ws
                        && request
                            .headers()
                            .get(header::ORIGIN)
                            .and_then(|origin| origin.to_str().ok())
                            == Some(alternate_origin.as_str());
                    let cookie = if foreign {
                        super::cookie_handle(request.headers())
                    } else {
                        None
                    };
                    let response = next.run(request).await;
                    let mut observed = observed.lock().unwrap();
                    if session && response.status() == StatusCode::NO_CONTENT {
                        observed.issued = response
                            .headers()
                            .get(header::SET_COOKIE)
                            .and_then(|value| value.to_str().ok())
                            .and_then(|value| value.split(';').next())
                            .and_then(|field| field.strip_prefix("__Host-aleph-v1="))
                            .and_then(|value| value.parse().ok());
                    }
                    if ws && response.status() == StatusCode::SERVICE_UNAVAILABLE {
                        observed.refused_capacity = true;
                    }
                    if foreign
                        && response.status() == StatusCode::FORBIDDEN
                        && cookie.is_some()
                        && cookie == observed.issued
                    {
                        observed.refused_origin_with_cookie = true;
                    }
                    response
                }
            }
        }));
    let handle = axum_server::Handle::new();
    let server = tokio::spawn(
        axum_server::from_tcp_rustls(listener, tls.clone())
            .unwrap()
            .handle(handle.clone())
            .serve(router.into_make_service_with_connect_info::<SocketAddr>()),
    );
    let alternate_handle = axum_server::Handle::new();
    let alternate_server = tokio::spawn(
        axum_server::from_tcp_rustls(alternate, tls)
            .unwrap()
            .handle(alternate_handle.clone())
            .serve(
                gateway
                    .build_router()
                    .into_make_service_with_connect_info::<SocketAddr>(),
            ),
    );
    let fixture = root.path().join("fixture.json");
    std::fs::write(
        &fixture,
        serde_json::json!({
            "origin": format!("https://{addr}"), "certificate":pem,
            "alternateOrigin": alternate_origin,
            "token":paired.device_token, "ownKey":own.to_key_string(),
        })
        .to_string(),
    )
    .unwrap();
    let mut browser = tokio::process::Command::new("node")
        .args(["qa/browser_live/web-v1.cjs"])
        .arg(&fixture)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let outcome = timeout(Duration::from_secs(45), async {
        loop {
            if root.path().join("revoke").is_file() {
                {
                    let observed = observed.lock().unwrap();
                    assert!(
                        observed.refused_capacity,
                        "pending hellos did not receive a 503"
                    );
                    assert!(
                        observed.refused_origin_with_cookie,
                        "cross-port upgrade did not offer the issued Cookie and receive a 403"
                    );
                }
                let connection_id = std::fs::read_to_string(root.path().join("revoke")).unwrap();
                assert!(gateway
                    .connections
                    .read()
                    .await
                    .contains_key(&connection_id));
                assert!(store.revoke_device_token(&token.token_id).unwrap());
                timeout(Duration::from_secs(3), async {
                    while gateway
                        .connections
                        .read()
                        .await
                        .contains_key(&connection_id)
                    {
                        sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("idle revoked Web v1 connection must release its Gateway row");
                std::fs::write(root.path().join("revoked"), "ok").unwrap();
                return browser.wait().await.unwrap();
            }
            if let Some(status) = browser.try_wait().unwrap() {
                return status;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    stop.cancel();
    handle.graceful_shutdown(Some(Duration::from_secs(2)));
    alternate_handle.graceful_shutdown(Some(Duration::from_secs(2)));
    for task in [server, alternate_server] {
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    assert!(outcome.expect("browser fixture timed out").success());
    timeout(Duration::from_secs(2), async {
        while !gateway.connections.read().await.is_empty() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("v1 and legacy connection rows must be removed");
}

async fn get_panel(app: Router, path: &str) -> axum::response::Response {
    app.oneshot(
        Request::get(path)
            .header(header::HOST, "localhost:8443")
            .extension(ConnectInfo(
                "127.0.0.1:45000".parse::<SocketAddr>().unwrap(),
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn panel_route_absent_when_unconfigured() {
    let (gateway, _, _) = fixture(true);
    let app = gateway.native_tls_router(
        gateway.build_shared_state(),
        tokio_util::sync::CancellationToken::new(),
    );
    let response = get_panel(app, "/v1/panel/index.html").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn panel_route_serves_index_with_csp() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("index.html"), "<!doctype html>panel").unwrap();
    let (mut gateway, _, _) = fixture(true);
    gateway.config.panel_web_dir = Some(root.path().to_path_buf());
    let app = gateway.native_tls_router(
        gateway.build_shared_state(),
        tokio_util::sync::CancellationToken::new(),
    );
    for path in ["/v1/panel/", "/v1/panel/index.html"] {
        let response = get_panel(app.clone(), path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            "script-src 'self' 'wasm-unsafe-eval'; img-src 'self' blob: data:; connect-src 'self'"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
    }
}

#[tokio::test]
async fn panel_route_rejects_traversal() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("package");
    std::fs::create_dir(&package).unwrap();
    std::fs::write(root.path().join("secret.txt"), "outside").unwrap();
    let (mut gateway, _, _) = fixture(true);
    gateway.config.panel_web_dir = Some(package);
    let app = gateway.native_tls_router(
        gateway.build_shared_state(),
        tokio_util::sync::CancellationToken::new(),
    );
    for path in ["/v1/panel/../secret.txt", "/v1/panel/%2e%2e/secret.txt"] {
        let response = get_panel(app.clone(), path).await;
        assert_ne!(response.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn voice_turn_v1_ingress_is_server_derived() {
    use crate::gateway::handlers::voice_turn_tests::{
        mount_synthesis_bytes, transcribe_request, transcribe_request_with_samples,
        CaptureMediaFixture, SttFixture,
    };
    use aleph_protocol::panel_voice::{
        MAX_UTTERANCE_SAMPLES, VOICE_TURN_CAPTURE_CANCEL_METHOD, VOICE_TURN_CAPTURE_FINISH_METHOD,
        VOICE_TURN_CAPTURE_METHOD, VOICE_TURN_CAPTURE_READY_METHOD, VOICE_TURN_TRANSCRIBE_METHOD,
    };
    use futures::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use std::sync::atomic::Ordering;
    use tokio::{
        net::{TcpListener, TcpStream},
        time::timeout,
    };
    use tokio_tungstenite::{
        client_async,
        tungstenite::{client::IntoClientRequest, Message},
        WebSocketStream,
    };
    use tokio_util::sync::CancellationToken;

    async fn call(socket: &mut WebSocketStream<TcpStream>, request: Value) -> Value {
        let text = request.to_string();
        let mut frame = super::encode_header(text.len(), super::MAX_MESSAGE)
            .unwrap()
            .to_vec();
        frame.extend_from_slice(text.as_bytes());
        socket.send(Message::Binary(frame.into())).await.unwrap();
        let response = socket.next().await.unwrap().unwrap().into_data();
        let (text, tail) = super::split_frame(&response, super::MAX_MESSAGE)
            .unwrap()
            .expect("complete response frame");
        assert!(tail.is_empty());
        serde_json::from_str(text).unwrap()
    }

    let provider = SttFixture::new().await;
    let (mut gateway, _, manager) = fixture(true);
    gateway.config.require_idempotency_key = true;
    provider.mount(gateway.handlers_mut());
    let media = std::sync::Arc::new(CaptureMediaFixture::default());
    media.mount(gateway.handlers_mut());
    let tts_calls = mount_synthesis_bytes(gateway.handlers_mut(), 786_000);
    let stop = CancellationToken::new();
    let app = gateway.native_tls_router(gateway.build_shared_state(), stop.clone());
    let ticket = manager
        .create_bootstrap_ticket(None, Some("member-fixture"))
        .unwrap();
    let origin = "https://localhost:8443";
    let response = exchange(
        app.clone(),
        Some(origin),
        json!({"bootstrap_ticket":ticket}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    // Exercise the real native-router WebSocket admission and framed dispatch;
    // this loopback fixture does not repeat the separate TLS/browser test.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let outcome = timeout(std::time::Duration::from_secs(8), async {
        let upgrade = |cookie: &str| {
            let mut request = "ws://localhost:8443/v1/ws".into_client_request().unwrap();
            for (name, value) in [
                (header::ORIGIN, origin),
                (header::SEC_WEBSOCKET_PROTOCOL, super::SUBPROTOCOL),
                (header::COOKIE, cookie),
            ] {
                request.headers_mut().insert(name, value.parse().unwrap());
            }
            request
        };
        let error = client_async(
            upgrade("__Host-aleph-v1=invalid"),
            TcpStream::connect(addr).await.unwrap(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            tokio_tungstenite::tungstenite::Error::Http(response)
                if response.status() == StatusCode::UNAUTHORIZED
        ));
        assert_eq!(provider.calls(), 0);
        let (mut socket, _) =
            client_async(upgrade(cookie), TcpStream::connect(addr).await.unwrap())
                .await
                .unwrap();
        let hello = json!({"jsonrpc":"2.0","id":0,"method":"system.hello","params":{
            "protocol":{"major":1,"minor":0},
            "client":{"name":"voice-fixture","version":"1","platform":"test","ui_runtime":"test"},
            "features":[]
        }});
        let response = call(&mut socket, hello).await;
        assert!(response.get("result").is_some(), "{response}");
        let capture_id = uuid::Uuid::new_v4();
        for method in [VOICE_TURN_CAPTURE_METHOD, VOICE_TURN_CAPTURE_READY_METHOD,
            VOICE_TURN_CAPTURE_FINISH_METHOD, VOICE_TURN_CAPTURE_CANCEL_METHOD] {
            let response = call(&mut socket, json!({"jsonrpc":"2.0","id":"capture",
                "method":method,"params":{"capture_id":capture_id}})).await;
            assert_eq!(response["error"]["code"], crate::gateway::protocol::METHOD_NOT_FOUND);
        }
        let response = call(&mut socket, json!({"jsonrpc":"2.0","id":"indirect-capture",
            "method":VOICE_TURN_TRANSCRIBE_METHOD,"params":{
                "turn_id":uuid::Uuid::new_v4(),"capture_id":capture_id
            }})).await;
        assert_eq!(response["error"]["data"]["kind"], "permission_denied");
        let capture_entered = media.entered.notified();
        tokio::pin!(capture_entered);
        assert!(futures::poll!(&mut capture_entered).is_pending());
        assert_eq!(provider.calls(), 0);
        assert_eq!(
            call(&mut socket, transcribe_request_with_samples(MAX_UTTERANCE_SAMPLES)).await["result"]["text"],
            "fixture"
        );
        assert_eq!(provider.calls(), 1);
        let mut request = transcribe_request();
        request["method"] = json!("voice.transcribe");
        assert_eq!(
            call(&mut socket, request).await["error"]["code"],
            crate::gateway::protocol::METHOD_NOT_FOUND
        );
        assert_eq!(provider.calls(), 1);
        provider.config.write().await.generation.transcription_providers.clear();
        // Caller-supplied "desktop" metadata cannot elect offline fallback.
        assert_eq!(
            call(&mut socket, transcribe_request()).await["error"]["data"]["kind"],
            "not_configured"
        );
        assert_eq!(provider.calls(), 1);

        let mut request = json!({"jsonrpc":"2.0","id":"short","method":"voice.turn.synthesize","params":{
            "turn_id":"1b63aa6b-0688-4be3-991f-e6994e0f795c","text":""
        }});
        // Both Query methods work without a key under strict Web policy.
        let result = call(&mut socket, request.clone()).await;
        assert_eq!(result["error"]["data"]["kind"], "no_speech");
        assert_eq!(tts_calls.load(Ordering::SeqCst), 0);
        request["params"]["text"] = json!("Speak.");
        request["params"]["idempotency_key"] = json!("same");
        let result = call(&mut socket, request.clone()).await;
        assert_eq!(result["result"]["mime_type"], "audio/mpeg", "{:?}", result.get("error"));
        assert_eq!(
            result["result"]["audio_base64"].as_str().unwrap().len(),
            1_048_000
        );
        assert_eq!(tts_calls.load(Ordering::SeqCst), 1);
        let long_id = "\"".repeat(230);
        request["id"] = json!(long_id);
        // Identical caller key must not replay a body sized for a shorter id.
        let result = call(&mut socket, request).await;
        assert_eq!(result["id"], long_id);
        assert_eq!(result["error"]["data"]["kind"], "too_large");
        assert_eq!(tts_calls.load(Ordering::SeqCst), 2);
    })
    .await;
    stop.cancel();
    task.abort();
    outcome.unwrap();
}

#[tokio::test]
async fn drive_voice_media_reads_cancel_and_eof_before_stalled_reply_ack() {
    use crate::gateway::handlers::voice_turn_tests::{blocked_stt_cancel, blocked_stt_turn};
    use axum::extract::ws::Message;
    use serde_json::{json, Value};
    use tokio::sync::Notify;

    fn frame(request: Value) -> Result<Message, std::convert::Infallible> {
        Ok(super::wire_message(&request.to_string(), None, 0, false).unwrap())
    }

    for send_cancel in [true, false] {
        let (mut input, mut incoming) = futures::channel::mpsc::channel(2);
        let (writes, mut replies) = super::Writes::queue(2);
        let cancelled = Notify::new();
        let mut deferred = None;
        let active_id = Some(json!(1));
        let run = super::drive_voice_media(
            async {
                cancelled.notified().await;
                "cancelled".to_owned()
            },
            &mut incoming,
            &active_id,
            &mut deferred,
            &writes,
            |_, control| {
                if control.id == Some(json!(1001)) {
                    cancelled.notify_one();
                }
                std::future::ready(Some(Message::Binary(Vec::new().into())))
            },
        );
        let peer = async move {
            use futures::SinkExt;
            input.send(frame(blocked_stt_turn(3))).await.unwrap();
            input.send(frame(blocked_stt_cancel(2))).await.unwrap();
            let stalled_reply = replies.recv().await.unwrap();
            if send_cancel {
                input.send(frame(blocked_stt_cancel(1))).await.unwrap();
            }
            drop(input);
            // `join!` retains this item (and its ack sender) until the driver returns.
            (stalled_reply, replies)
        };
        let (result, _stalled_writer) =
            tokio::time::timeout(super::IO_TIMEOUT, async { tokio::join!(run, peer) })
                .await
                .expect("voice media must not wait for the blocked reply acknowledgment");
        assert_eq!(result.as_deref(), send_cancel.then_some("cancelled"));
        assert_eq!(deferred.unwrap().1.id, Some(json!(3)));
    }
}

#[tokio::test]
async fn drive_voice_media_reserves_control_id_until_write_ack() {
    use crate::gateway::handlers::voice_turn_tests::{blocked_stt_cancel, blocked_stt_turn};
    use axum::extract::ws::Message;
    use futures::SinkExt;
    use serde_json::{json, Value};
    use std::cell::Cell;

    fn frame(request: Value) -> Result<Message, std::convert::Infallible> {
        Ok(super::wire_message(&request.to_string(), None, 0, false).unwrap())
    }

    for second_is_cancel in [true, false] {
        for acknowledge in [false, true] {
            let (mut input, mut incoming) = futures::channel::mpsc::channel(2);
            let (writes, mut replies) = super::Writes::queue(super::MAX_PENDING_VOICE_CONTROLS + 1);
            let executed = Cell::new(0);
            let mut deferred = None;
            let active_id = Some(json!(1));
            let first = blocked_stt_cancel(2);
            let id = first["id"].clone();
            let mut second = if second_is_cancel {
                blocked_stt_cancel(3)
            } else {
                blocked_stt_turn(3)
            };
            second["id"] = id.clone();
            let run = super::drive_voice_media(
                std::future::pending::<String>(),
                &mut incoming,
                &active_id,
                &mut deferred,
                &writes,
                |_, _| {
                    executed.set(executed.get() + 1);
                    std::future::ready(Some(Message::Binary(Vec::new().into())))
                },
            );
            let peer = async move {
                input.send(frame(first)).await.unwrap();
                let reply = replies.recv().await.unwrap();
                let held = if acknowledge {
                    reply.sent.send(()).unwrap();
                    None
                } else {
                    Some(reply.sent)
                };
                input.send(frame(second)).await.unwrap();
                drop(input);
                // Retain both the held acknowledgment and the queue receiver
                // until the driver has admitted or refused the later frame.
                (held, replies)
            };
            let (result, _writer) =
                tokio::time::timeout(super::IO_TIMEOUT, async { tokio::join!(run, peer) })
                    .await
                    .expect("driver must not wait on a control write acknowledgment");
            assert!(result.is_none());
            assert_eq!(
                executed.get(),
                if acknowledge && second_is_cancel {
                    2
                } else {
                    1
                }
            );
            assert_eq!(
                deferred.and_then(|(_, request)| request.id),
                (acknowledge && !second_is_cancel).then_some(id)
            );
        }
    }
}

#[tokio::test]
async fn voice_turn_control_bypasses_full_web_query_lane() {
    blocked_voice_turn_web(true).await;
}

#[tokio::test]
async fn voice_turn_web_disconnect_drops_active_provider_wait() {
    blocked_voice_turn_web(false).await;
}

async fn blocked_voice_turn_web(cancel: bool) {
    use crate::gateway::{
        handlers::voice_turn_tests::{
            blocked_stt_cancel, blocked_stt_turn, fill_query_lane, BlockedSttFixture,
        },
        lane::ChannelClass,
    };
    use futures::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::{
        client_async,
        tungstenite::{client::IntoClientRequest, Message},
        WebSocketStream,
    };

    async fn send(socket: &mut WebSocketStream<TcpStream>, request: Value) {
        let text = request.to_string();
        let mut frame = super::encode_header(text.len(), super::MAX_MESSAGE)
            .unwrap()
            .to_vec();
        frame.extend_from_slice(text.as_bytes());
        socket.send(Message::Binary(frame.into())).await.unwrap();
    }
    async fn receive(socket: &mut WebSocketStream<TcpStream>) -> Value {
        let bytes = socket.next().await.unwrap().unwrap().into_data();
        let (text, tail) = super::split_frame(&bytes, super::MAX_MESSAGE)
            .unwrap()
            .unwrap();
        assert!(tail.is_empty());
        serde_json::from_str(text).unwrap()
    }

    let (mut gateway, _, manager) = fixture(true);
    gateway.config.require_idempotency_key = true;
    let provider = BlockedSttFixture::new(gateway.handlers_mut()).await;
    let lanes = gateway.lane_manager.clone();
    let stop = tokio_util::sync::CancellationToken::new();
    let app = gateway.native_tls_router(gateway.build_shared_state(), stop.clone());
    let origin = "https://localhost:8443";
    let ticket = manager
        .create_bootstrap_ticket(None, Some("member-fixture"))
        .unwrap();
    let exchange_reply = exchange(
        app.clone(),
        Some(origin),
        json!({"bootstrap_ticket":ticket}),
    )
    .await;
    assert_eq!(exchange_reply.status(), StatusCode::NO_CONTENT);
    let cookie = exchange_reply.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        let mut request = "ws://localhost:8443/v1/ws".into_client_request().unwrap();
        for (name, value) in [(header::ORIGIN, origin), (header::SEC_WEBSOCKET_PROTOCOL, super::SUBPROTOCOL), (header::COOKIE, cookie)] {
            request.headers_mut().insert(name, value.parse().unwrap());
        }
        let (mut socket, _) = client_async(request, TcpStream::connect(addr).await.unwrap()).await.unwrap();
        send(&mut socket, json!({"jsonrpc":"2.0","id":0,"method":"system.hello","params":{
            "protocol":{"major":1,"minor":0},
            "client":{"name":"voice-control-fixture","version":"1","platform":"test","ui_runtime":"test"},
            "features":[]
        }})).await;
        assert!(receive(&mut socket).await.get("result").is_some());
        send(&mut socket, blocked_stt_turn(1)).await;
        provider.entered(1).await;
        let _remaining_query_permits = fill_query_lane(&lanes, ChannelClass::Bot).await;
        if cancel {
            send(&mut socket, blocked_stt_cancel(1)).await;
            let mut responses = [receive(&mut socket).await, receive(&mut socket).await];
            responses.sort_by_key(|response| response["id"].as_u64().unwrap());
            assert_eq!(responses[0]["error"]["data"]["kind"], "cancelled");
            assert_eq!(responses[1]["result"]["cancelled"], true);
        }
        drop(socket);
        provider.exited(1).await;
    }).await;
    stop.cancel();
    server.abort();
    outcome.expect("blocked Web provider wait must end without releasing the fixture");
}
