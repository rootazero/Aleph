//! Feishu (Lark) channel — thin facade.
//!
//! The full inbound + runtime + message-ops stack was severed in the
//! 2026-08-17 audit: no `FeishuChannel` is constructed by `register_channel_plugins`
//! and the whole pipeline (runtime + webhook + DmPolicyEngine + group policies +
//! MessageOps + FeishuSender) had zero production callers.
//!
//! What remains is the surface actually used by the inbound router to spin up
//! a per-message `FeishuEventEmitter`:
//! - [`FeishuApi`] — low-level API client (`api`).
//! - [`TokenManager`] — credential cache (`auth`).
//! - [`FeishuConfig`] — typed config (`config`).
//! - `feishu_outbound::streaming::FeishuEventEmitter` — the streaming card
//!   emitter constructed by `inbound_router/executor.rs::try_create_feishu_emitter`.
//!
//! Re-introducing the rest is a deliberate product decision, not a wire fix.

pub(crate) mod api;
pub(crate) mod auth;
pub mod config;
pub mod feishu_outbound;

pub use api::FeishuApi;
pub use auth::TokenManager;
pub use config::FeishuConfig;