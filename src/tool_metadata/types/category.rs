//! Re-export shim — `ToolCategory` lives in `aleph-slot-api`.
//!
//! The four-variant enum (`Builtin` / `Skills` / `Mcp` / `Custom`),
//! the `serde(rename_all = "lowercase")` tag, the `display_name()`
//! / `icon()` inherent methods, and the `Display` impl all moved to
//! `aleph_slot_api::metadata::ToolCategory` as of P0 Task 2. This
//! file re-exports it so any `crate::tool_metadata::types::category::ToolCategory`
//! path still compiles.

pub use aleph_slot_api::metadata::ToolCategory;
