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
//! - **Signal**: Signal via signal-cli REST API integration (requires `signal` feature)
//! - **Mattermost**: Mattermost WebSocket + REST API v4 integration (requires `mattermost` feature)
//! - **IRC**: IRC raw TCP integration via RFC 2812 (requires `irc` feature)
//! - **Webhook**: Generic bidirectional HTTP webhook (requires `webhook` feature)
//! - **XMPP**: XMPP raw TCP integration via RFC 6120/6121 + XEP-0045 MUC (requires `xmpp` feature)

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

#[cfg(feature = "signal")]
pub mod signal;

#[cfg(feature = "mattermost")]
pub mod mattermost;

#[cfg(feature = "irc")]
pub mod irc;

#[cfg(feature = "webhook")]
pub mod webhook;

#[cfg(feature = "xmpp")]
pub mod xmpp;

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

#[cfg(feature = "signal")]
pub use signal::{SignalChannel, SignalChannelFactory, SignalConfig};

#[cfg(feature = "mattermost")]
pub use mattermost::{MattermostChannel, MattermostChannelFactory, MattermostConfig};

#[cfg(feature = "irc")]
pub use irc::{IrcChannel, IrcChannelFactory, IrcConfig};

#[cfg(feature = "webhook")]
pub use webhook::{WebhookChannel, WebhookChannelFactory, WebhookChannelConfig};

#[cfg(feature = "xmpp")]
pub use xmpp::{XmppChannel, XmppChannelFactory, XmppConfig};
