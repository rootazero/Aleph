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
//! - **Feishu**: Feishu/Lark Bot WebSocket + REST API integration
//! - **LINE**: LINE Messaging API webhook + REST API integration
//! - **WeChat**: WeChat iLink Bot API integration

pub mod cli;
pub mod plugin;
pub mod wechat;

#[cfg(target_os = "macos")]
pub mod imessage;

pub mod discord;
pub mod email;
pub mod feishu;
pub mod irc;
pub mod line;
pub mod matrix;
pub mod mattermost;
pub mod msteams;
pub mod nostr;
pub mod qq;
pub mod signal;
pub mod slack;
pub mod telegram;
pub mod webhook;
pub mod whatsapp;
pub mod xmpp;

pub use cli::{CliChannel, CliChannelConfig, CliChannelFactory};

#[cfg(target_os = "macos")]
pub use imessage::{
    IMessageChannel, IMessageChannelFactory, IMessageConfig, IMessageTarget, MessageSender,
    MessagesDb,
};

pub use discord::{DiscordChannel, DiscordChannelFactory, DiscordConfig};
pub use email::{EmailChannel, EmailChannelFactory, EmailConfig};
pub use feishu::{FeishuChannel, FeishuConfig};
pub use irc::{IrcChannel, IrcChannelFactory, IrcConfig};
pub use line::{LineChannel, LineChannelFactory, LineConfig};
pub use matrix::{MatrixChannel, MatrixChannelFactory, MatrixConfig};
pub use mattermost::{MattermostChannel, MattermostChannelFactory, MattermostConfig};
pub use msteams::{MsTeamsChannel, MsTeamsConfig};
pub use nostr::{NostrChannel, NostrChannelFactory, NostrConfig};
pub use qq::{QQChannel, QQChannelFactory, QQConfig, QQDmPolicy, QQGroupPolicy};
pub use signal::{SignalChannel, SignalChannelFactory, SignalConfig};
pub use slack::{SlackChannel, SlackChannelFactory, SlackConfig};
pub use telegram::{TelegramChannel, TelegramChannelFactory, TelegramConfig};
pub use webhook::{WebhookChannel, WebhookChannelConfig, WebhookChannelFactory};
pub use wechat::{WeChatChannel, WeChatChannelFactory, WeChatConfig};
pub use whatsapp::{WhatsAppChannel, WhatsAppChannelFactory, WhatsAppConfig};
pub use xmpp::{XmppChannel, XmppChannelFactory, XmppConfig};

pub fn register_channel_plugins() {
    line::register_with_plugin();
    telegram::register_with_plugin();
    wechat::register_with_plugin();
    qq::register_with_plugin();
}
