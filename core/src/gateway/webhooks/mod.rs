//! Webhook System
//!
//! HTTP-based webhook receiver for external integrations.
//!
//! # Overview
//!
//! The webhook system provides a way for external services (GitHub, Stripe, etc.)
//! to trigger agent actions via HTTP POST requests.
//!
//! # Features
//!
//! - Multiple signature verification formats (GitHub, Stripe, Generic)
//! - HMAC-SHA256 with constant-time comparison
//! - Session key templating for agent routing
//! - Async processing (returns 200 immediately)
//! - Configurable event filtering
//!
//! # Example Configuration
//!
//! ```toml
//! [webhooks]
//! enabled = true
//! port = 0  # Use gateway port
//!
//! [[webhooks.endpoints]]
//! id = "github-push"
//! path = "/webhooks/github"
//! secret = "whsec_xxx"
//! signature_format = "github"
//! agent = "main"
//! session_key_template = "task:webhook:{webhook_id}:{event_type}"
//! allowed_events = ["push", "pull_request"]
//! ```
//!
//! # Security
//!
//! All webhook endpoints should use signature verification to prevent
//! unauthorized access. The system supports:
//!
//! - **GitHub**: `X-Hub-Signature-256` header with `sha256=<hex>` format
//! - **Stripe**: `Stripe-Signature` header with `t=<timestamp>,v1=<signature>` format
//! - **Generic**: `X-Webhook-Signature` header with plain hex signature
//!
//! Signature comparison uses constant-time algorithms to prevent timing attacks.

pub mod config;
pub mod handler;
pub mod hmac;
pub mod template;

// Re-exports
pub use config::{
    ConfigValidationError, SignatureFormat, WebhookEndpointConfig, WebhooksConfig,
};
pub use handler::{
    create_router, WebhookAccepted, WebhookError, WebhookHandlerState, WebhookProcessor,
    WebhookRejected, WebhookRequest,
};
pub use hmac::{generate_signature, verify_signature, VerificationResult};
pub use template::{extract_variables, render_template, TemplateContext};
