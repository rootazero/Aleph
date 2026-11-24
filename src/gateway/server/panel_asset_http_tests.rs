use std::net::SocketAddr;
use std::path::Path;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use base64::Engine;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use crate::discovery::DiscoveryConfig;
use crate::extension::{ExtensionConfig, ExtensionManager};
use crate::gateway::origin_policy::OriginPolicy;
use crate::gateway::security::{DeviceTokenManager, SecurityStore};
use crate::sync_primitives::Arc;

use super::panel_asset_route::{panel_asset_routes, PanelAssetRouteState};

const FIRST: &[u8] = b"export const revision = 1;";

#[tokio::test]
async fn panel_asset_http_preserves_existing_public_builtin_assets() {
    // RustEmbed debug serving uses the worktree's public build output. Create a
    // unique temporary public artifact, never a plugin root or a tracked file.
    let parent = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("interfaces/webchat/dist/tabs/terminal");
    std::fs::create_dir_all(&parent).unwrap();
    let public = tempfile::Builder::new()
        .prefix("http-compat-")
        .tempdir_in(&parent)
        .unwrap();
    std::fs::write(
        public.path().join("terminal.js"),
        b"export const builtin = true;",
    )
    .unwrap();
    let path = format!(
        "/tabs/terminal/{}/terminal.js",
        public.path().file_name().unwrap().to_str().unwrap()
    );
    let original = crate::gateway::control_plane::server::serve_static_asset(
        axum::http::HeaderMap::new(),
        axum::extract::Path(path.trim_start_matches('/').to_owned()),
    )
    .await;
    assert_eq!(original.status(), StatusCode::OK);
    let router = super::GatewayServer::new("127.0.0.1:0".parse().unwrap()).build_router();
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()));
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "community ingress must preserve public built-in assets"
    );
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        b"export const builtin = true;"
    );
}

const SECOND: &[u8] = b"export const revision = 2;";
const ENTRY: &str = "/tabs/community-live/1.0.0/ui/panel.mjs";

pub(super) fn write_panel(home: &Path, version: &str, bytes: &[u8]) {
    let root = home.join("plugins/community-live");
    std::fs::create_dir_all(root.join("ui")).unwrap();
    std::fs::write(root.join("ui/panel.mjs"), bytes).unwrap();
    std::fs::write(root.join("generic.mjs"), b"generic is not a Panel").unwrap();
    std::fs::write(root.join("aleph.plugin.toml"), format!(
        "[plugin]\nid = \"community-live\"\nname = \"Community\"\nversion = \"{version}\"\nkind = \"static\"\nentry = \"generic.mjs\"\n\n[panel]\ntab_id = \"overview\"\nlabel = \"Overview\"\nicon = \"layout\"\norder = 0\nentry = \"ui/panel.mjs\"\napi_major = 1\npermissions = []\nasset_integrity = \"sha256-{}\"\n",
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(bytes))
    )).unwrap();
}

pub(super) async fn manager(root: &Path) -> Arc<ExtensionManager> {
    std::fs::create_dir_all(root.join("project")).unwrap();
    let manager = ExtensionManager::new(ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: root.to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
            claude_home_override: None,
        },
        plugins_config_path: Some(root.join("plugins.toml")),
        extra_plugin_parents: vec![crate::discovery::ProjectPluginParent {
            dir: root.join("project/plugins"),
            project_root: root.join("project"),
        }],
    })
    .await
    .unwrap();
    manager.load_all().await.unwrap();
    Arc::new(manager)
}

fn state(manager: Arc<ExtensionManager>, secure: bool) -> Arc<PanelAssetRouteState> {
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    Arc::new(PanelAssetRouteState::new(
        Arc::new(DeviceTokenManager::new(store)),
        Arc::new(OriginPolicy::loopback_only()),
        false,
        vec![],
        false,
        secure,
        Arc::new(move || Some(manager.clone())),
        Arc::new(|token| token == "fixture-secret"),
    ))
}

