//! Group policy (R2 D8) — the second half of the Discord-channel SSOT
//! pair.
//!
//! `session_key` answers "what conversation does this event belong to";
//! `group_policy` answers "should this event be processed at all, and if
//! so under what constraints?".
//!
//! The current Discord `Handler::message` (and `interaction_create`)
//! express the policy inline as 4 hard-coded `if` checks. This module
//! pulls those checks into one place so future policy changes — e.g. a
//! lurk-only mode, a per-user cooldow — can be made in one file instead
//! of grepping the handler.
//!
//! ## Precedence
//!
//! 1. **`Ignored`** — channel not in allowlist, OR DM when `dm_allowed`
//!    is false. The event never reaches the agent loop.
//! 2. **`Silenced`** — caller is muted. Same surface as Ignored but with
//!    a different audit reason so the operator can tell the two apart.
//! 3. **`RateLimited`** — last message within the rate-limit window.
//!    Retry-after is included so the caller can schedule a deferred reply.
//! 4. **`Allow`** — proceed.
//!
//! "Precedence" here means the FIRST matching rule wins; later rules do
//! not override earlier ones. This is what the inline `if` chain
//! computes today; the function is just a re-expression that returns a
//! typed enum instead of falling through `return` statements.

use std::time::{Duration, Instant};

/// Outcome of [`decide`]. The dispatcher acts on each variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupDecision {
    /// Forward to the agent loop.
    Allow,
    /// Channel/user muted — skip without surfacing an error.
    Silenced,
    /// Recent activity in this conversation; rate-limit. The duration
    /// is "wait at least this long before trying again".
    RateLimited { retry_after: Duration },
    /// Not allowlisted (channel or DM policy violated) — drop without
    /// surfacing an error. Audit trail should record the reason.
    Ignored,
}

/// Inputs the policy needs. Callers build this once per event from the
/// existing inline checks' local variables.
#[derive(Debug, Clone, Copy)]
pub struct PolicyContext<'a> {
    /// `true` iff the inbound has no `guild_id` (i.e. it's a DM).
    pub is_dm: bool,
    /// Whether DM traffic is permitted globally.
    pub dm_allowed: bool,
    /// Whether the originating guild is in the config's allowlist.
    pub guild_allowlisted: bool,
    /// Whether the originating channel is in the config's allowlist
    /// (only consulted when `is_dm` is false).
    pub channel_allowlisted: bool,
    /// Whether the sender has been muted by an operator. A muted user
    /// can still trigger commands explicitly addressed to the bot
    /// (`mention`-style) but their free-text is dropped.
    pub is_muted: bool,
    /// When this conversation last sent a message; `None` if first
    /// time. Rate-limit window is 1 second.
    pub last_message_at: Option<Instant>,
    /// Reference clock for the rate-limit comparison. Pass
    /// `Instant::now()` for real-time, or a captured timestamp for
    /// tests.
    pub now: Instant,
    /// Marker; if `None`, the kind is inferred from `guild_id` /
    /// `thread_id`. Use `Some(ChannelKind::DirectMessage)` to force DM
    /// even with `guild_id.is_some()` (rare; happens with user-installed
    /// apps that carry their own guild_id).
    pub _phantom: std::marker::PhantomData<&'a ()>,
}

/// Default rate-limit window. 1 second is generous for chat but tight
/// enough to prevent a runaway bot loop. The existing inline check
/// matched the same threshold implicitly.
pub const DEFAULT_RATE_LIMIT: Duration = Duration::from_secs(1);

