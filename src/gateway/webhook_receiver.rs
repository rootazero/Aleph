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
use tracing::{info, warn};

use super::channel::{ChannelResult, InboundMessage, InboundMessageSender};

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
    /// not take the daemon down at boot.
    #[must_use]
    pub fn router(mounts: Vec<WebhookMount>) -> Router {
        let mut router = Router::new();
        let mut mounted: Vec<String> = Vec::new();

        for mount in mounts {
            let path = mount.handler.path().to_string();

            if crate::gateway::server::is_reserved_route(&path) {
                warn!(
                    path = %path,
                    "webhook path collides with a gateway route — handler not mounted"
                );
                continue;
            }
            if mounted.iter().any(|p| p == &path) {
                warn!(path = %path, "duplicate webhook path — handler not mounted");
                continue;
            }

            let handler_state = Arc::new(HandlerState {
                handler: mount.handler,
                inbound: mount.inbound,
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
}

/// Axum endpoint handler that dispatches to the appropriate `WebhookHandler`.
async fn webhook_endpoint(
    State(state): State<Arc<HandlerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
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
    use crate::gateway::channel::{ChannelError, ChannelId, ConversationId, MessageId, UserId};
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
        let mut rx = channel_state.inbound_subscribe();

        let handler_state = Arc::new(HandlerState {
            handler: Arc::clone(&handler),
            inbound: channel_state.sender(),
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

        let handler_state = Arc::new(HandlerState {
            handler: Arc::clone(&handler),
            inbound: channel_state.sender(),
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

        let handler_state = Arc::new(HandlerState {
            handler: Arc::clone(&handler),
            inbound: channel_state.sender(),
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
        let mut rx = state.inbound_subscribe();

        let handler = Arc::new(MockWebhookHandler {
            secret: "router-secret".to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let app = WebhookReceiver::router(vec![WebhookMount {
            handler,
            inbound: state.sender(),
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
}
