//! Channel Implementations
//!
//! This module contains concrete channel implementations for various messaging platforms.
//!
//! # Available Channels
//!
//! - **CLI**: Command-line interface channel for testing and local use
//! - **iMessage**: macOS iMessage integration (requires Full Disk Access)
//! - **Telegram**: Telegram Bot API integration (requires `telegram` feature)
//! - **Discord**: Discord Bot API integration (requires `discord` feature)

pub mod cli;

#[cfg(target_os = "macos")]
pub mod imessage;

#[cfg(feature = "telegram")]
pub mod telegram;

#[cfg(feature = "discord")]
pub mod discord;

pub use cli::{CliChannel, CliChannelConfig, CliChannelFactory};

#[cfg(target_os = "macos")]
pub use imessage::{IMessageChannel, IMessageChannelFactory, IMessageConfig, IMessageTarget, MessageSender, MessagesDb};

#[cfg(feature = "telegram")]
pub use telegram::{TelegramChannel, TelegramChannelFactory, TelegramConfig};

#[cfg(feature = "discord")]
pub use discord::{DiscordChannel, DiscordChannelFactory, DiscordConfig};
