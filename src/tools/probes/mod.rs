//! Concrete [`ToolHealthProbe`] implementations.
//!
//! Probes live here so they remain decoupled from `tools::handlers` (which
//! owns the executor-side `ToolHandler`) and from
//! `dispatcher::registry::health` (which owns the cache + trait surface).
//! Each probe is registered with the dispatcher's `ToolHealthCache` at the
//! same lifecycle point where the corresponding tool is registered.

pub mod mcp;