fn request(path: &str, credential: bool, origin: Option<&str>, peer: &str) -> Request<Body> {
    let mut request = Request::builder()
        .uri(path)
        .header(header::HOST, "127.0.0.1");
    if credential {
        request = request.header(header::AUTHORIZATION, "Bearer fixture-secret");
    }
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    let mut request = request.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    request
}

async fn body(response: axum::response::Response) -> Vec<u8> {
    to_bytes(response.into_body(), 5 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn panel_asset_http_auth_guards_and_exact_current_declaration() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = crate::runtimes::post_install::HomeEnvGuards::acquire_and_set(&home, root.path());
    write_panel(&home, "1.0.0", FIRST);
    let manager = manager(root.path()).await;
    assert_eq!(manager.panel_ui_capability_snapshot().declarations.len(), 1);
    let router = panel_asset_routes(state(manager.clone(), false));
    let ok = router
        .clone()
        .oneshot(request(
            ENTRY,
            true,
            Some("http://127.0.0.1"),
            "127.0.0.1:4000",
        ))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.headers()[header::CACHE_CONTROL], "private, no-store");
    assert_eq!(
        ok.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(ok.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(body(ok).await, FIRST);

    let refused = router
        .clone()
        .oneshot(request(ENTRY, false, None, "127.0.0.1:4000"))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    let refusal = body(refused).await;
    for (path, origin, peer) in [
        (
            ENTRY.to_string(),
            Some("https://attacker.invalid"),
            "127.0.0.1:4000",
        ),
        (ENTRY.to_string(), None, "203.0.113.7:4000"),
        (format!("{ENTRY}?revision=1"), None, "127.0.0.1:4000"),
        (
            format!("{ENTRY}?url=https://attacker.invalid"),
            None,
            "127.0.0.1:4000",
        ),
        (
            "/tabs/community-live/1.0.0/generic.mjs".into(),
            None,
            "127.0.0.1:4000",
        ),
        (
            "/tabs/community-live/1.0.0/%75i/panel.mjs".into(),
            None,
            "127.0.0.1:4000",
        ),
        (
            "/tabs/community-live/1.0.0/../generic.mjs".into(),
            None,
            "127.0.0.1:4000",
        ),
        (
            "/tabs/missing/1.0.0/ui/panel.mjs".into(),
            None,
            "127.0.0.1:4000",
        ),
    ] {
        let response = router
            .clone()
            .oneshot(request(&path, true, origin, peer))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(body(response).await, refusal, "{path}");
    }
    let mut project = request(ENTRY, true, None, "127.0.0.1:4000");
    project
        .headers_mut()
        .insert("x-aleph-project", "unverified".parse().unwrap());
    assert_eq!(
        router.clone().oneshot(project).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    let secure = panel_asset_routes(state(manager.clone(), true));
    assert_eq!(
        secure
            .oneshot(request(ENTRY, true, None, "203.0.113.7:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let mut spoof = request(ENTRY, true, None, "203.0.113.7:4000");
    spoof
        .headers_mut()
        .insert("x-forwarded-proto", "https".parse().unwrap());
    assert_eq!(
        router.clone().oneshot(spoof).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let projected = manager.clone();
    let proxy = panel_asset_routes(Arc::new(PanelAssetRouteState::new(
        Arc::new(DeviceTokenManager::new(store)),
        Arc::new(OriginPolicy::loopback_only()),
        true,
        vec!["127.0.0.1".parse().unwrap()],
        false,
        false,
        Arc::new(move || Some(projected.clone())),
        Arc::new(|token| token == "fixture-secret"),
    )));
    let mut forwarded = request(ENTRY, true, None, "127.0.0.1:4000");
    forwarded
        .headers_mut()
        .insert("x-forwarded-for", "203.0.113.7".parse().unwrap());
    assert_eq!(
        proxy.clone().oneshot(forwarded).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let mut forwarded = request(ENTRY, true, None, "127.0.0.1:4000");
    forwarded
        .headers_mut()
        .insert("x-forwarded-for", "203.0.113.7".parse().unwrap());
    forwarded
        .headers_mut()
        .insert("x-forwarded-proto", "https".parse().unwrap());
    assert_eq!(
        proxy.oneshot(forwarded).await.unwrap().status(),
        StatusCode::OK
    );

    // A private HTTP bucket also limits authenticated loopback requests.
    let limited = panel_asset_routes(state(manager.clone(), false));
    for _ in 0..120 {
        assert_eq!(
            limited
                .clone()
                .oneshot(request(ENTRY, true, None, "127.0.0.1:4000"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    let excess = limited
        .oneshot(request(ENTRY, true, None, "127.0.0.1:4000"))
        .await
        .unwrap();
    assert_eq!(excess.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(excess).await, refusal);

    std::fs::write(home.join("plugins/community-live/ui/panel.mjs"), SECOND).unwrap();
    assert_eq!(
        body(
            router
                .clone()
                .oneshot(request(ENTRY, true, None, "127.0.0.1:4000"))
                .await
                .unwrap()
        )
        .await,
        FIRST
    );
    manager.reload_plugin("community-live").await.unwrap();
    assert_eq!(
        router
            .clone()
            .oneshot(request(ENTRY, true, None, "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    write_panel(&home, "2.0.0", SECOND);
    manager.reload_plugin("community-live").await.unwrap();
    assert_eq!(
        router
            .clone()
            .oneshot(request(ENTRY, true, None, "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let next = "/tabs/community-live/2.0.0/ui/panel.mjs";
    assert_eq!(
        body(
            router
                .clone()
                .oneshot(request(next, true, None, "127.0.0.1:4000"))
                .await
                .unwrap()
        )
        .await,
        SECOND
    );
    manager.unmount("community-live").await.unwrap();
    assert_eq!(
        router
            .oneshot(request(next, true, None, "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn panel_asset_http_project_scope_is_not_a_client_claim() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = crate::runtimes::post_install::HomeEnvGuards::acquire_and_set(&home, root.path());
    write_panel(&root.path().join("project"), "1.0.0", FIRST);
    let manager = manager(root.path()).await;
    assert!(manager
        .panel_ui_capability_snapshot()
        .declarations
        .is_empty());
    let context = crate::extension::visibility::VisibilityCtx::from_project_root(Some(
        root.path().join("project"),
    ));
    assert_eq!(
        manager
            .panel_ui_capability_snapshot_for(&context)
            .declarations
            .len(),
        1
    );
    let routes = panel_asset_routes(state(manager, false));
    for path in [ENTRY.to_string(), format!("{ENTRY}?project=project")] {
        let mut request = request(&path, true, None, "127.0.0.1:4000");
        request
            .headers_mut()
            .insert("x-aleph-project", "project".parse().unwrap());
        assert_eq!(
            routes.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        routes
            .oneshot(request(ENTRY, true, None, "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn panel_asset_http_missing_core_never_falls_through_to_spa() {
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let routes = panel_asset_routes(Arc::new(PanelAssetRouteState::new(
        Arc::new(DeviceTokenManager::new(store)),
        Arc::new(OriginPolicy::loopback_only()),
        false,
        vec![],
        false,
        false,
        Arc::new(|| None),
        Arc::new(|_| true),
    )))
    .fallback(|| async { "SPA fallback must not answer assets" });
    let response = routes
        .oneshot(request(ENTRY, true, None, "127.0.0.1:4000"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(response).await, b"Panel asset unavailable");
    let gateway = super::GatewayServer::new("127.0.0.1:0".parse().unwrap());
    let response = gateway
        .build_router()
        .oneshot(request(ENTRY, false, None, "127.0.0.1:4000"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(response).await, b"Panel asset unavailable");
}
