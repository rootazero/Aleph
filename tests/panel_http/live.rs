//! Opt-in browser fixture: real Core producer, Gateway router, WS handlers and
//! device-token validation. Only static shell hosting and file controls are test-only.

use std::path::PathBuf;

use axum::Router;
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};

use alephcore::gateway::handlers::auth::TransportPolicy;
use alephcore::gateway::handlers::connect::{handle_connect, ConnectContext};
use alephcore::gateway::security::store::UserRole;
use alephcore::gateway::security::{DeviceTokenManager, SecurityStore};
use alephcore::gateway::state_version::StateVersionTracker;
use std::sync::Arc;

use super::common::{manager, write_panel, Home};
use super::GatewayServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a freshly generated Panel shell and explicit file-control directory"]
async fn panel_asset_live_gateway_fixture() {
    let shell = PathBuf::from(std::env::var("PANEL_LIVE_SHELL").expect("PANEL_LIVE_SHELL"));
    assert!(
        shell.join("aleph_panel_bg.wasm").is_file(),
        "generate the current Panel first"
    );
    let control = PathBuf::from(std::env::var("PANEL_LIVE_CONTROL").expect("PANEL_LIVE_CONTROL"));
    std::fs::create_dir_all(&control).unwrap();
    for name in ["connection.json", "command.json", "status.json"] {
        match std::fs::remove_file(control.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("cannot reset local fixture control: {error}"),
        }
    }
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = Home::set(&home, root.path());
    let source = include_str!("../../interfaces/webchat/tests/fixtures/community-live.mjs");
    write_panel(&home, "1.0.0", source.as_bytes());
    let manager = manager(root.path()).await;
    assert_eq!(manager.panel_ui_capability_snapshot().declarations.len(), 1);
    assert!(alephcore::extension::init_extension_manager(manager.clone()).is_ok());
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    store
        .create_user("fixture-operator", "Fixture operator", UserRole::Admin)
        .unwrap();
    let tokens = Arc::new(DeviceTokenManager::new(store.clone()));
    let ticket = tokens
        .create_bootstrap_ticket(None, Some("fixture-operator"))
        .unwrap();
    let issued = tokens
        .exchange_bootstrap_ticket(
            &ticket,
            Some("fixture-panel".into()),
            Some("Live Panel fixture".into()),
            None,
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut gateway = GatewayServer::new(address);
    gateway.set_device_token_manager(tokens.clone());
    gateway.set_security_store(store);
    let connect = Arc::new(ConnectContext {
        state_versions: Arc::new(StateVersionTracker::new()),
        transport_policy: TransportPolicy::defaults(),
    });
    gateway.handlers_mut().register("connect", move |request| {
        handle_connect(request, connect.clone())
    });
    gateway.handlers_mut().register(
        "panel.capabilities.snapshot",
        alephcore::gateway::handlers::panel_capabilities::handle,
    );
    // The production /ws and /tabs routes win over the test-only generated SPA.
    let router: Router = gateway
        .build_router()
        .fallback_service(ServeDir::new(&shell).fallback(ServeFile::new(shell.join("index.html"))))
        .layer(alephcore::security::headers::SecurityHeadersLayer::new());
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    // Ephemeral test credential stays in ignored local tooling, never in a URL,
    // console output, publication record, report, or imported module ABI.
    std::fs::write(
        control.join("connection.json"),
        serde_json::to_vec(&json!({
            "origin": format!("http://{address}"), "token": issued.device_token,
        }))
        .unwrap(),
    )
    .unwrap();
    println!("Live Panel fixture ready (real Gateway/auth/Core); controls are local files");
    let mut sequence = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let Ok(bytes) = std::fs::read(control.join("command.json")) else {
            continue;
        };
        let Ok(command) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let next = command["sequence"].as_u64().unwrap_or(0);
        if next <= sequence {
            continue;
        }
        sequence = next;
        match command["action"].as_str().unwrap_or("") {
            "retract" => {
                manager.unmount("community-live").await.unwrap();
            }
            "replace" => {
                let source = source.replace("1.0.0", "2.0.0");
                write_panel(&home, "2.0.0", source.as_bytes());
                manager.reload().await.unwrap();
            }
            "broken" => {
                write_panel(
                    &home,
                    "3.0.0",
                    b"throw Error('intentional fixture loader failure');",
                );
                manager.reload().await.unwrap();
            }
            "revoke" => {
                tokens.revoke_device(&issued.device_id).unwrap();
            }
            "stop" => {
                break;
            }
            _ => panic!("unknown local fixture command"),
        }
        let snapshot = manager.panel_ui_capability_snapshot();
        std::fs::write(
            control.join("status.json"),
            serde_json::to_vec(&json!({
                "sequence": sequence, "revision": snapshot.revision,
                "declarations": snapshot.declarations.len(),
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let _ = shutdown.send(());
    server.abort();
}
