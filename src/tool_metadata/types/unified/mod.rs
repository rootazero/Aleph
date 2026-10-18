//! Unified Tool Representation — P0 Task 2 re-export shim.
//!
//! As of P0 Task 2, the `UnifiedTool` struct (with all 25 private
//! fields), `ChannelType`, the `populate_safety_profile` algorithm
//! (unchanged), the `to_prompt_line` rendering, and every builder
//! method (`with_display_name`, `with_aliases`,
//! `with_parameters_schema`, `with_requires_confirmation`,
//! `with_safety_level`, `with_icon`, `with_usage`, `with_param_hint`,
//! `with_localization_key`, `with_sort_order`, `with_routing_regex`,
//! `with_routing_system_prompt`, `with_routing_capabilities`,
//! `with_routing_intent_type`, `with_routing_strip_prefix`,
//! `with_visible_channels`) all live in
//! `aleph_slot_api::metadata`. This module re-exports them so every
//! existing `crate::tool_metadata::types::unified::*` path keeps
//! compiling.
//!
//! The `pub(crate)` / private fields are accessed only through the
//! getter methods (`is_active`, `requires_confirmation`,
//! `safety_level`) and the declaration-only `set_active` setter,
//! which documents explicitly that it does NOT activate the
//! registry/grant.

pub use aleph_slot_api::metadata::{ChannelType, UnifiedTool};
