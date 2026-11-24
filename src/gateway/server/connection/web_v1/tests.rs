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

#[tokio::test]
async fn web_binding_tracks_exact_token_device_user_and_deadline() {
    let (gateway, store, manager) = fixture(true);
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
    let expected = binding.snapshot("member");
    let mut row = super::ConnectionState::new("127.0.0.1".parse().unwrap(), false);
    row.first_message = false;
    row.caller_role = "member".into();
    row.caller_user = expected.user.clone();
    row.verified_user = expected.verified_user.clone();
    row.device_id = expected.device_id.clone();
    let connection_id = "still-connected".to_owned();
    gateway
        .connections
        .write()
        .await
        .insert(connection_id.clone(), row);
    let peer = |binding| {
        super::DispatchPeer::web(
            gateway.connections.clone(),
            connection_id.clone(),
            expected.clone(),
            binding,
        )
    };
    assert!(peer(binding.clone()).snapshot().await.is_some());
    store
        .issue_device_token(
            "expired-fixture",
            &binding.device_id,
            "expired-hash",
            "member",
            &[],
            -1,
        )
        .unwrap();
    let mut expired_token = binding.clone();
    expired_token.token_id = "expired-fixture".into();
    assert!(expired_token.expires > tokio::time::Instant::now());
    assert!(peer(expired_token).snapshot().await.is_none());
    assert!(gateway
        .connections
        .read()
        .await
        .contains_key(&connection_id));

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
