//! Tests for the Anthropic protocol adapter, grouped by functional area:
//!
//! - `helpers`       — shared `body_of` / `build_body` / `build_http` utilities.
//! - `basic`         — endpoint resolution, think-level mapping, supports_native_tools.
//! - `convert`       — `convert_messages` (text/tool/image/thinking/multi-turn).
//! - `build_request` — config wiring (sampling/service_tier/metadata/cache_control/beta).
//! - `schema`        — tool `input_schema` sanitization & dedup.
//! - `oauth`         — OAuth-token detection & authorization-header wiring.
//! - `adaptive`      — Claude 4.6/4.7 adaptive-thinking + xhigh downgrade.
//! - `prefix_stability` — strict-prefix-extension cache contract on raw bodies.

mod helpers;

mod adaptive;
mod basic;
mod build_request;
mod convert;
mod oauth;
mod prefix_stability;
mod schema;