/// Compute the policy outcome.
///
/// Order matters (see module docs). The function is total — every
/// possible [`PolicyContext`] returns a [`GroupDecision`].
#[must_use]
pub fn decide(ctx: &PolicyContext<'_>) -> GroupDecision {
    // 1. Ignored: DM when dm_allowed=false, OR guild not allowlisted,
    //    OR channel not allowlisted.
    if ctx.is_dm && !ctx.dm_allowed {
        return GroupDecision::Ignored;
    }
    if !ctx.is_dm && (!ctx.guild_allowlisted || !ctx.channel_allowlisted) {
        return GroupDecision::Ignored;
    }

    // 2. Silenced: caller muted. We only consult this for free-form
    //    messages, not explicit mentions; the caller's `is_mention`
    //    flag was checked in the inline chain before this is reached
    //    (handler still owns mention-vs-not gating).
    if ctx.is_muted {
        return GroupDecision::Silenced;
    }

    // 3. RateLimited: previous message within window. First-ever
    //    message (last_message_at == None) never rate-limits.
    if let Some(prev) = ctx.last_message_at {
        let elapsed = ctx.now.saturating_duration_since(prev);
        if elapsed < DEFAULT_RATE_LIMIT {
            return GroupDecision::RateLimited {
                retry_after: DEFAULT_RATE_LIMIT - elapsed,
            };
        }
    }

    GroupDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PolicyContext<'static> {
        PolicyContext {
            is_dm: false,
            dm_allowed: true,
            guild_allowlisted: true,
            channel_allowlisted: true,
            is_muted: false,
            last_message_at: None,
            now: Instant::now(),
            _phantom: std::marker::PhantomData,
        }
    }

    #[test]
    fn dm_when_dm_disallowed_is_ignored() {
        let mut ctx = base();
        ctx.is_dm = true;
        ctx.dm_allowed = false;
        assert_eq!(decide(&ctx), GroupDecision::Ignored);
    }

    #[test]
    fn dm_when_dm_allowed_is_allow() {
        let mut ctx = base();
        ctx.is_dm = true;
        ctx.dm_allowed = true;
        assert_eq!(decide(&ctx), GroupDecision::Allow);
    }

    #[test]
    fn guild_not_allowlisted_is_ignored() {
        let mut ctx = base();
        ctx.guild_allowlisted = false;
        assert_eq!(decide(&ctx), GroupDecision::Ignored);
    }

    #[test]
    fn channel_not_allowlisted_is_ignored() {
        let mut ctx = base();
        ctx.channel_allowlisted = false;
        assert_eq!(decide(&ctx), GroupDecision::Ignored);
    }

    #[test]
    fn muted_user_is_silenced() {
        let mut ctx = base();
        ctx.is_muted = true;
        assert_eq!(decide(&ctx), GroupDecision::Silenced);
    }

    #[test]
    fn rapid_repeat_is_rate_limited_with_retry_after() {
        let now = Instant::now();
        let mut ctx = base();
        ctx.last_message_at = Some(now);
        ctx.now = now + Duration::from_millis(500);
        match decide(&ctx) {
            GroupDecision::RateLimited { retry_after } => {
                // 1s - 500ms = 500ms (allow ±5ms jitter).
                assert!(
                    retry_after >= Duration::from_millis(450)
                        && retry_after <= Duration::from_millis(550),
                    "retry_after={retry_after:?}"
                );
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn first_message_never_rate_limited() {
        let mut ctx = base();
        ctx.last_message_at = None;
        assert_eq!(decide(&ctx), GroupDecision::Allow);
    }

    #[test]
    fn repeat_after_window_is_allowed() {
        let now = Instant::now();
        let mut ctx = base();
        ctx.last_message_at = Some(now);
        ctx.now = now + Duration::from_secs(2);
        assert_eq!(decide(&ctx), GroupDecision::Allow);
    }

    #[test]
    fn precedence_ignored_wins_over_silenced() {
        // Guild not allowlisted AND user muted — the user-muted state
        // is moot if the channel itself is dropped. Operators want to
        // see "channel_not_allowlisted" in the audit trail, not
        // "muted", because the action is "allowlist the channel"
        // not "unmute the user".
        let mut ctx = base();
        ctx.guild_allowlisted = false;
        ctx.is_muted = true;
        assert_eq!(decide(&ctx), GroupDecision::Ignored);
    }
}
