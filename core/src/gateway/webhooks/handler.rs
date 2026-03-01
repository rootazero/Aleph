//! Webhook HTTP Handler
//!
//! Axum-based HTTP handler for receiving webhook requests.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Serialize;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::config::WebhooksConfig;
use super::hmac::{verify_signature, VerificationResult};
use super::template::{render_template, TemplateContext};

/// Webhook handler state
pub struct WebhookHandlerState {
    /// Webhooks configuration
    config: Arc<RwLock<WebhooksConfig>>,
    /// Callback for processing webhook payloads
    processor: Arc<dyn WebhookProcessor>,
}

/// Trait for processing webhook payloads
#[async_trait::async_trait]
pub trait WebhookProcessor: Send + Sync {
    /// Process a webhook payload
    ///
    /// Called asynchronously after signature verification.
    /// The handler returns 200 OK immediately, and this runs in background.
    async fn process(&self, request: WebhookRequest) -> Result<(), WebhookError>;
}

/// Incoming webhook request
#[derive(Debug, Clone)]
pub struct WebhookRequest {
    /// Unique delivery ID
    pub delivery_id: String,
    /// Webhook endpoint ID
    pub webhook_id: String,
    /// Target agent ID
    pub agent_id: String,
    /// Rendered session key
    pub session_key: String,
    /// Event type (from header or body)
    pub event_type: Option<String>,
    /// Raw payload bytes
    pub payload: Vec<u8>,
    /// Parsed JSON payload (if applicable)
    pub payload_json: Option<serde_json::Value>,
    /// Extracted headers
    pub headers: std::collections::HashMap<String, String>,
    /// Timestamp
    pub timestamp: chrono::DateTime<Utc>,
}

/// Webhook processing error
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("Webhook endpoint not found: {0}")]
    NotFound(String),

    #[error("Webhook endpoint disabled: {0}")]
    Disabled(String),

    #[error("Signature verification failed: {0}")]
    SignatureFailed(String),

    #[error("Event type not allowed: {0}")]
    EventNotAllowed(String),

    #[error("Payload too large: {size} bytes (max: {max})")]
    PayloadTooLarge { size: usize, max: usize },

    #[error("Processing failed: {0}")]
    ProcessingFailed(String),
}

/// Webhook response for successful acceptance
#[derive(Debug, Serialize)]
pub struct WebhookAccepted {
    pub accepted: bool,
    pub delivery_id: String,
}

/// Webhook response for errors
#[derive(Debug, Serialize)]
pub struct WebhookRejected {
    pub accepted: bool,
    pub error: String,
}

impl WebhookHandlerState {
    /// Create new handler state
    pub fn new(config: WebhooksConfig, processor: Arc<dyn WebhookProcessor>) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            processor,
        }
    }

    /// Update configuration
    pub async fn update_config(&self, config: WebhooksConfig) {
        *self.config.write().await = config;
    }

    /// Get current configuration
    pub async fn get_config(&self) -> WebhooksConfig {
        self.config.read().await.clone()
    }
}

/// Create the webhook router
pub fn create_router(state: Arc<WebhookHandlerState>) -> Router {
    Router::new()
        .route("/webhooks/{id}", post(handle_webhook))
        .route("/webhooks/health", get(health_check))
        .route("/webhooks", get(list_webhooks))
        .with_state(state)
}

