//! Webhook Receiver — Shared HTTP Server for Channel Webhook Ingestion
//!
//! Provides a reusable HTTP server that social bot channels (WhatsApp, Generic Webhook, etc.)
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
use crate::sync_primitives::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use super::channel::{ChannelResult, InboundMessage};

type HmacSha256 = Hmac<Sha256>;

/// Trait for platform-specific webhook handling.
///
/// Each channel (WhatsApp, Generic Webhook, etc.) implements this trait to:
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

    /// Parse webhook payload into InboundMessages.
    ///
    /// A single webhook request may produce multiple messages (e.g., batch delivery).
    async fn handle(&self, headers: &HeaderMap, body: Bytes) -> ChannelResult<Vec<InboundMessage>>;

    /// URL path for this handler (e.g., "/webhook/whatsapp").
    ///
    /// Must start with `/` and be unique across all registered handlers.
    fn path(&self) -> &str;
}

/// Shared HTTP server for receiving channel webhooks.
///
/// Manages an axum HTTP server that routes incoming webhook requests
/// to registered `WebhookHandler` implementations.
pub struct WebhookReceiver {
    port: u16,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl WebhookReceiver {
    /// Create a new WebhookReceiver bound to the given port.
    pub fn new(port: u16) -> Self {
        Self {
            port,
            shutdown_tx: None,
        }
    }

    /// Start the webhook receiver HTTP server.
    ///
    /// Builds an axum Router from the registered handlers and spawns
    /// a Tokio task to serve requests. Incoming messages are forwarded
    /// to `inbound_tx`.
    ///
    /// # Arguments
    ///
    /// * `handlers` - List of webhook handlers to register
    /// * `inbound_tx` - Channel sender for forwarding parsed messages
    pub async fn start(
        &mut self,
        handlers: Vec<Arc<dyn WebhookHandler>>,
        inbound_tx: mpsc::Sender<InboundMessage>,
    ) -> ChannelResult<()> {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Build router from handlers
        let mut router = Router::new();

        for handler in handlers {
            let path = handler.path().to_string();
            let handler = Arc::clone(&handler);
            let tx = inbound_tx.clone();

            let handler_state = Arc::new(HandlerState {
                handler,
                inbound_tx: tx,
            });

            router = router.route(
                &path,
                post(webhook_endpoint).with_state(handler_state),
            );

            info!(path = %path, "Registered webhook handler");
        }

        let port = self.port;
        let mut shutdown_rx = shutdown_rx;

        tokio::spawn(async move {
            let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
            info!(addr = %addr, "Webhook receiver starting");

            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    warn!(port = port, error = %e, "Failed to bind webhook receiver port");
                    return;
                }
            };

            let server = axum::serve(listener, router);

            tokio::select! {
                result = server => {
                    if let Err(e) = result {
                        warn!(error = %e, "Webhook receiver server error");
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("Webhook receiver shutting down");
                }
            }

            info!("Webhook receiver stopped");
        });

        Ok(())
    }

    /// Stop the webhook receiver by sending the shutdown signal.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
    }

    /// Compute HMAC-SHA256 signature of data with the given secret.
    ///
    /// Returns the signature in the format `"sha256={hex_digest}"`.
    pub fn compute_signature(secret: &str, data: &[u8]) -> String {
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
        mac.update(data);
        let result = mac.finalize();
        let hex_str = hex::encode(result.into_bytes());
        format!("sha256={hex_str}")
    }

    /// Verify an HMAC-SHA256 signature using constant-time comparison.
    ///
    /// The `signature` parameter should be in `"sha256={hex_digest}"` format.
    /// Returns `true` if the signature matches.
    pub fn verify_signature(secret: &str, body: &[u8], signature: &str) -> bool {
        let expected = Self::compute_signature(secret, body);
        if expected.len() != signature.len() {
            return false;
        }
        // Constant-time comparison to prevent timing attacks
        expected.as_bytes().ct_eq(signature.as_bytes()).into()
    }
}

