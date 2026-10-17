//! `ToolHandler` implementations for builtin / MCP sources.
//!
//! The `extension` submodule was removed 2026-05-20: it was a Phase-2-era
//! placeholder for WASM plugin tools that flowed into `tools::ToolHandlerRegistry`,
//! but the boot wiring (Phase 2 Task 10 — `AppContext` plumbing) never
//! landed. Plugin tools today reach the LLM exclusively via the
//! `tool_metadata::ToolCatalog` path that `run_loop` consults. Re-introduce
//! `ExtensionHandler` only when the Gap 1 unification of Phase-2 vs
//! tool-catalog registries is settled (see CLAUDE.md memory notes).
//!
//! ## P0 Task 2: `ToolHandler` trait + `McpServerFilter` moved to `aleph-slot-api`
//!
//! The trait and the `McpServerFilter` type alias now live in
//! `aleph_slot_api::handler`. This module re-exports them so every
//! existing `crate::tools::handlers::ToolHandler` and
//! `crate::tools::handlers::McpServerFilter` path keeps compiling.
//! The five methods (`invoke`, `definition`, `concurrency_claim`,
//! `bind_visible_servers`, `fences_output`) keep the exact
//! production signatures: `concurrency_claim(&self, &Value)` and
//! `bind_visible_servers(&self, &McpServerFilter)`. `McpServerFilter`
//! is the public, pure `Arc<dyn Fn(&str) -> bool + Send + Sync>`
//! alias.

pub mod builtin;
pub mod mcp;
pub mod registration;

// Re-exports — single source of truth is `aleph_slot_api::handler`.
// Kept `pub` (not `pub(crate)`) so production handlers and
// external consumers reach the same trait / alias the rest of the
// crate uses. The five methods and the alias type are part of the
// identity surface — see `crates/aleph-slot-api/src/handler.rs`.
pub use aleph_slot_api::handler::{McpServerFilter, ToolHandler};
