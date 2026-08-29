//! Pair Loop Guard
//!
//! Prevents bot-to-bot reply storms across any channel adapter.
//!
//! When two bot identities are allowed to message each other (e.g. a Telegram
//! bot reachable from a Slack bot bridge), they can fall into an infinite
//! "reply to reply" loop that burns LLM tokens and API quota.
//!
//! The guard tracks directed pairs `(scope, conversation, sender → receiver)`
//! over a sliding window. When the pair exceeds `max_events_per_window` events
//! inside `window_seconds`, the pair is suppressed for `cooldown_seconds`.
//!
//! Defaults match the openclaw kernel: 20 events / 60s window / 60s cooldown.
//!
//! This guard is a *scaffold* (R10): it is invoked AFTER the message is
//! identified as bot-authored (`MessageMeta::BotAuthored` on the inbound
//! envelope). Human-authored messages bypass the guard entirely.
//!
//! # Adapter opt-in surface
//!
//! Channel adapters opt in by pushing `MessageMeta::BotAuthored` onto the
//! inbound message's `metadata` vec when the upstream platform marks the
//! sender as a bot. Self-loops (messages from the adapter's *own* bot) are
//! always hard-filtered at the adapter and never reach the guard — only
//! third-party (foreign) bots flow through with the marker so the guard can
//! suppress sustained bot↔bot storms.
//!
//! Current status:
//!
//! - **Telegram polling** (`interfaces/telegram/handlers.rs::convert_message`):
//!   opted in via teloxide `User.is_bot`.
//! - **Telegram webhook** (`interfaces/telegram/webhook.rs`): the local
//!   `TelegramUser` struct carries `#[serde(default)] is_bot: bool` and
//!   `process_message` pushes `BotAuthored` when set. The file is currently
//!   kept out of the compile tree (pre-existing bit-rot unrelated to this
//!   guard, e.g. removed `TelegramMessage.sticker` field); once the
//!   webhook handler is repaired and re-declared in `telegram/mod.rs`,
//!   pair-loop-guard opt-in is already in place.
//! - **Slack** (`interfaces/slack/message_ops/converter.rs`): self-loop
//!   `user_id == bot_user_id` still hard-filters. Foreign bots (`bot_id`
//!   present and not self) now flow through with `BotAuthored` instead of
//!   being dropped.
//! - **Discord** (`interfaces/discord/mod.rs::message`): self-loop
//!   `msg.author.id == bot_self_id` (cached from `ready()`) still
//!   hard-filters. Foreign bots (`msg.author.bot && !is_self`) now flow
//!   through with `BotAuthored`.
//! - **Other adapters** (Matrix / Feishu / `WhatsApp` / IRC / etc.): not opted
//!   in yet. Each can be enabled by pushing `MessageMeta::BotAuthored` in
//!   its converter when the upstream payload marks the sender as a bot.
//!
//! Until an adapter opts in, the guard is dormant for that channel. Opt-in
//! changes Slack/Discord behavior slightly: foreign-bot messages that were
//! previously dropped silently now reach the router (and are suppressed by
//! the guard if they exceed 20 events / 60 s by default). Self-loop
//! semantics are unchanged.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::sync_primitives::Mutex;

use serde::{Deserialize, Serialize};

const DEFAULT_MAX_EVENTS_PER_WINDOW: u32 = 20;
const DEFAULT_WINDOW_SECONDS: u64 = 60;
const DEFAULT_COOLDOWN_SECONDS: u64 = 60;
/// Periodic prune cadence to bound memory under long-running daemons.
const PRUNE_INTERVAL: Duration = Duration::from_secs(60);
/// Hard cap on tracked pairs as a defensive backstop.
const MAX_TRACKED_PAIRS: usize = 10_000;

const fn default_enabled() -> bool {
    true
}
const fn default_max_events_per_window() -> u32 {
    DEFAULT_MAX_EVENTS_PER_WINDOW
}
const fn default_window_seconds() -> u64 {
    DEFAULT_WINDOW_SECONDS
}
const fn default_cooldown_seconds() -> u64 {
    DEFAULT_COOLDOWN_SECONDS
}