/// Internal state passed to each axum handler.
struct HandlerState {
    handler: Arc<dyn WebhookHandler>,
    inbound_tx: mpsc::Sender<InboundMessage>,
}

/// Axum endpoint handler that dispatches to the appropriate WebhookHandler.
async fn webhook_endpoint(
    State(state): State<Arc<HandlerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Step 1: Verify signature
    if !state.handler.verify(&headers, &body) {
        warn!(path = %state.handler.path(), "Webhook signature verification failed");
        return (StatusCode::FORBIDDEN, "Forbidden: invalid signature");
    }

    // Step 2: Parse payload into messages
    match state.handler.handle(&headers, body).await {
        Ok(messages) => {
            for msg in messages {
                if let Err(e) = state.inbound_tx.send(msg).await {
                    warn!(
                        path = %state.handler.path(),
                        error = %e,
                        "Failed to forward inbound message"
                    );
                }
            }
            (StatusCode::OK, "ok")
        }
        Err(e) => {
            warn!(
                path = %state.handler.path(),
                error = %e,
                "Webhook handler error"
            );
            (StatusCode::BAD_REQUEST, "Bad request")
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
        assert!(!WebhookReceiver::verify_signature("wrong-secret", body, &sig));
        // Wrong body
        assert!(!WebhookReceiver::verify_signature(secret, b"different body", &sig));
        // Completely wrong signature
        assert!(!WebhookReceiver::verify_signature(
            secret,
            body,
            "sha256=0000000000000000000000000000000000000000000000000000000000000000"
        ));
        // Truncated signature
        assert!(!WebhookReceiver::verify_signature(secret, body, "sha256=bad"));
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
            let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
                ChannelError::ReceiveFailed(format!("Invalid JSON: {e}"))
            })?;

            let text = json["message"]
                .as_str()
                .ok_or_else(|| ChannelError::ReceiveFailed("Missing 'message' field".into()))?
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
            }])
        }

        fn path(&self) -> &str {
            &self.handler_path
        }
    }

    #[tokio::test]
    async fn test_webhook_endpoint_valid_signature() {
        use axum::http::Request;
        use tower::ServiceExt;

        let secret = "integration-test-secret";
        let handler: Arc<dyn WebhookHandler> = Arc::new(MockWebhookHandler {
            secret: secret.to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let (tx, mut rx) = mpsc::channel::<InboundMessage>(16);

        let handler_state = Arc::new(HandlerState {
            handler: Arc::clone(&handler),
            inbound_tx: tx,
        });

        let app = Router::new()
            .route("/webhook/mock", post(webhook_endpoint))
            .with_state(handler_state);

        let body = r#"{"message":"Hello from webhook!"}"#;
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
        use axum::http::Request;
        use tower::ServiceExt;

        let handler: Arc<dyn WebhookHandler> = Arc::new(MockWebhookHandler {
            secret: "real-secret".to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let (tx, _rx) = mpsc::channel::<InboundMessage>(16);

        let handler_state = Arc::new(HandlerState {
            handler: Arc::clone(&handler),
            inbound_tx: tx,
        });

        let app = Router::new()
            .route("/webhook/mock", post(webhook_endpoint))
            .with_state(handler_state);

        let body = r#"{"message":"Unauthorized!"}"#;

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
        use axum::http::Request;
        use tower::ServiceExt;

        let handler: Arc<dyn WebhookHandler> = Arc::new(MockWebhookHandler {
            secret: "some-secret".to_string(),
            handler_path: "/webhook/mock".to_string(),
        });

        let (tx, _rx) = mpsc::channel::<InboundMessage>(16);

        let handler_state = Arc::new(HandlerState {
            handler: Arc::clone(&handler),
            inbound_tx: tx,
        });

        let app = Router::new()
            .route("/webhook/mock", post(webhook_endpoint))
            .with_state(handler_state);

        let body = r#"{"message":"No sig!"}"#;

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

    #[test]
    fn test_webhook_receiver_creation() {
        let receiver = WebhookReceiver::new(9090);
        assert_eq!(receiver.port, 9090);
        assert!(receiver.shutdown_tx.is_none());
    }
}
