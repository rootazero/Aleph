//! Unified Extensions Store JSON-RPC façade.
//!
//! `extensions.*` presents one user-facing `Extension` concept over the
//! existing MCP / plugin / skill backends. Handlers here only delegate to
//! those backends and the store cache; they never reimplement their logic.
//! See docs/superpowers/specs/2026-06-19-unified-extensions-store-design.md
pub mod catalog;
pub mod lifecycle;
pub mod sources;
