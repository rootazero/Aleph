//! Unified Extensions Store: one user-facing `Extension` concept over the
//! existing plugin / MCP / skill backends. See
//! docs/superpowers/specs/2026-06-19-unified-extensions-store-design.md
pub mod cache;
pub mod reconcile;
pub mod types;

pub use types::{
    EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, HeaderDecl, InstallSpec,
    McpTransport, TrustTier,
};