/// Per-channel (or default) pair-loop protection configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairLoopGuardConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_events_per_window")]
    pub max_events_per_window: u32,
    #[serde(default = "default_window_seconds")]
    pub window_seconds: u64,
    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u64,
}

impl Default for PairLoopGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_events_per_window: DEFAULT_MAX_EVENTS_PER_WINDOW,
            window_seconds: DEFAULT_WINDOW_SECONDS,
            cooldown_seconds: DEFAULT_COOLDOWN_SECONDS,
        }
    }
}

impl PairLoopGuardConfig {
    fn window(&self) -> Duration {
        Duration::from_secs(self.window_seconds.max(1))
    }
    const fn cooldown(&self) -> Duration {
        Duration::from_secs(self.cooldown_seconds)
    }
}

/// Identifying facts about a directed bot→bot exchange.
#[derive(Debug, Clone)]
pub struct PairLoopFacts<'a> {
    pub scope_id: &'a str,
    pub conversation_id: &'a str,
    pub sender_id: &'a str,
    pub receiver_id: &'a str,
}

/// Outcome of a single `record_and_check` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairLoopGuardResult {
    pub suppressed: bool,
    pub reason: Option<String>,
    pub retry_after: Option<Duration>,
}

impl PairLoopGuardResult {
    const fn allowed() -> Self {
        Self {
            suppressed: false,
            reason: None,
            retry_after: None,
        }
    }

