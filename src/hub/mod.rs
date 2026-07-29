//! Unified Extensions Hub: one user-facing `Extension` concept over the
//! existing plugin / MCP / skill backends, fed by the single published Aleph
//! Hub catalog. See
//! docs/superpowers/specs/2026-06-20-aleph-hub-single-source-design.md
pub mod cache;
pub mod catalog_client;
pub mod hub_catalog;
pub mod install;
pub mod official_mcp;
pub mod official_plugins;
pub mod official_skills;
pub mod origin;
pub mod primer;
pub mod reconcile;
pub mod secrets;
pub mod trust;
pub mod types;
pub mod verify;

pub use types::{
    EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, HeaderDecl, InstallSpec,
    McpTransport, TrustTier,
};
