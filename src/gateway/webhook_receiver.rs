//! Webhook Receiver — Shared HTTP Server for Channel Webhook Ingestion
//!
//! Provides a reusable HTTP server that social bot channels (`WhatsApp`, Generic Webhook, etc.)
//! can register webhook handlers on. Each handler gets its own URL path and performs
//! platform-specific signature verification and payload parsing.
//!
//! # Difference from `webhooks` Module
//!
//! The `webhooks` module handles external service webhooks (GitHub, Stripe, etc.) that
//! trigger agent actions. This module handles **channel-level** webhook ingestion —
//! converting incoming platform messages into `InboundMessage` for the channel system.
//!
//! # Architecture
//!
//! ```text
//! External Platform (WhatsApp, Generic, etc.)
//!        │ HTTP POST
//!        ▼
//! ┌──────────────────────┐
//! │   WebhookReceiver    │  ← Shared axum HTTP server
//! │  ┌────────────────┐  │
//! │  │ WhatsApp Handler│  │  ← /webhook/whatsapp
//! │  │ Generic Handler │  │  ← /webhook/generic
//! │  └────────────────┘  │
//! └──────────┬───────────┘
//!            │ InboundMessage
//!            ▼
//!      ChannelRegistry
//! ```
//!
//! # Security
//!
//! - HMAC-SHA256 signature verification with constant-time comparison
//! - Per-handler secret management
//! - Configurable signature header format (`sha256={hex}`)

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::channel::{
    ChannelId, ChannelResult, ChannelStatus, InboundMessage, InboundMessageSender,
};

type HmacSha256 = Hmac<Sha256>;

/// Trait for platform-specific webhook handling.
///
/// Each channel (`WhatsApp`, Generic Webhook, etc.) implements this trait to:
/// 1. Verify the incoming request signature
/// 2. Parse the platform-specific payload into `InboundMessage`(s)
/// 3. Declare its URL path
#[async_trait]
pub trait WebhookHandler: Send + Sync {
    /// Verify webhook signature (HMAC-SHA256, platform-specific headers, etc.)
    ///
    /// Implementations should extract the signature from the appropriate header
    /// and verify it against the request body using their secret.
    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> bool;

    /// Parse webhook payload into `InboundMessages`.
    ///
    /// A single webhook request may produce multiple messages (e.g., batch delivery).
    async fn handle(&self, headers: &HeaderMap, body: Bytes) -> ChannelResult<Vec<InboundMessage>>;

    /// URL path for this handler (e.g., "/webhook/whatsapp").
    ///
    /// Must start with `/` and be unique across all registered handlers.
    fn path(&self) -> &str;
}

/// One webhook handler mounted at its own path, paired with the inbound sink
/// of the channel that owns it.
///
/// The sink is the channel's own broadcast (`ChannelState::sender()`), not the
/// registry's — going direct would bypass `start_message_forwarder`, the only
/// place inbound traffic stamps channel health.
pub struct WebhookMount {
    pub handler: Arc<dyn WebhookHandler>,
    pub inbound: InboundMessageSender,
    /// The owning channel's shared status cell (`ChannelState::status_handle()`).
    ///
    /// The route table built from these mounts is a boot-time snapshot with no
    /// invalidation — `channel.stop()`/`channel.delete` cannot remove a route
    /// that already exists. Reading this status per-request is what actually
    /// stops a stopped/deleted channel from still answering HTTP, without
    /// building a dynamic mount table.
    pub status: Arc<tokio::sync::RwLock<ChannelStatus>>,
    /// The owning channel's id, so a skipped mount's warning names which
    /// config section is at fault instead of only the (possibly shared) path.
    pub channel_id: ChannelId,
}

/// The one path prefix every channel webhook route lives under.
///
/// A single constant route (`/webhook/{*rest}`) carries all channel webhook
/// traffic, so an operator-writable `path` never enters axum's route table.
/// That is what makes the mount table hot-swappable *and* what removed the
/// boot-panic failure mode `RESERVED_ROUTE_PREFIXES` used to guard against.
pub const WEBHOOK_ROUTE_PREFIX: &str = "/webhook";