    fn suppressed(reason: impl Into<String>, retry_after: Duration) -> Self {
        Self {
            suppressed: true,
            reason: Some(reason.into()),
            retry_after: Some(retry_after),
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct PairKey {
    scope_id: String,
    conversation_id: String,
    sender_id: String,
    receiver_id: String,
}

#[derive(Debug)]
struct PairState {
    events: VecDeque<Instant>,
    cooldown_until: Option<Instant>,
}

impl PairState {
    const fn new() -> Self {
        Self {
            events: VecDeque::new(),
            cooldown_until: None,
        }
    }
}

/// Snapshot of one tracked pair (for diagnostics / tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairLoopGuardSnapshot {
    pub scope_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub receiver_id: String,
    pub events_in_window: usize,
    pub cooling_down: bool,
}

/// Pair-loop guard kernel. Cheap to clone (internal `Mutex`).
#[derive(Debug, Default)]
pub struct PairLoopGuard {
    state: Mutex<GuardState>,
}

#[derive(Debug, Default)]
struct GuardState {
    pairs: HashMap<PairKey, PairState>,
    last_prune: Option<Instant>,
}

impl PairLoopGuard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an event for the pair and report whether it must be suppressed.
    ///
    /// `now` is exposed for deterministic testing; pass `Instant::now()` in
    /// production via [`Self::record_and_check`].
    pub fn record_and_check_at(
        &self,
        facts: &PairLoopFacts<'_>,
        config: &PairLoopGuardConfig,
        now: Instant,
    ) -> PairLoopGuardResult {
        if !config.enabled {
            return PairLoopGuardResult::allowed();
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        Self::maybe_prune(&mut state, config, now);

        let key = PairKey {
            scope_id: facts.scope_id.to_string(),
            conversation_id: facts.conversation_id.to_string(),
            sender_id: facts.sender_id.to_string(),
            receiver_id: facts.receiver_id.to_string(),
        };

        let pair = state.pairs.entry(key).or_insert_with(PairState::new);

        if let Some(until) = pair.cooldown_until {
            if now < until {
                let retry = until.saturating_duration_since(now);
                return PairLoopGuardResult::suppressed(
                    "bot pair in cooldown after exceeding loop-protection budget",
                    retry,
                );
            }
            pair.cooldown_until = None;
            pair.events.clear();
        }

        let window = config.window();
        while let Some(front) = pair.events.front().copied() {
            if now.saturating_duration_since(front) > window {
                pair.events.pop_front();
            } else {
                break;
            }
        }

        pair.events.push_back(now);

        if pair.events.len() as u32 > config.max_events_per_window {
            let cooldown = config.cooldown();
            pair.cooldown_until = Some(now + cooldown);
            return PairLoopGuardResult::suppressed(
                "bot pair exceeded loop-protection budget",
                cooldown,
            );
        }

        PairLoopGuardResult::allowed()
    }

    /// Production entry point: record and check using wall-clock `Instant::now()`.
    pub fn record_and_check(
        &self,
        facts: &PairLoopFacts<'_>,
        config: &PairLoopGuardConfig,
    ) -> PairLoopGuardResult {
        self.record_and_check_at(facts, config, Instant::now())
    }

    /// Snapshot tracked pairs (used by diagnostics / tests).
    pub fn snapshot(&self) -> Vec<PairLoopGuardSnapshot> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .pairs
            .iter()
            .map(|(k, v)| PairLoopGuardSnapshot {
                scope_id: k.scope_id.clone(),
                conversation_id: k.conversation_id.clone(),
                sender_id: k.sender_id.clone(),
                receiver_id: k.receiver_id.clone(),
                events_in_window: v.events.len(),
                cooling_down: v.cooldown_until.is_some(),
            })
            .collect()
    }

    /// Test-only: drop all tracked state.
    pub fn clear_for_tests(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.pairs.clear();
        state.last_prune = None;
    }

    fn maybe_prune(state: &mut GuardState, config: &PairLoopGuardConfig, now: Instant) {
        let due = state
            .last_prune
            .is_none_or(|t| now.saturating_duration_since(t) >= PRUNE_INTERVAL);
        if !due && state.pairs.len() <= MAX_TRACKED_PAIRS {
            return;
        }
        state.last_prune = Some(now);

        let window = config.window();
        state.pairs.retain(|_, pair| {
            if let Some(until) = pair.cooldown_until {
                if now < until {
                    return true;
                }
                pair.cooldown_until = None;
                pair.events.clear();
            }
            while let Some(front) = pair.events.front().copied() {
                if now.saturating_duration_since(front) > window {
                    pair.events.pop_front();
                } else {
                    break;
                }
            }
            !pair.events.is_empty()
        });

        if state.pairs.len() > MAX_TRACKED_PAIRS {
            let drain = state.pairs.len() - MAX_TRACKED_PAIRS / 2;
            let keys: Vec<PairKey> = state.pairs.keys().take(drain).cloned().collect();
            for k in keys {
                state.pairs.remove(&k);
            }
        }
    }
}

/// Resolve effective settings from channel config, account/global defaults,
/// and the `default_enabled` policy. Mirrors the openclaw kernel.
///
/// ⚠️ SEVERED AS OF 2026-08-29 — this function has **zero production
/// callers**. Its only call sites are in this file's own `#[cfg(test)]`
/// module, and `InboundMessageRouter::with_bot_loop_protection` (the one
/// writer of the router's `pair_loop_config`) is likewise called only from
/// `inbound_router`'s test module. Production therefore runs on
/// `PairLoopGuardConfig::default()`, which is `enabled: true` with the
/// `DEFAULT_*` budget — an ACTIVE, hard-coded 20-per-60s silent drop
/// (`pair_loop_check` logs one `warn!` and returns false; nobody is told).
/// Since telegram/discord/slack all opt in to `MessageMeta::BotAuthored`, a
/// bridged notification bot bursting 21 messages in a minute is suppressed
/// and the operator has no configuration that can widen or disable it: the
/// only key with that name, `ChannelAccessConfig::bot_loop_protection`, is
/// reachable solely from `[channels.whatsapp].access` and has no reader.
/// (`channel_policy.rs` still documents that field as falling back "via
/// [`resolve_pair_loop_settings`]"; that is a promise nothing keeps.)
///
/// This is a CONNECT, not a CUT: "how do I widen or disable bot-loop
/// protection" has no other answer in the repo, so deleting the merge would
/// leave a fail-closed default with no door handle at all. Wiring it needs a
/// decision that cannot be made in this file, because the shapes disagree —
/// the router holds ONE `pair_loop_config` while `bot_loop_protection` is
/// declared per-channel. Either (a) add a global config block and resolve it
/// once at boot, or (b) give the router a `HashMap<channel_id, _>` and have
/// `pair_loop_check` look up by `msg.channel_id`. Do not ship a per-channel
/// key that silently collapses to a global — that is a second answer to the
/// same question, which is how this ended up severed.
#[must_use]
pub fn resolve_pair_loop_settings(
    channel: Option<&PairLoopGuardConfig>,
    defaults: Option<&PairLoopGuardConfig>,
    default_enabled: bool,
) -> PairLoopGuardConfig {
    let base = PairLoopGuardConfig {
        enabled: default_enabled,
        ..PairLoopGuardConfig::default()
    };
    let merged_defaults = defaults.cloned().map_or(base, |d| PairLoopGuardConfig {
        enabled: d.enabled,
        max_events_per_window: d.max_events_per_window.max(1),
        window_seconds: d.window_seconds.max(1),
        cooldown_seconds: d.cooldown_seconds,
    });
    match channel.cloned() {
        Some(c) => PairLoopGuardConfig {
            enabled: c.enabled,
            max_events_per_window: c.max_events_per_window.max(1),
            window_seconds: c.window_seconds.max(1),
            cooldown_seconds: c.cooldown_seconds,
        },
        None => merged_defaults,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(
        scope: &'a str,
        conv: &'a str,
        sender: &'a str,
        recv: &'a str,
    ) -> PairLoopFacts<'a> {
        PairLoopFacts {
            scope_id: scope,
            conversation_id: conv,
            sender_id: sender,
            receiver_id: recv,
        }
    }

    fn small_config() -> PairLoopGuardConfig {
        PairLoopGuardConfig {
            enabled: true,
            max_events_per_window: 3,
            window_seconds: 60,
            cooldown_seconds: 30,
        }
    }

    #[test]
    fn allows_events_under_budget() {
        let guard = PairLoopGuard::new();
        let cfg = small_config();
        let f = facts("tg", "conv", "botA", "botB");
        let now = Instant::now();
        for i in 0..3 {
            let r = guard.record_and_check_at(&f, &cfg, now + Duration::from_millis(i * 10));
            assert!(!r.suppressed, "event {i} should be allowed");
        }
    }

    #[test]
    fn suppresses_once_budget_exceeded() {
        let guard = PairLoopGuard::new();
        let cfg = small_config();
        let f = facts("tg", "conv", "botA", "botB");
        let now = Instant::now();
        for _ in 0..3 {
            let _ = guard.record_and_check_at(&f, &cfg, now);
        }
        let r = guard.record_and_check_at(&f, &cfg, now + Duration::from_millis(1));
        assert!(r.suppressed);
        assert!(r.retry_after.unwrap() >= Duration::from_secs(29));
        assert!(r.reason.unwrap().contains("budget"));
    }

    #[test]
    fn cooldown_blocks_further_events() {
        let guard = PairLoopGuard::new();
        let cfg = small_config();
        let f = facts("tg", "conv", "botA", "botB");
        let now = Instant::now();
        for _ in 0..4 {
            let _ = guard.record_and_check_at(&f, &cfg, now);
        }
        let still_cooling = guard.record_and_check_at(&f, &cfg, now + Duration::from_secs(5));
        assert!(still_cooling.suppressed);
        assert!(still_cooling.reason.unwrap().contains("cooldown"));
    }

    #[test]
    fn cooldown_expires_and_pair_resumes() {
        let guard = PairLoopGuard::new();
        let cfg = small_config();
        let f = facts("tg", "conv", "botA", "botB");
        let now = Instant::now();
        for _ in 0..4 {
            let _ = guard.record_and_check_at(&f, &cfg, now);
        }
        let resumed = guard.record_and_check_at(&f, &cfg, now + Duration::from_secs(31));
        assert!(!resumed.suppressed, "after cooldown the pair should reset");
    }

    #[test]
    fn old_events_fall_off_sliding_window() {
        let guard = PairLoopGuard::new();
        let cfg = small_config();
        let f = facts("tg", "conv", "botA", "botB");
        let t0 = Instant::now();
        for _ in 0..3 {
            let _ = guard.record_and_check_at(&f, &cfg, t0);
        }
        // 90s later, all three events fall off the 60s window
        let r = guard.record_and_check_at(&f, &cfg, t0 + Duration::from_secs(90));
        assert!(!r.suppressed);
        let snaps = guard.snapshot();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].events_in_window, 1);
    }

    #[test]
    fn directed_pairs_are_independent() {
        let guard = PairLoopGuard::new();
        let cfg = small_config();
        let now = Instant::now();
        let a_to_b = facts("tg", "conv", "botA", "botB");
        let b_to_a = facts("tg", "conv", "botB", "botA");
        for _ in 0..4 {
            let _ = guard.record_and_check_at(&a_to_b, &cfg, now);
        }
        let r = guard.record_and_check_at(&b_to_a, &cfg, now);
        assert!(
            !r.suppressed,
            "reverse direction should track its own counters"
        );
    }

    #[test]
    fn disabled_config_short_circuits() {
        let guard = PairLoopGuard::new();
        let cfg = PairLoopGuardConfig {
            enabled: false,
            ..PairLoopGuardConfig::default()
        };
        let f = facts("tg", "conv", "botA", "botB");
        let now = Instant::now();
        for _ in 0..100 {
            let r = guard.record_and_check_at(&f, &cfg, now);
            assert!(!r.suppressed);
        }
        assert!(
            guard.snapshot().is_empty(),
            "disabled guard must not allocate state"
        );
    }

    #[test]
    fn snapshot_reflects_pair_state() {
        let guard = PairLoopGuard::new();
        let cfg = small_config();
        let f = facts("tg", "conv", "botA", "botB");
        let now = Instant::now();
        let _ = guard.record_and_check_at(&f, &cfg, now);
        let _ = guard.record_and_check_at(&f, &cfg, now);
        let snaps = guard.snapshot();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].events_in_window, 2);
        assert!(!snaps[0].cooling_down);
    }

    #[test]
    fn resolve_settings_prefers_channel_over_defaults() {
        let ch = PairLoopGuardConfig {
            enabled: true,
            max_events_per_window: 5,
            window_seconds: 30,
            cooldown_seconds: 10,
        };
        let defaults = PairLoopGuardConfig {
            enabled: true,
            max_events_per_window: 100,
            window_seconds: 600,
            cooldown_seconds: 0,
        };
        let r = resolve_pair_loop_settings(Some(&ch), Some(&defaults), true);
        assert_eq!(r.max_events_per_window, 5);
        assert_eq!(r.window_seconds, 30);
        assert_eq!(r.cooldown_seconds, 10);
    }

    #[test]
    fn resolve_settings_falls_back_to_defaults_then_default_enabled() {
        let r = resolve_pair_loop_settings(None, None, false);
        assert!(!r.enabled);
        assert_eq!(r.max_events_per_window, DEFAULT_MAX_EVENTS_PER_WINDOW);
    }

    #[test]
    fn config_normalizes_zero_window_to_one_second() {
        let cfg = PairLoopGuardConfig {
            enabled: true,
            max_events_per_window: 3,
            window_seconds: 0,
            cooldown_seconds: 0,
        };
        assert_eq!(cfg.window(), Duration::from_secs(1));
    }

    #[test]
    fn poisoned_mutex_recovers_via_into_inner() {
        // Synthesize a poisoned lock via a panicking guard; record_and_check_at
        // must still succeed.
        let guard = std::sync::Arc::new(PairLoopGuard::new());
        let g2 = guard.clone();
        let _ = std::thread::spawn(move || {
            let _lock = g2.state.lock().unwrap();
            panic!("intentional");
        })
        .join();
        let cfg = small_config();
        let f = facts("tg", "conv", "botA", "botB");
        let r = guard.record_and_check_at(&f, &cfg, Instant::now());
        assert!(!r.suppressed);
    }
}
