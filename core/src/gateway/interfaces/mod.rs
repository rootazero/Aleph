//! Interface Implementations
//!
//! This module contains concrete interface implementations for various messaging platforms.
//! Each interface represents a connection endpoint (Telegram, Discord, iMessage, CLI, etc.)
//! through which users interact with the Aleph Server.
//!
//! # Available Interfaces
//!
//! - **CLI**: Command-line interface for testing and local use
//! - **iMessage**: macOS iMessage integration (requires Full Disk Access)
//! - **Telegram**: Telegram Bot API integration (requires `telegram` feature)
//! - **Discord**: Discord Bot API integration (requires `discord` feature)
//! - **Slack**: Slack Socket Mode + REST API integration (requires `slack` feature)
//! - **Email**: IMAP + SMTP email integration (requires `email` feature)
//! - **Matrix**: Matrix Client-Server API v3 integration (requires `matrix` feature)

pub mod cli;

#[cfg(target_os = "macos")]
pub mod imessage;

#[cfg(feature = "telegram")]
pub mod telegram;

#[cfg(feature = "discord")]
pub mod discord;

#[cfg(feature = "whatsapp")]
pub mod whatsapp;

#[cfg(feature = "slack")]
pub mod slack;

#[cfg(feature = "email")]
pub mod email;

#[cfg(feature = "matrix")]
pub mod matrix;

pub use cli::{CliChannel, CliChannelConfig, CliChannelFactory};

#[cfg(target_os = "macos")]
pub use imessage::{IMessageChannel, IMessageChannelFactory, IMessageConfig, IMessageTarget, MessageSender, MessagesDb};

#[cfg(feature = "telegram")]
pub use telegram::{TelegramChannel, TelegramChannelFactory, TelegramConfig};

#[cfg(feature = "discord")]
pub use discord::{DiscordChannel, DiscordChannelFactory, DiscordConfig};

#[cfg(feature = "whatsapp")]
pub use whatsapp::{WhatsAppChannel, WhatsAppChannelFactory, WhatsAppConfig};

#[cfg(feature = "slack")]
pub use slack::{SlackChannel, SlackChannelFactory, SlackConfig};

#[cfg(feature = "email")]
pub use email::{EmailChannel, EmailChannelFactory, EmailConfig};

#[cfg(feature = "matrix")]
pub use matrix::{MatrixChannel, MatrixChannelFactory, MatrixConfig};