/// Whether `path` can be reached behind `{WEBHOOK_ROUTE_PREFIX}/{{*rest}}`.
///
/// Requires a whole extra segment: `/webhook` and `/webhookx/y` are both out
/// (the wildcard needs at least one segment, and the prefix must end on a
/// segment boundary).
fn is_mountable_path(path: &str) -> bool {
    path.strip_prefix(WEBHOOK_ROUTE_PREFIX)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|sub| !sub.is_empty())
}

/// What a request needs, cloned out of the table's read guard.
///
/// Handler work is async; holding the table's `RwLock` across that await
/// would let one slow platform starve every other mount, plus block every
/// `channel.start` / `stop`.
pub(crate) struct MountedHandler {
    pub(crate) handler: Arc<dyn WebhookHandler>,
    pub(crate) inbound: InboundMessageSender,
    pub(crate) status: Arc<RwLock<ChannelStatus>>,
}

/// Live `path -> mount` map behind the single webhook route.
///
/// Owned by `ChannelRegistry`, which is the only thing allowed to mutate it —
/// mounting follows channel lifecycle instead of being a boot snapshot. A
/// route that `channel.stop` / `channel.delete` cannot remove is an
/// authenticated endpoint the operator believes is gone.
#[derive(Default)]
pub struct WebhookMountTable {
    mounts: RwLock<HashMap<String, WebhookMount>>,
}

impl WebhookMountTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mount `mount` at its handler's declared path.
    ///
    /// Returns `false` when the mount was refused; every refusal is warned
    /// with the offending channel id, never panicked — `path` is operator
    /// config.
    pub async fn mount(&self, mount: WebhookMount) -> bool {
        let path = mount.handler.path().to_string();

        if !is_mountable_path(&path) {
            warn!(
                channel_id = %mount.channel_id,
                path = %path,
                prefix = WEBHOOK_ROUTE_PREFIX,
                "webhook path must be \"{WEBHOOK_ROUTE_PREFIX}/<name>\" — handler not mounted"
            );
            return false;
        }

        let mut mounts = self.mounts.write().await;
        if let Some(existing) = mounts.get(&path) {
            // Same channel restarting: always take the fresh handler. Keeping
            // the old clone is precisely the staleness this table prevents.
            if existing.channel_id != mount.channel_id {
                // Two channels want one path — operator misconfiguration. Pick
                // by channel id, not by arrival order: `start_all` iterates a
                // HashMap, so arrival order would make route ownership a
                // per-boot coin flip.
                if existing.channel_id.as_str() <= mount.channel_id.as_str() {
                    warn!(
                        path = %path,
                        holder = %existing.channel_id,
                        refused = %mount.channel_id,
                        "duplicate webhook path — keeping the lower channel id, handler not mounted"
                    );
                    return false;
                }
                warn!(
                    path = %path,
                    evicted = %existing.channel_id,
                    holder = %mount.channel_id,
                    "duplicate webhook path — lower channel id takes over the route"
                );
            }
        }

        let channel_id = mount.channel_id.clone();
        mounts.insert(path.clone(), mount);
        info!(path = %path, channel_id = %channel_id, "webhook handler mounted");
        true
    }

    /// Remove every mount owned by `channel_id`. Returns how many were removed.
    ///
    /// Called on stop / delete / re-register. Idempotent.
    pub async fn unmount_channel(&self, channel_id: &ChannelId) -> usize {
        let mut mounts = self.mounts.write().await;
        let before = mounts.len();
        mounts.retain(|_, mount| &mount.channel_id != channel_id);
        let removed = before - mounts.len();
        if removed > 0 {
            info!(
                channel_id = %channel_id,
                removed,
                "webhook handler(s) unmounted"
            );
        }
        removed
    }

    /// How many paths are live. Boot logging and diagnostics only.
    pub async fn mounted_count(&self) -> usize {
        self.mounts.read().await.len()
    }

    /// Exact-path lookup. The key is the configured path verbatim.
    pub(crate) async fn lookup(&self, path: &str) -> Option<MountedHandler> {
        let mounts = self.mounts.read().await;
        let mount = mounts.get(path)?;
        Some(MountedHandler {
            handler: Arc::clone(&mount.handler),
            inbound: mount.inbound.clone(),
            status: Arc::clone(&mount.status),
        })
    }
}

