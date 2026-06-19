//! Command handlers for Aleph Gateway CLI
//!
//! This module organizes all subcommand implementations.

pub mod audit;
pub mod bootstrap_runtime;
pub mod bootstrap_token;
pub mod doctor;
pub mod gateway;
pub mod hooks;
pub mod node;
pub mod plugins;
pub mod prompt_size;
pub mod sandbox_debug;
pub mod secret;
pub mod start;
pub mod update;

// Re-export commonly used items
pub use audit::*;
pub use bootstrap_token::handle_bootstrap_token;
pub use doctor::handle_doctor_command;
pub use gateway::*;
pub use hooks::*;
pub use plugins::*;
pub use sandbox_debug::handle_sandbox_debug;
pub use secret::*;
pub use start::*;
pub use update::handle_update;
