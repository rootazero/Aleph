//! In-process RPC -> mounted consume proof, without a browser or socket.
use super::*;
use crate::browser::error::BrowserError;
use crate::browser::live::grant::ExchangedGrant;
use crate::browser::live::issuer::tests::{fixture, nonce, ORIGIN, USER};
use crate::browser::live::viewer_backend::{BoxFuture, PreparedViewer, ViewerBackend};
use crate::gateway::caller_identity::with_verified_caller_identity;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use aleph_protocol::browser_live::{
    ConsumeFragmentNonceRequest, HumanInputEvent, IssueViewerGrantResponse,
};
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::json;
use std::time::Duration;
use tower::ServiceExt;

struct NeverPrepared;
impl ViewerBackend for NeverPrepared {
    fn prepare<'a>(
        &'a self,
        _grant: &'a ExchangedGrant,
    ) -> BoxFuture<'a, Result<PreparedViewer, BrowserError>> {
        Box::pin(async { panic!("nonce consume must not attach a browser") })
    }
    fn dispatch_input<'a>(
        &'a self,
        _route: &'a crate::browser::live::viewer_backend::ViewerInputRoute,
        _event: HumanInputEvent,
    ) -> BoxFuture<'a, Result<(), BrowserError>> {
        Box::pin(async { panic!("nonce consume must not dispatch input") })
    }
}

async fn issue(server: &GatewayServer, resource_id: &str) -> JsonRpcResponse {
    let request = JsonRpcRequest::with_id(
        "browser.viewer.grant.issue",
        Some(json!({"resource_id":resource_id,"permissions":["frames"]})),
        json!(1),
    );
    with_verified_caller_identity(
        Some("member".into()),
        Some(USER.into()),
        false,
        Some("connection-one".into()),
        Some(USER.into()),
        server.handlers.handle(&request),
    )
    .await
}

fn consume(issued: &IssueViewerGrantResponse, origin: &str) -> Request<Body> {
    let body = serde_json::to_string(&ConsumeFragmentNonceRequest {
        grant_id: issued.grant_id.clone(),
        nonce: nonce(issued),
    })
    .unwrap();
    let mut req = Request::builder()
        .method("POST")
        .uri("/viewer/browser/consume")
        .header("origin", origin)
        .header("host", "viewer.example.test")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    req
}

#[tokio::test]
async fn issuer_and_gateway_mounted_route_share_one_store_and_replace_denying_registration() {
    let f = fixture(Duration::from_secs(600));
    let mut server = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    server.config.allowed_origins = vec![ORIGIN.into()];
    server.register_browser_viewer_handlers();
    assert_eq!(
        issue(&server, &f.resource.resource_id)
            .await
            .error
            .unwrap()
            .message,
        "denied"
    );
    server.set_browser_viewer_backend(Arc::new(NeverPrepared), f.state.clone());
    // Builder-after-install must also retain the same issuer state.
    server.register_browser_viewer_handlers();
    let response = issue(&server, &f.resource.resource_id).await;
    assert!(response.error.is_none(), "{:?}", response.error);
    let issued: IssueViewerGrantResponse =
        serde_json::from_value(response.result.unwrap()).unwrap();
    assert!(f.grants.grant_info(&issued.grant_id).is_some());
    let app = server.build_router();
    // Neither a changed Origin nor a second default store can consume it.
    let rejected = app
        .clone()
        .oneshot(consume(&issued, "https://other.example.test"))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    let response = app.clone().oneshot(consume(&issued, ORIGIN)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("set-cookie").is_none());
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["ticket"].as_str().is_some());
    // A valid origin makes this assertion specifically about nonce state,
    // not the unrelated missing-origin denial.
    let origin = f.grants.grant_info(&issued.grant_id).unwrap().origin;
    assert!(f
        .grants
        .consume_nonce(&issued.grant_id, &nonce(&issued), Some(&origin))
        .is_err());
    let replay = app.oneshot(consume(&issued, ORIGIN)).await.unwrap();
    assert_eq!(replay.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn issuer_uninstalled_state_never_mounts_a_consumable_route() {
    let f = fixture(Duration::from_secs(600));
    let mut server = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    server.config.allowed_origins = vec![ORIGIN.into()];
    server.register_browser_viewer_handlers();
    assert_eq!(
        issue(&server, &f.resource.resource_id)
            .await
            .error
            .unwrap()
            .message,
        "denied"
    );
    let issued = f
        .state
        .issue(
            USER,
            aleph_protocol::browser_live::IssueViewerGrantRequest {
                resource_id: f.resource.resource_id,
                permissions: vec![aleph_protocol::browser_live::ViewerPermission::Frames],
            },
        )
        .unwrap();
    let response = server
        .build_router()
        .oneshot(consume(&issued, ORIGIN))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::OK);
    // Even a real nonce from another store cannot reach an absent route.
    let origin = f.grants.grant_info(&issued.grant_id).unwrap().origin;
    assert!(f
        .grants
        .consume_nonce(&issued.grant_id, &nonce(&issued), Some(&origin))
        .is_ok());
}
