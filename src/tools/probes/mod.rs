//! Concrete [`ToolHealthProbe`] implementations.
//!
//! Probes live here so they remain decoupled from `tools::handlers` (which
//! owns the executor-side `ToolHandler`) and from
//! `tool_metadata::registry::health` (which owns the cache + trait surface).
//! Each probe is registered with the tool catalog's `ToolHealthCache` at the
//! same lifecycle point where the corresponding tool is registered.

pub mod browser;
pub mod generation;
pub mod mcp;
