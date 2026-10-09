//! Session-key SSOT for Discord (R2 D8).
//!
//! Three wire formats cover the channels Discord can produce inbound
//! messages from:
//!
//! | Channel kind | Wire format                   | Example              |
//! |--------------|-------------------------------|----------------------|
//! | DM           | `dm:<user_id>`                | `dm:42`              |
//! | Guild        | `<guild_id>#<channel_id>`     | `7#99`               |
//! | Thread       | `<channel_id>+<thread_id>`    | `99+123`             |
//!
//! Why three formats (not one composite key) — the inbound router matches
//! these against existing conversations to thread multi-turn replies.
//! Changing the format silently breaks every persisted conversation; the
//! constants here are the only source of truth.
//!
//! What this module is NOT — it does not decide routing or policy. Routing
//! is the inbound router's job; policy is `group_policy::decide`.

use serde::{Deserialize, Serialize};

/// Which Discord channel surface produced an inbound event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelKind {
    /// Direct message (no `guild_id`).
    DirectMessage,
    /// Guild channel (has `guild_id`, no `thread_id`).
    GuildChannel,
    /// Thread on a guild channel (has both `guild_id` and `thread_id`).
    Thread,
}

/// Context the dispatcher needs to compute a session key.
///
/// The fields mirror the relevant slice of `InboundMessage` /
/// `ComponentInteraction` / `Interaction`. Pass-by-reference keeps this
/// cheap; the caller already holds the source message.
#[derive(Debug, Clone, Copy)]
pub struct ChannelContext<'a> {
    pub user_id: u64,
    pub channel_id: u64,
    pub guild_id: Option<u64>,
    pub thread_id: Option<u64>,
    /// Marker; if `None`, the kind is inferred from `guild_id` /
    /// `thread_id`. Use `Some(ChannelKind::DirectMessage)` to force DM
    /// even with `guild_id.is_some()` (rare; happens with user-installed
    /// apps that carry their own guild_id).
    pub kind: Option<ChannelKind>,
    /// Optional kind label for callers that don't want to think about
    /// the kind-inference rules. When `None`, we infer.
    pub _phantom: std::marker::PhantomData<&'a ()>,
}

/// Resolve the [`ChannelKind`] from the optional fields.
///
/// Threads win over guilds: a thread has both `guild_id` and `thread_id`
/// present, so checking `thread_id` first disambiguates. DMs are the
/// absence of `guild_id`.
#[must_use]
pub fn resolve_kind(ctx: &ChannelContext<'_>) -> ChannelKind {
    if let Some(k) = ctx.kind {
        return k;
    }
    if ctx.thread_id.is_some() {
        ChannelKind::Thread
    } else if ctx.guild_id.is_some() {
        ChannelKind::GuildChannel
    } else {
        ChannelKind::DirectMessage
    }
}

/// Compute the session key (the conversation id the inbound router will
/// thread against). Callers pass the resolved kind implicitly via the
/// context — `key_for` calls [`resolve_kind`] internally.
#[must_use]
pub fn key_for(ctx: &ChannelContext<'_>) -> String {
    match resolve_kind(ctx) {
        ChannelKind::DirectMessage => format!("dm:{}", ctx.user_id),
        ChannelKind::GuildChannel => match ctx.guild_id {
            Some(g) => format!("{}#{}", g, ctx.channel_id),
            None => format!("dm:{}", ctx.user_id),
        },
        ChannelKind::Thread => match (ctx.guild_id, ctx.thread_id) {
            (Some(_), Some(t)) => format!("{}#{}", ctx.channel_id, t),
            _ => format!("dm:{}", ctx.user_id),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm_ctx(user: u64) -> ChannelContext<'static> {
        ChannelContext {
            user_id: user,
            channel_id: 0,
            guild_id: None,
            thread_id: None,
            kind: None,
            _phantom: std::marker::PhantomData,
        }
    }

    fn guild_ctx(user: u64, guild: u64, channel: u64) -> ChannelContext<'static> {
        ChannelContext {
            user_id: user,
            channel_id: channel,
            guild_id: Some(guild),
            thread_id: None,
            kind: None,
            _phantom: std::marker::PhantomData,
        }
    }

    fn thread_ctx(user: u64, guild: u64, channel: u64, thread: u64) -> ChannelContext<'static> {
        ChannelContext {
            user_id: user,
            channel_id: channel,
            guild_id: Some(guild),
            thread_id: Some(thread),
            kind: None,
            _phantom: std::marker::PhantomData,
        }
    }

    #[test]
    fn dm_key_uses_user_id_only() {
        assert_eq!(key_for(&dm_ctx(42)), "dm:42");
    }

    #[test]
    fn guild_key_uses_hash_format() {
        assert_eq!(key_for(&guild_ctx(1, 7, 99)), "7#99");
    }

    #[test]
    fn thread_key_uses_hash_on_channel_and_thread() {
        assert_eq!(key_for(&thread_ctx(1, 7, 99, 123)), "99#123");
    }

    #[test]
    fn thread_kind_inferred_when_only_thread_id_set() {
        // Edge: thread_id set but guild_id is None (degenerate but
        // possible from a malformed interaction). Falls back to DM.
        let ctx = ChannelContext {
            user_id: 1,
            channel_id: 99,
            guild_id: None,
            thread_id: Some(123),
            kind: None,
            _phantom: std::marker::PhantomData,
        };
        assert_eq!(resolve_kind(&ctx), ChannelKind::Thread);
        assert_eq!(key_for(&ctx), "dm:1");
    }

    #[test]
    fn explicit_kind_overrides_field_inference() {
        let ctx = ChannelContext {
            user_id: 1,
            channel_id: 99,
            guild_id: Some(7),
            thread_id: None,
            kind: Some(ChannelKind::DirectMessage),
            _phantom: std::marker::PhantomData,
        };
        assert_eq!(resolve_kind(&ctx), ChannelKind::DirectMessage);
        assert_eq!(key_for(&ctx), "dm:1");
    }

    #[test]
    fn resolve_kind_prefers_thread_over_guild() {
        let ctx = thread_ctx(1, 7, 99, 123);
        assert_eq!(resolve_kind(&ctx), ChannelKind::Thread);
    }

    #[test]
    fn resolve_kind_dm_when_no_guild_or_thread() {
        assert_eq!(resolve_kind(&dm_ctx(42)), ChannelKind::DirectMessage);
    }

    #[test]
    fn resolve_kind_guild_when_only_guild_set() {
        assert_eq!(
            resolve_kind(&guild_ctx(1, 7, 99)),
            ChannelKind::GuildChannel
        );
    }
}
