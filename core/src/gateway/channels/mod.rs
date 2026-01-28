//! Channel Implementations
//!
//! This module contains concrete channel implementations for various messaging platforms.
//!
//! # Available Channels
//!
//! - **CLI**: Command-line interface channel for testing and local use
//! - (More channels to be added: iMessage, Telegram, Slack, Discord, etc.)

pub mod cli;

pub use cli::{CliChannel, CliChannelConfig, CliChannelFactory};
