//! Command handlers for Aleph Gateway CLI
//!
//! This module organizes all subcommand implementations.

pub mod audit;
pub mod bootstrap_runtime;
pub mod bootstrap_token;
pub mod devices;
pub mod gateway;
pub mod hooks;
pub mod pairing;
pub mod plugins;
pub mod sandbox_debug;
pub mod secret;
pub mod start;

// Re-export commonly used items
pub use audit::*;
pub use bootstrap_token::handle_bootstrap_token;
pub use devices::*;
pub use gateway::*;
pub use hooks::*;
pub use pairing::*;
pub use plugins::*;
pub use sandbox_debug::handle_sandbox_debug;
pub use secret::*;
pub use start::*;