/// Builds the axum routes for channel webhook ingestion.
///
/// This does **not** own a listener. The gateway's own server merges these
/// routes into `build_router()`, so webhook traffic inherits the configured
/// bind address, TLS, and security headers. The previous version bound
/// `0.0.0.0` itself, which silently opened a LAN port regardless of
/// `[gateway] host`.
pub struct WebhookReceiver;

impl WebhookReceiver {
    /// Build the router for the given mounts.
    ///
    /// A mount whose path collides with a gateway route, or with an earlier
    /// mount, is skipped with a warning — `Router::merge` panics on duplicate
    /// routes and `path` is an operator-writable config field, so a typo must
    /// not take the daemon down at boot. A path missing its leading `/` is
    /// likewise skipped rather than left to panic in `Router::route` — the
    /// only current producer validates this (`WebhookChannelConfig::validate`),
    /// but that is enforced by convention, not by this function, and a future
    /// `WebhookHandler` without that validation should not be able to take the
    /// daemon down at boot.
    pub fn router(mounts: Vec<WebhookMount>) -> Router {
        let mut router = Router::new();
        let mut mounted: Vec<String> = Vec::new();

        for mount in mounts {
            let path = mount.handler.path().to_string();

            if !path.starts_with('/') {
                warn!(
                    channel_id = %mount.channel_id,
                    path = %path,
                    "webhook path missing leading '/' — handler not mounted"
                );
                continue;
            }
            if crate::gateway::server::is_reserved_route(&path) {
                warn!(
                    channel_id = %mount.channel_id,
                    path = %path,
                    "webhook path collides with a gateway route — handler not mounted"
                );
                continue;
            }
            if mounted.iter().any(|p| p == &path) {
                warn!(
                    channel_id = %mount.channel_id,
                    path = %path,
                    "duplicate webhook path — handler not mounted"
                );
                continue;
            }

            let handler_state = Arc::new(HandlerState {
                handler: mount.handler,
                inbound: mount.inbound,
                status: mount.status,
            });
            router = router.route(&path, post(webhook_endpoint).with_state(handler_state));
            info!(path = %path, "Registered webhook handler");
            mounted.push(path);
        }

        router
    }

    /// Compute HMAC-SHA256 signature of data with the given secret.
    ///
    /// Returns the signature in the format `"sha256={hex_digest}"`.
    #[must_use]
    pub fn compute_signature(secret: &str, data: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .unwrap_or_else(|_| unreachable!("HMAC accepts any key size"));
        mac.update(data);
        let result = mac.finalize();
        let hex_str = hex::encode(result.into_bytes());
        format!("sha256={hex_str}")
    }

    /// Verify an HMAC-SHA256 signature using constant-time comparison.
    ///
    /// The `signature` parameter should be in `"sha256={hex_digest}"` format.
    /// Returns `true` if the signature matches.
    #[must_use]
    pub fn verify_signature(secret: &str, body: &[u8], signature: &str) -> bool {
        let expected = Self::compute_signature(secret, body);
        crate::security::secret_equal_bytes(expected.as_bytes(), signature.as_bytes())
    }
}

/// Internal state passed to each axum handler.
struct HandlerState {
    handler: Arc<dyn WebhookHandler>,
    inbound: InboundMessageSender,
    status: Arc<tokio::sync::RwLock<ChannelStatus>>,
}

