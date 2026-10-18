//! Re-export shim — metadata `ToolDefinition` lives in `aleph-slot-api`.
//!
//! The six-field struct (`name` / `description` / `parameters` /
//! `requires_confirmation` / `category` / `strict`), the
//! `#[non_exhaustive]` attribute, the `new` constructor, and the
//! `with_confirmation` / `with_strict` builders all moved to
//! `aleph_slot_api::metadata::ToolDefinition` as of P0 Task 2.
//! This file re-exports it so any
//! `crate::tool_metadata::types::definition::ToolDefinition` path
//! still compiles.
//!
//! Note: this is the *metadata* `ToolDefinition` (provider-facing
//! schema form). The *loop-side* `ToolDefinition` lives at
//! `aleph_slot_api::tool::ToolDefinition` (and
//! `crate::tools::service::ToolDefinition`); the two are distinct
//! types and are kept apart on purpose — the loop-side form
//! carries source + idempotence / concurrency metadata, the
//! metadata form is the panel-rendering shape.

pub use aleph_slot_api::metadata::ToolDefinition;
