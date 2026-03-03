//! Interface Implementations
//!
//! This module contains concrete interface implementations for various messaging platforms.
//! Each interface represents a connection endpoint (Telegram, Discord, iMessage, CLI, etc.)
//! through which users interact with the Aleph Server.
//!
//! # Available Interfaces
//!
//! - **CLI**: Command-line interface for testing and local use
//! - **iMessage**: macOS iMessage integration (macOS only)
//! - **Telegram**: Telegram Bot API integration
//! - **Discord**: Discord Bot API integration
//! - **Slack**: Slack Socket Mode + REST API integration
//! - **Email**: IMAP + SMTP email integration
//! - **Matrix**: Matrix Client-Server API v3 integration
//! - **Signal**: Signal via signal-cli REST API integration
//! - **Mattermost**: Mattermost WebSocket + REST API v4 integration
//! - **IRC**: IRC raw TCP integration via RFC 2812
//! - **Webhook**: Generic bidirectional HTTP webhook
//! - **XMPP**: XMPP raw TCP integration via RFC 6120/6121 + XEP-0045 MUC
//! - **Nostr**: Nostr NIP-01 relay WebSocket + NIP-04 DM integration

pub mod cli;

#[cfg(target_os = "macos")]
pub mod imessage;

pub mod telegram;
pub mod discord;
pub mod whatsapp;
pub mod slack;
pub mod email;
pub mod matrix;
pub mod signal;
pub mod mattermost;
pub mod irc;
pub mod webhook;
pub mod xmpp;
pub mod nostr;

pub use cli::{CliChannel, CliChannelConfig, CliChannelFactory};

#[cfg(target_os = "macos")]
pub use imessage::{IMessageChannel, IMessageChannelFactory, IMessageConfig, IMessageTarget, MessageSender, MessagesDb};

pub use telegram::{TelegramChannel, TelegramChannelFactory, TelegramConfig};
pub use discord::{DiscordChannel, DiscordChannelFactory, DiscordConfig};
pub use whatsapp::{WhatsAppChannel, WhatsAppChannelFactory, WhatsAppConfig};
pub use slack::{SlackChannel, SlackChannelFactory, SlackConfig};
pub use email::{EmailChannel, EmailChannelFactory, EmailConfig};
pub use matrix::{MatrixChannel, MatrixChannelFactory, MatrixConfig};
pub use signal::{SignalChannel, SignalChannelFactory, SignalConfig};
pub use mattermost::{MattermostChannel, MattermostChannelFactory, MattermostConfig};
pub use irc::{IrcChannel, IrcChannelFactory, IrcConfig};
pub use webhook::{WebhookChannel, WebhookChannelFactory, WebhookChannelConfig};
pub use xmpp::{XmppChannel, XmppChannelFactory, XmppConfig};
pub use nostr::{NostrChannel, NostrChannelFactory, NostrConfig};
