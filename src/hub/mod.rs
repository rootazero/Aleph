//! Unified Extensions Store: one user-facing `Extension` concept over the
//! existing plugin / MCP / skill backends. See
//! docs/superpowers/specs/2026-06-19-unified-extensions-store-design.md
pub mod cache;
pub mod categorize;
pub mod dedup;
pub mod hub_catalog;
pub mod install;
pub mod provider;
pub mod reconcile;
pub mod secrets;
pub mod trust;
pub mod types;
pub mod verify;

pub use types::{
    EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, HeaderDecl, InstallSpec,
    McpTransport, TrustTier,
};
