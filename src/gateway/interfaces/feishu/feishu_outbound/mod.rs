//! Feishu outbound surface.
//!
//! After the 2026-08-17 severed-wire audit, only the streaming card emitter
//! remains — that is what `inbound_router/executor.rs::try_create_feishu_emitter`
//! constructs to send progressive card updates to a feishu conversation.
//!
//! The deleted submodules (sender, media, reactions) only served the
//! un-constructed `FeishuChannel`; `feishu_inbound`, `feishu_policy`,
//! `feishu_runtime` and the top-level `message_ops`/`types` were never wired.

pub mod streaming;