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
pub use feishu::{FeishuChannel, FeishuChannelFactory, FeishuConfig};
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
    register_plain_channel!("feishu", FeishuChannelFactory);
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
    use aleph_protocol::channels::{CONFIGURABLE_CHANNEL_TYPES, FACTORY_TABLE_BYPASS};

    /// The factory table is exactly the configurable set, minus the bypass.
    ///
    /// This replaced a hand-spelled list of fifteen names on 2026-08-18. The
    /// old one asserted only one direction — "each of these fifteen is
    /// registered" — and its own doc admitted it could not catch a future
    /// adapter that forgot to register. It could not, and it did not: `feishu`
    /// shipped a complete adapter with no factory struct, so the back-fill
    /// sweep (which enumerated `impl ChannelFactory`) never saw it, this
    /// tripwire never mentioned it, and `[channels.feishu]` was inert for four
    /// months while the Panel rendered a full Feishu settings card.
    ///
    /// Equality is the point. A subset assertion is structurally blind to the
    /// thing that actually went wrong — something missing from *both* the
    /// registration and the list reads as a pass.
    #[test]
    fn the_factory_table_matches_the_configurable_channel_set() {
        super::register_channel_plugins();
        let registered = super::plugin::channel_types();

        let mut expected: Vec<&str> = CONFIGURABLE_CHANNEL_TYPES
            .iter()
            .copied()
            .filter(|t| !FACTORY_TABLE_BYPASS.contains(t))
            .collect();
        expected.sort_unstable();

        let missing: Vec<&str> = expected
            .iter()
            .copied()
            .filter(|t| !registered.contains(t))
            .collect();
        assert!(
            missing.is_empty(),
            "channel type(s) {missing:?} are advertised as configurable but have no entry in \
             the factory table, so `[channels.<type>]` resolves to None and boot logs one \
             `Failed to create channel` line — exactly the state feishu shipped in",
        );

        let unlisted: Vec<&str> = registered
            .iter()
            .copied()
            .filter(|t| !expected.contains(t))
            .collect();
        assert!(
            unlisted.is_empty(),
            "channel type(s) {unlisted:?} are registered but absent from \
             aleph_protocol::channels::CONFIGURABLE_CHANNEL_TYPES — the Panel reconciles \
             its cards against that list, so an unlisted type can never grow a settings card",
        );
    }

    /// The feishu entry actually builds a channel, not just resolves a name.
    ///
    /// `the_factory_table_matches_the_configurable_channel_set` asserts a key
    /// is in a `HashMap` — that is the producer side, and a table entry whose
    /// `create` rejects every config is byte-identical to a working one from
    /// where that test stands. This walks the path `initialize_channels`
    /// actually walks: resolve the factory, hand it a `[channels.feishu]` body,
    /// get a live `Channel` back. Nothing here touches the network — `start()`
    /// is what dials Lark, and this stops one step short of it.
    #[tokio::test]
    async fn the_feishu_factory_builds_a_channel_from_a_config_block() {
        use crate::gateway::channel::ChannelConfig;

        super::register_channel_plugins();

        // The two fields `FeishuConfig::validate` requires; everything else has
        // a serde default, and the default `connection_mode` is not `webhook`,
        // so no verification_token/encrypt_key is needed.
        let body = serde_json::json!({
            "app_id": "cli_qa_app_id",
            "app_secret": "qa_app_secret",
        });

        let factory = super::plugin::create(
            "feishu",
            ChannelConfig {
                id: "feishu".into(),
                channel_type: "feishu".into(),
                enabled: true,
                config: body.clone(),
            },
        )
        .expect("feishu is registered, so the table must hand back a factory");
        assert_eq!(factory.channel_type(), "feishu");

        let channel = factory
            .create(body)
            .await
            .expect("a minimally valid [channels.feishu] block must produce a channel");
        assert_eq!(channel.info().channel_type, "feishu");
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