/// Handle incoming webhook
async fn handle_webhook(
    State(state): State<Arc<WebhookHandlerState>>,
    Path(webhook_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let delivery_id = Uuid::new_v4().to_string();

    debug!(
        delivery_id = %delivery_id,
        webhook_id = %webhook_id,
        body_size = body.len(),
        "Received webhook request"
    );

    // Get configuration
    let config = state.config.read().await;

    // Find endpoint
    let endpoint = match config.endpoints.iter().find(|e| e.id == webhook_id) {
        Some(e) => e.clone(),
        None => {
            warn!(webhook_id = %webhook_id, "Webhook endpoint not found");
            return (
                StatusCode::NOT_FOUND,
                Json(WebhookRejected {
                    accepted: false,
                    error: format!("Webhook '{}' not found", webhook_id),
                }),
            )
                .into_response();
        }
    };

    // Check if enabled
    if !endpoint.enabled {
        warn!(webhook_id = %webhook_id, "Webhook endpoint disabled");
        return (
            StatusCode::FORBIDDEN,
            Json(WebhookRejected {
                accepted: false,
                error: "Webhook endpoint is disabled".to_string(),
            }),
        )
            .into_response();
    }

    // Check payload size
    if body.len() > endpoint.max_body_size {
        warn!(
            webhook_id = %webhook_id,
            size = body.len(),
            max = endpoint.max_body_size,
            "Payload too large"
        );
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(WebhookRejected {
                accepted: false,
                error: format!(
                    "Payload too large: {} bytes (max: {})",
                    body.len(),
                    endpoint.max_body_size
                ),
            }),
        )
            .into_response();
    }

    // Verify signature
    let signature_header = endpoint
        .signature_format
        .header_name()
        .and_then(|name| headers.get(name))
        .and_then(|v| v.to_str().ok());

    let secret = endpoint.secret.as_deref().unwrap_or("");
    let verification = verify_signature(
        endpoint.signature_format,
        secret,
        signature_header,
        &body,
    );

    if verification.is_err() {
        let error_msg = match &verification {
            VerificationResult::Missing => "Signature header missing",
            VerificationResult::Invalid => "Invalid signature",
            VerificationResult::Malformed(msg) => msg.as_str(),
            _ => "Verification failed",
        };
        warn!(
            webhook_id = %webhook_id,
            error = error_msg,
            "Signature verification failed"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(WebhookRejected {
                accepted: false,
                error: error_msg.to_string(),
            }),
        )
            .into_response();
    }

    // Extract event type from common headers
    let event_type = extract_event_type(&headers, &body);

    // Check if event is allowed
    if let Some(ref event) = event_type {
        if !endpoint.is_event_allowed(event) {
            warn!(
                webhook_id = %webhook_id,
                event_type = %event,
                "Event type not allowed"
            );
            return (
                StatusCode::FORBIDDEN,
                Json(WebhookRejected {
                    accepted: false,
                    error: format!("Event type '{}' not allowed", event),
                }),
            )
                .into_response();
        }
    }

    // Extract headers
    let mut extracted_headers = std::collections::HashMap::new();
    for header_name in &endpoint.extract_headers {
        if let Some(value) = headers.get(header_name.as_str()) {
            if let Ok(v) = value.to_str() {
                extracted_headers.insert(header_name.clone(), v.to_string());
            }
        }
    }

    // Render session key
    let context = TemplateContext::for_webhook(
        &webhook_id,
        event_type.as_deref(),
        None, // TODO: extract source_id from payload
    );
    let session_key = render_template(&endpoint.session_key_template, &context);

    // Parse JSON payload
    let payload_json = serde_json::from_slice(&body).ok();

    // Build request
    let request = WebhookRequest {
        delivery_id: delivery_id.clone(),
        webhook_id: webhook_id.clone(),
        agent_id: endpoint.agent.clone(),
        session_key,
        event_type,
        payload: body.to_vec(),
        payload_json,
        headers: extracted_headers,
        timestamp: Utc::now(),
    };

    // Drop the config lock before processing
    drop(config);

    // Clone for async spawn
    let delivery_id_clone = delivery_id.clone();
    let webhook_id_clone = webhook_id.clone();

    // Process asynchronously
    let processor = state.processor.clone();
    tokio::spawn(async move {
        if let Err(e) = processor.process(request).await {
            error!(
                delivery_id = %delivery_id_clone,
                webhook_id = %webhook_id_clone,
                error = %e,
                "Webhook processing failed"
            );
        }
    });

    // Return 200 OK immediately
    info!(
        delivery_id = %delivery_id,
        webhook_id = %webhook_id,
        "Webhook accepted"
    );

    (
        StatusCode::OK,
        Json(WebhookAccepted {
            accepted: true,
            delivery_id,
        }),
    )
        .into_response()
}

/// Health check endpoint
async fn health_check(State(state): State<Arc<WebhookHandlerState>>) -> impl IntoResponse {
    let config = state.config.read().await;

    Json(serde_json::json!({
        "status": "ok",
        "enabled": config.enabled,
        "endpoints": config.endpoints.len(),
    }))
}

/// List configured webhooks
async fn list_webhooks(State(state): State<Arc<WebhookHandlerState>>) -> impl IntoResponse {
    let config = state.config.read().await;

    let endpoints: Vec<_> = config
        .endpoints
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "path": e.path,
                "enabled": e.enabled,
                "agent": e.agent,
                "signature_format": e.signature_format,
            })
        })
        .collect();

    Json(serde_json::json!({
        "endpoints": endpoints,
    }))
}

/// Extract event type from headers or body
fn extract_event_type(headers: &HeaderMap, body: &[u8]) -> Option<String> {
    // GitHub: X-GitHub-Event
    if let Some(event) = headers.get("X-GitHub-Event") {
        if let Ok(e) = event.to_str() {
            return Some(e.to_string());
        }
    }

    // Stripe: Look in the payload
    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(event_type) = json.get("type").and_then(|v| v.as_str()) {
            return Some(event_type.to_string());
        }
    }

    // GitLab: X-Gitlab-Event
    if let Some(event) = headers.get("X-Gitlab-Event") {
        if let Ok(e) = event.to_str() {
            return Some(e.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::config::WebhookEndpointConfig;
    use axum::http::Request;
    use tower::ServiceExt;

    struct MockProcessor;

    #[async_trait::async_trait]
    impl WebhookProcessor for MockProcessor {
        async fn process(&self, _request: WebhookRequest) -> Result<(), WebhookError> {
            Ok(())
        }
    }

    fn create_test_state() -> Arc<WebhookHandlerState> {
        let mut config = WebhooksConfig::default();
        config.endpoints.push(
            WebhookEndpointConfig::new("test", "/webhooks/test", "main")
                .with_signature_format(super::super::config::SignatureFormat::None),
        );
        Arc::new(WebhookHandlerState::new(config, Arc::new(MockProcessor)))
    }

    #[tokio::test]
    async fn test_health_check() {
        let state = create_test_state();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/webhooks/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_webhook_not_found() {
        let state = create_test_state();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhooks/nonexistent")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_webhook_accepted() {
        let state = create_test_state();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhooks/test")
                    .body(axum::body::Body::from(r#"{"event":"test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn test_extract_event_type_github() {
        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", "push".parse().unwrap());

        let event = extract_event_type(&headers, b"{}");
        assert_eq!(event, Some("push".to_string()));
    }

    #[test]
    fn test_extract_event_type_stripe() {
        let headers = HeaderMap::new();
        let body = br#"{"type":"payment.succeeded"}"#;

        let event = extract_event_type(&headers, body);
        assert_eq!(event, Some("payment.succeeded".to_string()));
    }
}
