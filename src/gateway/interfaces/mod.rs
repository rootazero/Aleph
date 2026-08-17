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
//! - **`WeChat`**: `WeChat` iLink Bot API integration

pub mod cli;
pub mod plugin;
pub mod wechat;

// iMessage module compiles on all platforms: the BlueBubbles transport is pure
// HTTP and OS-agnostic. The local (chat.db + AppleScript) transport's TYPES
// compile everywhere (rusqlite is bundled/cross-platform, AppleScript is a
// subprocess), but it is only *registered* on macOS (see subsystems.rs).
pub mod imessage;

pub mod discord;
pub mod email;
pub mod feishu;
pub mod irc;
pub mod line;
pub mod matrix;
pub mod mattermost;

pub mod nostr;
pub mod qq;
pub mod signal;
pub mod slack;
pub mod telegram;
pub mod webhook;
pub mod whatsapp;
pub mod xmpp;

pub use cli::{CliChannel, CliChannelConfig, CliChannelFactory};

pub use imessage::{
    BlueBubblesChannel, BlueBubblesConfig, IMessageChannel, IMessageChannelFactory, IMessageConfig,
    IMessageTarget, MessageSender, MessagesDb,
};

pub use discord::{DiscordChannel, DiscordChannelFactory, DiscordConfig};
pub use email::{EmailChannel, EmailChannelFactory, EmailConfig};
pub use feishu::{FeishuChannel, FeishuConfig};
pub use irc::{IrcChannel, IrcChannelFactory, IrcConfig};
pub use line::{LineChannel, LineChannelFactory, LineConfig};
pub use matrix::{MatrixChannel, MatrixChannelFactory, MatrixConfig};
pub use mattermost::{MattermostChannel, MattermostChannelFactory, MattermostConfig};

pub use nostr::{NostrChannel, NostrChannelFactory, NostrConfig};
pub use qq::{QQChannel, QQChannelFactory, QQConfig, QQDmPolicy, QQGroupPolicy};
pub use signal::{SignalChannel, SignalChannelFactory, SignalConfig};
pub use slack::{SlackChannel, SlackChannelFactory, SlackConfig};
pub use telegram::{TelegramChannel, TelegramChannelFactory, TelegramConfig};
pub use webhook::{WebhookChannel, WebhookChannelConfig, WebhookChannelFactory};
pub use wechat::{WeChatChannel, WeChatChannelFactory, WeChatConfig};
pub use whatsapp::{WhatsAppChannel, WhatsAppChannelFactory, WhatsAppConfig};
pub use xmpp::{XmppChannel, XmppChannelFactory, XmppConfig};

/// Register a plain unit-struct [`ChannelFactory`] under its config type name.
///
/// The five channels that carry extra construction wiring keep their own
/// `register_with_plugin`; everything else is this three-line shape, and ten
/// copies of it is ten places for the next one to be forgotten — which is
/// exactly how the ten below ended up unregistered.
macro_rules! register_plain_channel {
    ($name:literal, $factory:ident) => {{
        fn creator(
            _config: crate::gateway::channel::ChannelConfig,
        ) -> crate::gateway::channel::ChannelResult<
            crate::sync_primitives::Arc<dyn crate::gateway::channel::ChannelFactory>,
        > {
            Ok(crate::sync_primitives::Arc::new($factory))
        }
        let _ = plugin::register($name, creator);
    }};
}

/// Populate the channel-factory table that `create_channel_from_config` reads.
///
/// **A factory that is not registered here is unreachable**, however complete it
/// is: `handlers::channel::create_channel_from_config` resolves a configured
/// `[channels.<type>]` entry through `plugin::get_factory` and returns `None`
/// for an unknown type, after which `subsystems.rs::initialize_channels` logs
/// `Failed to create channel` and moves on.
///
/// The table landed 2026-04-05 and every channel added *after* it registered
/// itself in the same commit. The ten that predate it were never back-filled,
/// so Slack, Discord, Matrix, Mattermost, Signal, IRC, Nostr, XMPP, Email and
/// Webhook were silently unconfigurable until 2026-07-26 despite shipping full
/// adapters, configs and tests.
///
/// Deliberately absent: `imessage` (constructed directly in `initialize_channels`,
/// which `continue`s before ever consulting this table — registering it would be
/// dead code) and `cli` (not a configurable channel type).
pub fn register_channel_plugins() {
    line::register_with_plugin();
    telegram::register_with_plugin();
    wechat::register_with_plugin();
    qq::register_with_plugin();
    whatsapp::register_with_plugin();

    register_plain_channel!("discord", DiscordChannelFactory);
    register_plain_channel!("email", EmailChannelFactory);
    register_plain_channel!("irc", IrcChannelFactory);
    register_plain_channel!("matrix", MatrixChannelFactory);
    register_plain_channel!("mattermost", MattermostChannelFactory);
    register_plain_channel!("nostr", NostrChannelFactory);
    register_plain_channel!("signal", SignalChannelFactory);
    register_plain_channel!("slack", SlackChannelFactory);
    register_plain_channel!("webhook", WebhookChannelFactory);
    register_plain_channel!("xmpp", XmppChannelFactory);
}

#[cfg(test)]
mod register_tests {
    /// Every adapter that is meant to be configurable is reachable from config.
    ///
    /// This is a tripwire against REGRESSION — someone dropping a line from
    /// `register_channel_plugins`, or renaming a config type — not against a
    /// future adapter forgetting to register: nothing here can enumerate
    /// `impl ChannelFactory`, so a new name has to be added to this list by
    /// hand. That is the same manual step as the registration itself, which is
    /// why the list is spelled out rather than derived.
    #[test]
    fn every_configurable_channel_type_is_registered() {
        super::register_channel_plugins();
        let types = super::plugin::channel_types();
        for expected in [
            "discord",
            "email",
            "irc",
            "line",
            "matrix",
            "mattermost",
            "nostr",
            "qq",
            "signal",
            "slack",
            "telegram",
            "webhook",
            "wechat",
            "whatsapp",
            "xmpp",
        ] {
            assert!(
                types.contains(&expected),
                "channel type `{expected}` has a ChannelFactory but is not registered, so \
                 `[channels.{expected}]` silently does nothing",
            );
        }
    }

    /// Registration is idempotent — `register` refuses duplicates and the call
    /// site swallows that, so a second call must not blow up or drop the table.
    #[test]
    fn registering_twice_is_harmless() {
        super::register_channel_plugins();
        let before = super::plugin::channel_types().len();
        super::register_channel_plugins();
        assert_eq!(super::plugin::channel_types().len(), before);
    }
}
