//! Re-export shim — `ToolSafetyLevel` lives in `aleph-slot-api`.
//!
//! The four-variant enum (`ReadOnly` / `Reversible` /
//! `IrreversibleLowRisk` / `IrreversibleHighRisk`) with
//! `Default = ReadOnly` moved to
//! `aleph_slot_api::metadata::ToolSafetyLevel` as of P0 Task 2.
//! This file re-exports it so any
//! `crate::tool_metadata::types::safety::ToolSafetyLevel` path
//! still compiles.

pub use aleph_slot_api::metadata::ToolSafetyLevel;
