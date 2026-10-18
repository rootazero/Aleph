//! Re-export shim — `ToolSourceType` lives in `aleph-slot-api`.
//!
//! The 6-variant simplified enum (`Native` / `Builtin` / `Mcp` /
//! `Skill` / `Custom` / `Plugin`), the `From<&ToolSource>` impl,
//! the `default_icon()` and `badge_label()` const methods, and
//! their associated tests all moved to
//! `aleph_slot_api::metadata::ToolSourceType` as of P0 Task 2.
//! This file re-exports it so any
//! `crate::tool_metadata::types::tool_info::ToolSourceType` path
//! still compiles.

pub use aleph_slot_api::metadata::ToolSourceType;
