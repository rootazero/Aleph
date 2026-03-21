//! Command handlers for Aleph Gateway CLI
//!
//! This module organizes all subcommand implementations.

pub mod pairing;
pub mod devices;
pub mod plugins;
pub mod gateway;
pub mod start;
pub mod audit;
pub mod secret;

// Re-export commonly used items
pub use pairing::*;
pub use devices::*;
pub use plugins::*;
pub use gateway::*;
pub use start::*;
pub use audit::*;
pub use secret::*;
