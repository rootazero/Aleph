//! Browser backend pure contract — DTOs, constants, defaults, helpers.
//!
//! This module is the canonical home for the browser vocabulary that
//! Core backends, tools, and tests already import. The Core
//! `crate::browser::types` and `crate::browser::tab_registry::TabLine`
//! paths remain valid re-export shims pointing here, so existing call
//! sites keep compiling without change.
//!
//! As of P0 Task 8 A1, this module owns the pure DTOs of `browser/types`
//! and the `TabLine` row type that the Core parser produces. Native
//! parsers, registries, runtime authority, the capability table, and
//! the `BrowserBackend` trait stay in `alephcore`; only the pure
//! vocabulary moved.
//!
//! P0 Task 8 A1 handed off the `browser/types` DTOs and the `TabLine`
//! row type; Task 8 A2 adds the canonical `Engine` declaration (with its
//! one-list macro, pure methods, `Display` and tests) and the canonical
//! `EffectVerification` verdict DTO with its pure `as_wire` rendering.
//! A later batch will add the rest of the contract
//! (`BrowserManagerContract@1`, full Operation/Reply/Feature coverage,
//! the canonical `BrowserError`, route and recording DTOs) once Main
//! verifies this intermediate source state. The engine-aware
//! `EffectVerification::for_engine` derivation stays in Core as the
//! `EffectVerificationExt` extension trait, because the measured
//! capability table it consults is native authority.
//!
//! P0 Task 8 A4 moves the canonical `BrowserError` enum (with its 27
//! variants, `StaleReason` and `Display`, and the private
//! `tab_gone_text` / `unsupported_text` formatters) and the canonical
//! `quote` page-controlled-string escaper here. Pure error/text contract
//! vocabulary; the native `engine_unavailable` /
//! `engine_unavailable_no_launcher` constructors stay in Core because
//! they call into `chromium_resolve` and need the `Engine` re-export
//! through Core's own engine module. Core `crate::browser::error`
//! publicly re-exports `BrowserError` and `StaleReason` from here, and
//! Core `crate::browser::page_state::quote` re-exports the canonical
//! `quote` at the same old path.

pub mod contract;
pub mod effect;
pub mod engine;
pub mod error;
pub mod operations;
pub mod recording;
pub mod routes;
pub mod text;
pub mod types;
pub use contract::{
    validate_browser_claim, validate_browser_reply, validate_browser_request, BrowserCapabilities,
    BrowserContractError, BrowserManagerContract, BrowserRequest,
};
pub use effect::*;
pub use engine::*;
pub use error::*;
pub use operations::*;
pub use recording::*;
pub use routes::*;
pub use text::*;
pub use types::*;