/// Axum endpoint handler that dispatches to the appropriate `WebhookHandler`.
async fn webhook_endpoint(
    State(state): State<Arc<HandlerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Step 0: Reject before doing any other work if we have positive evidence
    // the owning channel is not connected (stopped or deleted). The mount
    // table has no invalidation of its own, so this check is what actually
    // stops a "stopped" channel from still answering HTTP.
    //
    // `try_read` intentionally fails OPEN on contention: a momentary
    // write-lock holder (another request's status flip in flight) is not
    // evidence the channel is down, and dropping live traffic on a lock race
    // would be a worse bug than the one this guards against.
    if let Ok(status) = state.status.try_read() {
        if *status != ChannelStatus::Connected {
            warn!(
                path = %state.handler.path(),
                status = ?*status,
                "webhook received for a channel that is not connected — rejecting"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                String::from("Service Unavailable: channel is not connected"),
            );
        }
    }

    // Step 1: Verify signature
    if !state.handler.verify(&headers, &body) {
        warn!(path = %state.handler.path(), "Webhook signature verification failed");
        return (
            StatusCode::FORBIDDEN,
            String::from("Forbidden: invalid signature"),
        );
    }

    // Step 2: Parse payload into messages
    match state.handler.handle(&headers, body).await {
        Ok(messages) => {
            let mut dropped = 0usize;
            for msg in messages {
                if state.inbound.send(msg).is_err() {
                    dropped += 1;
                    warn!(
                        path = %state.handler.path(),
                        "Failed to forward inbound message (no subscriber on the channel)"
                    );
                }
            }
            if dropped > 0 {
                // 503 so the sender retries: silently returning 200 would let
                // messages vanish while the channel looks healthy.
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("Dropped {dropped} messages: channel has no subscriber"),
                );
            }
            (StatusCode::OK, String::from("ok"))
        }
        Err(e) => {
            warn!(
                path = %state.handler.path(),
                error = %e,
                "Webhook handler error"
            );
            (StatusCode::BAD_REQUEST, String::from("Bad request"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{
        ChannelError, ChannelId, ChannelState, ConversationId, MessageId, UserId,
    };
    use chrono::Utc;

    // --- HMAC signature tests ---

    #[test]
    fn test_hmac_signature_verification() {
        let secret = "test-webhook-secret";
        let body = b"test body content for signature verification";
        let sig = WebhookReceiver::compute_signature(secret, body);

        assert!(sig.starts_with("sha256="));
        assert!(WebhookReceiver::verify_signature(secret, body, &sig));

        // Deterministic: same input produces same output
        let sig2 = WebhookReceiver::compute_signature(secret, body);
        assert_eq!(sig, sig2);
    }

    #[test]
    fn test_hmac_signature_rejects_invalid() {
        let secret = "my-secret";
        let body = b"some request body";
        let sig = WebhookReceiver::compute_signature(secret, body);

        // Wrong secret
        assert!(!WebhookReceiver::verify_signature(
            "wrong-secret",
            body,
            &sig
        ));
        // Wrong body
        assert!(!WebhookReceiver::verify_signature(
            secret,
            b"different body",
            &sig
        ));
        // Completely wrong signature
        assert!(!WebhookReceiver::verify_signature(
            secret,
            body,
            "sha256=0000000000000000000000000000000000000000000000000000000000000000"
        ));
        // Truncated signature
        assert!(!WebhookReceiver::verify_signature(
            secret,
            body,
            "sha256=bad"
        ));
    }

    #[test]
    fn test_hmac_constant_time_comparison() {
        let secret = "timing-attack-test";
        let body = b"sensitive payload";
        let sig = WebhookReceiver::compute_signature(secret, body);

        // Modify the last character — should still be rejected
        let mut tampered = sig.clone();
        let last_byte = tampered.pop().unwrap();
        let replacement = if last_byte == 'a' { 'b' } else { 'a' };
        tampered.push(replacement);

        assert_ne!(sig, tampered);
        assert!(!WebhookReceiver::verify_signature(secret, body, &tampered));
    }

    #[test]
    fn test_hmac_empty_body() {
        let secret = "secret-for-empty";
        let body = b"";
        let sig = WebhookReceiver::compute_signature(secret, body);

        assert!(sig.starts_with("sha256="));
        assert!(WebhookReceiver::verify_signature(secret, body, &sig));
    }

    #[test]
    fn test_hmac_empty_secret() {
        let secret = "";
        let body = b"body with empty secret";
        let sig = WebhookReceiver::compute_signature(secret, body);

        assert!(sig.starts_with("sha256="));
        assert!(WebhookReceiver::verify_signature(secret, body, &sig));
    }

    #[test]
    fn test_hmac_different_data_produces_different_signatures() {
        let secret = "same-secret";
        let sig1 = WebhookReceiver::compute_signature(secret, b"data1");
        let sig2 = WebhookReceiver::compute_signature(secret, b"data2");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_hmac_different_secrets_produce_different_signatures() {
        let body = b"same body";
        let sig1 = WebhookReceiver::compute_signature("secret1", body);
        let sig2 = WebhookReceiver::compute_signature("secret2", body);
        assert_ne!(sig1, sig2);
    }

    // --- WebhookHandler trait + integration test ---

    /// Mock handler for testing the webhook endpoint.
    struct MockWebhookHandler {
        secret: String,
        handler_path: String,
    }

    #[async_trait]
    impl WebhookHandler for MockWebhookHandler {
        fn verify(&self, headers: &HeaderMap, body: &[u8]) -> bool {
            let signature = headers
                .get("X-Webhook-Signature")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            WebhookReceiver::verify_signature(&self.secret, body, signature)
        }

        async fn handle(
            &self,
            _headers: &HeaderMap,
            body: Bytes,
        ) -> ChannelResult<Vec<InboundMessage>> {
            let json: serde_json::Value = serde_json::from_slice(&body)
                .map_err(|e| ChannelError::ReceiveFailed(format!("Invalid JSON: {e}")))?;

            let text = json["text"]
                .as_str()
                .ok_or_else(|| ChannelError::ReceiveFailed("Missing 'text' field".into()))?
                .to_string();

            Ok(vec![InboundMessage {
                id: MessageId::new("mock-msg-1"),
                channel_id: ChannelId::new("mock-channel"),
                conversation_id: ConversationId::new("mock-conv"),
                sender_id: UserId::new("mock-user"),
                sender_name: Some("Mock User".into()),
                text,
                attachments: vec![],
                timestamp: Utc::now(),
                reply_to: None,
                is_group: false,
                raw: None,
                metadata: vec![],
            }])
        }

        fn path(&self) -> &str {
            &self.handler_path
        }
    }

    #[tokio::test]
    async fn test_webhook_endpoint_valid_signature() {
        use crate::gateway::channel::ChannelState;
        use axum::http::Request;
        use tower::ServiceExt;

        let secret = "integration-test-secret";
        let handler: Arc<dyn WebhookHandler> = Arc::new(MockWebhookHandler {
            secret: secret.to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let channel_state = ChannelState::new(16);
        channel_state.set_status(ChannelStatus::Connected).await;
        let mut rx = channel_state.inbound_subscribe();

        let handler_state = Arc::new(HandlerState {
            handler: Arc::clone(&handler),
            inbound: channel_state.sender(),
            status: channel_state.status_handle(),
        });

        let app = Router::new()
            .route("/webhook/mock", post(webhook_endpoint))
            .with_state(handler_state);

        let body = r#"{"text":"Hello from webhook!"}"#;
        let sig = WebhookReceiver::compute_signature(secret, body.as_bytes());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/mock")
                    .header("X-Webhook-Signature", &sig)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Verify the message was forwarded
        let msg = rx.try_recv().expect("Should have received inbound message");
        assert_eq!(msg.text, "Hello from webhook!");
        assert_eq!(msg.channel_id.as_str(), "mock-channel");
    }

    #[tokio::test]
    async fn test_webhook_endpoint_invalid_signature() {
        use crate::gateway::channel::ChannelState;
        use axum::http::Request;
        use tower::ServiceExt;

        let handler: Arc<dyn WebhookHandler> = Arc::new(MockWebhookHandler {
            secret: "real-secret".to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let channel_state = ChannelState::new(16);
        channel_state.set_status(ChannelStatus::Connected).await;

        let handler_state = Arc::new(HandlerState {
            handler: Arc::clone(&handler),
            inbound: channel_state.sender(),
            status: channel_state.status_handle(),
        });

        let app = Router::new()
            .route("/webhook/mock", post(webhook_endpoint))
            .with_state(handler_state);

        let body = r#"{"text":"Unauthorized!"}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/mock")
                    .header("X-Webhook-Signature", "sha256=invalid")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_webhook_endpoint_missing_signature() {
        use crate::gateway::channel::ChannelState;
        use axum::http::Request;
        use tower::ServiceExt;

        let handler: Arc<dyn WebhookHandler> = Arc::new(MockWebhookHandler {
            secret: "some-secret".to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let channel_state = ChannelState::new(16);
        channel_state.set_status(ChannelStatus::Connected).await;

        let handler_state = Arc::new(HandlerState {
            handler: Arc::clone(&handler),
            inbound: channel_state.sender(),
            status: channel_state.status_handle(),
        });

        let app = Router::new()
            .route("/webhook/mock", post(webhook_endpoint))
            .with_state(handler_state);

        let body = r#"{"text":"No sig!"}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/mock")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        // No signature header → verify returns false → FORBIDDEN
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // --- WebhookReceiver::router() integration tests ---

    #[tokio::test]
    async fn signed_post_reaches_the_channel_broadcast() {
        use crate::gateway::channel::ChannelState;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let secret = "router-secret";
        let state = ChannelState::new(16);
        state.set_status(ChannelStatus::Connected).await;
        // Subscribe FIRST: InboundMessageSender::send returns Err when there are
        // no subscribers (broadcast semantics), and in production the subscriber
        // is ChannelRegistry::start_message_forwarder.
        let mut rx = state.inbound_subscribe();

        let handler = Arc::new(MockWebhookHandler {
            secret: secret.to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let app = WebhookReceiver::router(vec![WebhookMount {
            handler,
            inbound: state.sender(),
            status: state.status_handle(),
            channel_id: ChannelId::new("mock-channel"),
        }]);

        let body = br#"{"text":"hello from webhook"}"#.to_vec();
        let sig = WebhookReceiver::compute_signature(secret, &body);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/mock")
                    .header("x-webhook-signature", sig)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let msg = rx
            .try_recv()
            .expect("message must reach the channel broadcast");
        assert_eq!(msg.text, "hello from webhook");
    }

    #[tokio::test]
    async fn unsigned_post_is_rejected_and_publishes_nothing() {
        use crate::gateway::channel::ChannelState;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = ChannelState::new(16);
        state.set_status(ChannelStatus::Connected).await;
        let mut rx = state.inbound_subscribe();

        let handler = Arc::new(MockWebhookHandler {
            secret: "router-secret".to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let app = WebhookReceiver::router(vec![WebhookMount {
            handler,
            inbound: state.sender(),
            status: state.status_handle(),
            channel_id: ChannelId::new("mock-channel"),
        }]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/mock")
                    .header("x-webhook-signature", "sha256=deadbeef")
                    .body(Body::from(br#"{"text":"forged"}"#.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            rx.try_recv().is_err(),
            "rejected request must publish nothing"
        );
    }

    #[tokio::test]
    async fn disconnected_channel_returns_503_and_publishes_nothing() {
        use crate::gateway::channel::ChannelState;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let secret = "router-secret";
        // A fresh ChannelState defaults to Disconnected — mirrors the state
        // left behind by `WebhookChannel::stop()` / `channel.delete`, which
        // this test exists to prove the router now honors.
        let state = ChannelState::new(16);
        let mut rx = state.inbound_subscribe();

        let handler = Arc::new(MockWebhookHandler {
            secret: secret.to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let app = WebhookReceiver::router(vec![WebhookMount {
            handler,
            inbound: state.sender(),
            status: state.status_handle(),
            channel_id: ChannelId::new("mock-channel"),
        }]);

        let body = br#"{"text":"hello from webhook"}"#.to_vec();
        let sig = WebhookReceiver::compute_signature(secret, &body);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/mock")
                    .header("x-webhook-signature", sig)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            rx.try_recv().is_err(),
            "a stopped channel must publish nothing, even with a valid signature"
        );
    }

    // --- Reserved-path guard tests ---

    #[tokio::test]
    async fn reserved_path_is_skipped_not_panicked() {
        use crate::gateway::channel::ChannelState;

        let state = ChannelState::new(16);
        let handler = Arc::new(MockWebhookHandler {
            secret: "s".to_string(),
            handler_path: "/ws".to_string(),
        });

        // Must not panic, and must produce a router with no /ws route of its own
        // (merging one into the gateway router is what would panic at boot).
        let router = WebhookReceiver::router(vec![WebhookMount {
            handler,
            inbound: state.sender(),
            status: state.status_handle(),
            channel_id: ChannelId::new("ws-channel"),
        }]);

        // A router with zero routes 404s everything.
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ws")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn path_missing_leading_slash_is_skipped_not_panicked() {
        use crate::gateway::channel::ChannelState;

        let state = ChannelState::new(16);
        let handler = Arc::new(MockWebhookHandler {
            secret: "s".to_string(),
            handler_path: "webhook/no-slash".to_string(),
        });

        // Must not panic — Router::route panics on a path without a leading
        // '/'. The sole current producer (WebhookChannelConfig::validate) is
        // guarded elsewhere; this is the builder defending itself against a
        // future WebhookHandler that isn't.
        let router = WebhookReceiver::router(vec![WebhookMount {
            handler,
            inbound: state.sender(),
            status: state.status_handle(),
            channel_id: ChannelId::new("no-slash-channel"),
        }]);

        // A router with zero routes 404s everything.
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/no-slash")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn duplicate_webhook_paths_are_deduped() {
        use crate::gateway::channel::ChannelState;

        let state_a = ChannelState::new(16);
        let state_b = ChannelState::new(16);
        state_a.set_status(ChannelStatus::Connected).await;
        state_b.set_status(ChannelStatus::Connected).await;
        let mut rx_a = state_a.inbound_subscribe();
        let mut rx_b = state_b.inbound_subscribe();

        let mounts = vec![
            WebhookMount {
                handler: Arc::new(MockWebhookHandler {
                    secret: "s".to_string(),
                    handler_path: "/webhook/dup".to_string(),
                }),
                inbound: state_a.sender(),
                status: state_a.status_handle(),
                channel_id: ChannelId::new("channel-a"),
            },
            WebhookMount {
                handler: Arc::new(MockWebhookHandler {
                    secret: "s".to_string(),
                    handler_path: "/webhook/dup".to_string(),
                }),
                inbound: state_b.sender(),
                status: state_b.status_handle(),
                channel_id: ChannelId::new("channel-b"),
            },
        ];

        // Must not panic on the duplicate route.
        let router = WebhookReceiver::router(mounts);

        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let body = br#"{"text":"dup"}"#.to_vec();
        let sig = WebhookReceiver::compute_signature("s", &body);
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/dup")
                    .header("x-webhook-signature", sig)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        // First mount wins; the second was skipped.
        assert!(rx_a.try_recv().is_ok(), "first mount must be the live one");
        assert!(
            rx_b.try_recv().is_err(),
            "second mount must have been skipped"
        );
    }

    #[test]
    fn reserved_route_matches_prefix_segments_only() {
        use crate::gateway::server::is_reserved_route;

        assert!(is_reserved_route("/ws"));
        assert!(is_reserved_route("/health"));
        assert!(is_reserved_route("/v1/chat/completions"));
        assert!(is_reserved_route("/a2a/stream"));
        assert!(is_reserved_route("/artifact/x/y/z"));
        assert!(is_reserved_route("/.well-known/agent-card.json"));

        // A path that merely starts with the same letters is NOT reserved.
        assert!(!is_reserved_route("/wsx"));
        assert!(!is_reserved_route("/healthcheck"));
        assert!(!is_reserved_route("/webhook/generic"));
    }

    // --- WebhookMountTable admission rules ---

    fn mount_for(channel: &str, path: &str, secret: &str, state: &ChannelState) -> WebhookMount {
        WebhookMount {
            handler: Arc::new(MockWebhookHandler {
                secret: secret.to_string(),
                handler_path: path.to_string(),
            }),
            inbound: state.sender(),
            status: state.status_handle(),
            channel_id: ChannelId::new(channel),
        }
    }

    #[tokio::test]
    async fn mount_accepts_a_path_under_the_shared_prefix() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);
        let table = WebhookMountTable::new();

        assert!(
            table
                .mount(mount_for("a", "/webhook/one", "s", &state))
                .await
        );
        assert_eq!(table.mounted_count().await, 1);
        assert!(table.lookup("/webhook/one").await.is_some());
        // Exact match only — a sibling path is not a hit.
        assert!(table.lookup("/webhook/one/").await.is_none());
    }

    #[tokio::test]
    async fn mount_refuses_paths_outside_the_shared_prefix() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);
        let table = WebhookMountTable::new();

        // Every one of these is unreachable behind `/webhook/{*rest}`, so
        // accepting it would re-create the advertised-but-disabled shape this
        // work exists to remove: channel Connected, endpoint deaf.
        for path in [
            "/settings",        // would have shadowed a Panel SPA path
            "/",                // ditto, the SPA root
            "webhook/no-slash", // missing leading '/'
            "/webhook",         // no sub-path — `{*rest}` matches nothing
            "/webhook/",        // ditto
            "/webhookx/sneaky", // prefix look-alike, not a segment match
        ] {
            assert!(
                !table.mount(mount_for("a", path, "s", &state)).await,
                "path {path} must be refused"
            );
        }
        assert_eq!(table.mounted_count().await, 0);
    }

    /// Which handler is live at `path`, observed exactly the way production
    /// observes it: only the live secret verifies. Comparing `Arc` identity
    /// would not catch a table that kept the right *slot* with the wrong
    /// secret, which is the failure that matters.
    async fn live_secret_is(table: &WebhookMountTable, path: &str, secret: &str) -> bool {
        let live = table.lookup(path).await.expect("must still be mounted");
        let body = b"probe".as_slice();
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Webhook-Signature",
            WebhookReceiver::compute_signature(secret, body)
                .parse()
                .unwrap(),
        );
        live.handler.verify(&headers, body)
    }

    #[tokio::test]
    async fn duplicate_path_keeps_the_smaller_channel_id() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);

        // Incumbent is the smaller id → newcomer is refused.
        let table = WebhookMountTable::new();
        assert!(
            table
                .mount(mount_for("aaa", "/webhook/dup", "first", &state))
                .await
        );
        assert!(
            !table
                .mount(mount_for("zzz", "/webhook/dup", "second", &state))
                .await
        );
        assert!(
            live_secret_is(&table, "/webhook/dup", "first").await,
            "incumbent must survive"
        );

        // Incumbent is the larger id → newcomer evicts it. Same outcome
        // whichever order the registry happens to start channels in, which is
        // the whole point (D5): route ownership must not be a per-boot coin flip.
        let table = WebhookMountTable::new();
        assert!(
            table
                .mount(mount_for("zzz", "/webhook/dup", "first", &state))
                .await
        );
        assert!(
            table
                .mount(mount_for("aaa", "/webhook/dup", "second", &state))
                .await
        );
        assert!(
            live_secret_is(&table, "/webhook/dup", "second").await,
            "the lower channel id must win regardless of arrival order"
        );
    }

    #[tokio::test]
    async fn remounting_the_same_channel_refreshes_the_handler() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);
        let table = WebhookMountTable::new();

        // `restart_channel` builds a fresh handler; keeping the old clone is
        // exactly the staleness this table exists to prevent (spec §2.E).
        assert!(
            table
                .mount(mount_for("a", "/webhook/one", "old", &state))
                .await
        );
        assert!(
            table
                .mount(mount_for("a", "/webhook/one", "new", &state))
                .await
        );
        assert_eq!(table.mounted_count().await, 1);
        assert!(live_secret_is(&table, "/webhook/one", "new").await);
    }

    #[tokio::test]
    async fn unmount_channel_removes_every_path_that_channel_owns() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);
        let table = WebhookMountTable::new();

        table
            .mount(mount_for("a", "/webhook/one", "s", &state))
            .await;
        table
            .mount(mount_for("a", "/webhook/two", "s", &state))
            .await;
        table
            .mount(mount_for("b", "/webhook/three", "s", &state))
            .await;

        assert_eq!(table.unmount_channel(&ChannelId::new("a")).await, 2);
        assert!(table.lookup("/webhook/one").await.is_none());
        assert!(table.lookup("/webhook/two").await.is_none());
        // A sibling channel's route must not be collateral damage.
        assert!(table.lookup("/webhook/three").await.is_some());

        // Unmounting an unknown channel is a no-op, not an error.
        assert_eq!(table.unmount_channel(&ChannelId::new("nope")).await, 0);
    }
}
