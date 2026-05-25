//! `gateway.bootstrap.issue` — issue a one-time nonce that a local browser
//! can exchange for an `aleph_session` cookie via `GET /auth/bootstrap?nonce=…`.
//!
//! Auth: requires a previously-authenticated context (bearer / session /
//! existing device token). Anonymous callers receive `-32001 unauthorized`.
//!
//! The handler does not enforce auth itself — reaching it is proof of auth
//! because the gateway dispatcher only invokes non-anonymous-allow-listed
//! handlers for authenticated callers.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::AuthContext;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Result of a successful `gateway.bootstrap.issue` call.
#[derive(Debug, Serialize, Deserialize)]
pub struct BootstrapIssueResult {
    /// Random one-shot nonce. Hand to the user agent via the `url`
    /// field — do not log it.
    pub nonce: String,
    /// How long the nonce remains valid, in seconds.
    pub expires_in_secs: u64,
    /// Loopback URL a same-machine client (CLI / Tauri menu / shell)
    /// should open in the browser. Composed server-side so callers
    /// don't have to know the bind address.
    pub url: String,
}

pub async fn handle_gateway_bootstrap_issue(
    req: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    // The gateway dispatcher only invokes us for authenticated callers
    // (bearer / session / valid device-token connection), so reaching
    // this handler is itself proof of auth. If you add an anonymous
    // dispatch path later, gate it here explicitly.

    let (nonce, expires_in_secs) = ctx.bootstrap_mgr.issue();
    let url = format!(
        "http://{bind}/auth/bootstrap?nonce={nonce}",
        bind = ctx.public_bind_for_loopback(),
    );
    JsonRpcResponse::success(
        req.id,
        serde_json::to_value(BootstrapIssueResult {
            nonce,
            expires_in_secs,
            url,
        })
        .expect("serialize BootstrapIssueResult"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn issues_nonce_and_url() {
        let ctx = crate::gateway::handlers::auth::tests::create_test_context();
        let req = JsonRpcRequest::with_id("gateway.bootstrap.issue", None, json!(1));
        let resp = handle_gateway_bootstrap_issue(req, ctx).await;
        let result = resp.result.expect("result");
        let issued: BootstrapIssueResult =
            serde_json::from_value(result).expect("BootstrapIssueResult");
        assert!(issued.nonce.len() >= 32, "expected long nonce");
        assert_eq!(issued.expires_in_secs, 60);
        assert!(issued.url.starts_with("http://"));
        assert!(
            issued.url.contains("/auth/bootstrap?nonce="),
            "expected url to contain bootstrap path, got {}",
            issued.url
        );
    }

    #[tokio::test]
    async fn issued_nonce_is_consumable_once() {
        let ctx = crate::gateway::handlers::auth::tests::create_test_context();
        let req = JsonRpcRequest::with_id("gateway.bootstrap.issue", None, json!(1));
        let resp = handle_gateway_bootstrap_issue(req, ctx.clone()).await;
        let issued: BootstrapIssueResult =
            serde_json::from_value(resp.result.unwrap()).expect("BootstrapIssueResult");
        // The same manager backs both issue and consume.
        ctx.bootstrap_mgr
            .consume(&issued.nonce)
            .expect("first consume succeeds");
        assert!(
            ctx.bootstrap_mgr.consume(&issued.nonce).is_err(),
            "second consume must fail (replay)"
        );
    }
}
