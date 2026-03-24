//! Dispatcher configuration types
//!
//! Contains Dispatcher Layer configuration:
//! - DispatcherConfigToml: Routing and confirmation settings
//! - AgentConfigToml: L3 Agent (multi-step planning) settings

mod core;

// Re-export all types
pub use core::*;
