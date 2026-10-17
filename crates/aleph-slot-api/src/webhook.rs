//! `WebhookHandler` — the channel webhook ingestion contract.
//!
//! Moved from `alephcore::gateway::webhook_receiver` in Task 4 of the P0
//! modularization; only the handler trait and the shared route prefix live
//! here. The mount table (`WebhookMountTable`), the axum receiver
//! (`WebhookReceiver`), HMAC signature computation/verification, and the
//! endpoint admission rules all stay in `alephcore` — they are HTTP-runtime
//! concerns, not contract.
//!
//! The `http::HeaderMap` / `bytes::Bytes` types in this signature are the
//! workspace's `http` 1.x / `bytes` 1.x crates — the same types axum 0.8
//! re-exports — so a handler compiled against this crate plugs into the Core
//! receiver without a conversion layer (identity pinned in `alephcore`'s
//! `tests/modularization_contracts.rs`).

use async_trait::async_trait;
use bytes::Bytes;
use http::HeaderMap;

use crate::channel::{ChannelResult, InboundMessage};

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

/// The one path prefix every channel webhook route lives under.
///
/// A single constant route (`/webhook/{*rest}`) carries all channel webhook
/// traffic, so an operator-writable `path` never enters the Core route table.
/// That is what makes the Core mount table hot-swappable *and* what removed
/// the boot-panic failure mode its reserved-prefix guard used to guard
/// against.
pub const WEBHOOK_ROUTE_PREFIX: &str = "/webhook";
