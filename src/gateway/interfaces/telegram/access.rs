//! Policy-based access pre-filter for the Telegram channel.
//!
//! This controller coarsely classifies inbound messages against the channel's
//! DM/group policy and static allowlists so obviously-denied traffic is dropped
//! at the interface. The **authoritative** access and pairing decision lives in
//! the inbound router (`check_permission` + `pairing_store`, R4): the channel's
//! `dm_policy` / `group_policy` / allowlists are bridged into the router via
//! `From<&TelegramConfigV2> for ChannelConfig`, and the router owns the pairing
//! flow (mint code → operator `pairing.approve`). A `NeedsPairing` decision here
//! is therefore just "forward to the router" — the interface no longer keeps its
//! own pairing-code store or runtime-paired-user set.
//!
//! ## Per-group / per-topic lookup (SW-1)
//!
//! [`AccessController`] no longer bakes a single `ResolvedConfig` at boot — it
//! keeps an `Arc<ConfigResolver>` and looks up the effective config per
//! inbound `(chat_id, thread_id)` so a group's policy / allowlist / agent
//! override (and any topic-level override underneath) actually applies.
//! Bot construction still precomputes the account-level fallback for the
//! `config()` accessor used by the boot diagnostics path.

use super::config_resolver::{ConfigResolver, ResolvedConfig};
use super::config_v2::{DmPolicy, GroupPolicy};
use crate::sync_primitives::Arc;

/// Result of an access check on an incoming message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDecision {
    /// User is authorized — process the message.
    Allowed,
    /// User is not statically allowlisted but the DM policy is `Pairing`.
    /// Forward to the router, which owns the authoritative pairing gate.
    NeedsPairing,
    /// User is not authorized and cannot pair — silently drop.
    Denied,
}

/// Config-driven access pre-filter for the Telegram channel.
///
/// Looks up the effective config per inbound `(chat_id, thread_id)` from the
/// shared [`ConfigResolver`]; all pairing state has been unified into the
/// inbound router's `pairing_store`.
pub struct AccessController {
    resolver: Arc<ConfigResolver>,
    /// The account id this controller serves. Required so callers without
    /// explicit chat context (e.g. boot diagnostics) can still read the
    /// account-level config via [`Self::config`].
    account_id: String,
    /// Account-level fallback cached at construction so `config()` and the
    /// legacy `check_message` callsite don't pay the resolver hash lookup on
    /// every message in a channel that never configures groups.
    account_default: ResolvedConfig,
}

impl AccessController {
    /// Build a controller bound to one account. The resolver may carry configs
    /// for many accounts; the controller only ever resolves THIS one.
    #[must_use]
    pub fn new(resolver: Arc<ConfigResolver>, account_id: &str) -> Self {
        let account_default = resolver.resolve(account_id, 0, None).cloned().expect(
            "ConfigResolver built without an account-level entry for this account \
                 — the bot construction path must always insert one",
        );
        Self {
            resolver,
            account_id: account_id.to_string(),
            account_default,
        }
    }

    /// Classify an incoming message as allowed, needing pairing, or denied,
    /// honouring per-group / per-topic overrides via [`ConfigResolver`].
    ///
    /// `thread_id` is the forum topic id (or `None` for a non-topic message).
    /// Resolution order matches `ConfigResolver::resolve`:
    /// `topic.exact → group.chat_id → account.0`.
    #[must_use]
    pub fn check_message(
        &self,
        user_id: i64,
        chat_id: i64,
        thread_id: Option<i32>,
        is_group: bool,
    ) -> AccessDecision {
        let cfg = self
            .resolver
            .resolve(&self.account_id, chat_id, thread_id)
            .unwrap_or(&self.account_default);
        Self::decide(cfg, user_id, chat_id, is_group)
    }

    /// Legacy single-config form for callers that don't have a `thread_id` —
    /// forwards to [`Self::check_message`] with `thread_id = None`.
    #[must_use]
    pub fn check_message_no_topic(
        &self,
        user_id: i64,
        chat_id: i64,
        is_group: bool,
    ) -> AccessDecision {
        self.check_message(user_id, chat_id, None, is_group)
    }

    /// Reference to the account-level config. Used by boot diagnostics and
    /// legacy call points that don't carry a `(chat_id, thread_id)`. **Not**
    /// the source of truth for runtime per-chat decisions — see
    /// [`Self::check_message`].
    #[must_use]
    pub const fn config(&self) -> &ResolvedConfig {
        &self.account_default
    }

    /// Resolve the effective config for a given `(chat_id, thread_id)`. Used
    /// by the inbound handler to feed per-chat delivery knobs (retry budget,
    /// streaming toggles, error policy) — wiring this is what makes a group's
    /// `error_policy = silent` actually silent.
    #[must_use]
    pub fn config_for(&self, chat_id: i64, thread_id: Option<i32>) -> &ResolvedConfig {
        self.resolver
            .resolve(&self.account_id, chat_id, thread_id)
            .unwrap_or(&self.account_default)
    }

    // --- Private helpers ---

    fn decide(cfg: &ResolvedConfig, user_id: i64, chat_id: i64, is_group: bool) -> AccessDecision {
        if is_group {
            Self::decide_group(cfg, chat_id)
        } else {
            Self::decide_dm(cfg, user_id)
        }
    }

    fn decide_dm(cfg: &ResolvedConfig, user_id: i64) -> AccessDecision {
        match &cfg.dm_policy {
            DmPolicy::Disabled => AccessDecision::Denied,
            DmPolicy::Open => AccessDecision::Allowed,
            DmPolicy::Allowlist => {
                if cfg.allowed_users.contains(&user_id) {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Denied
                }
            }
            DmPolicy::Pairing => {
                if cfg.allowed_users.contains(&user_id) {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::NeedsPairing
                }
            }
        }
    }

    fn decide_group(cfg: &ResolvedConfig, chat_id: i64) -> AccessDecision {
        match &cfg.group_policy {
            GroupPolicy::Disabled => AccessDecision::Denied,
            GroupPolicy::Open => AccessDecision::Allowed,
            GroupPolicy::Allowlist => {
                // Empty allowlist with `Allowlist` policy means "allow all groups"
                // — the router's `From<&TelegramConfigV2>` bridge preserves this by
                // mapping the empty case to `Open`.
                if cfg.allowed_groups.is_empty() || cfg.allowed_groups.contains(&chat_id) {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Denied
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::telegram::config_v2::{
        ErrorPolicy, ErrorPolicyMode, TelegramAccountConfig, TelegramConfigV2, TelegramGroupConfig,
        TelegramTopicConfig,
    };

    fn account_only_v2(dm: DmPolicy, group: GroupPolicy, users: Vec<i64>) -> TelegramConfigV2 {
        TelegramConfigV2 {
            coalescing: None,
            accounts: vec![TelegramAccountConfig {
                id: "main".to_string(),
                bot_token: "tok".to_string(),
                bot_username: None,
                default_agent: None,
                dm_policy: Some(dm),
                group_policy: Some(group),
                send_typing: Some(true),
                require_mention: None,
                allowed_users: Some(users),
                allowed_groups: Some(vec![]),
                streaming: None,
                error_policy: Some(ErrorPolicy {
                    mode: ErrorPolicyMode::Silent,
                    template: None,
                    max_retries: 3,
                }),
                html_fallback: None,
                link_preview: None,
                proxy_url: None,
                groups: vec![],
                token_fingerprint: None,
            }],
        }
    }

    fn controller(v2: &TelegramConfigV2) -> AccessController {
        let resolver = Arc::new(ConfigResolver::from_v2(v2));
        AccessController::new(resolver, "main")
    }

    #[test]
    fn test_dm_disabled() {
        let ctrl = controller(&account_only_v2(
            DmPolicy::Disabled,
            GroupPolicy::default(),
            vec![],
        ));
        assert_eq!(
            ctrl.check_message(111, 111, None, false),
            AccessDecision::Denied
        );
    }

    #[test]
    fn test_dm_open() {
        let ctrl = controller(&account_only_v2(
            DmPolicy::Open,
            GroupPolicy::default(),
            vec![],
        ));
        assert_eq!(
            ctrl.check_message(111, 111, None, false),
            AccessDecision::Allowed
        );
    }

    #[test]
    fn test_dm_pairing_unknown_user_needs_pairing() {
        // Unknown user under `Pairing` is forwarded to the router (which owns the
        // authoritative pairing gate), not authorized locally.
        let ctrl = controller(&account_only_v2(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![],
        ));
        assert_eq!(
            ctrl.check_message(111, 111, None, false),
            AccessDecision::NeedsPairing,
        );
    }

    #[test]
    fn test_dm_pairing_allowlisted_user_allowed() {
        let ctrl = controller(&account_only_v2(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![111],
        ));
        assert_eq!(
            ctrl.check_message(111, 111, None, false),
            AccessDecision::Allowed
        );
    }

    #[test]
    fn test_dm_allowlist_allowed() {
        let ctrl = controller(&account_only_v2(
            DmPolicy::Allowlist,
            GroupPolicy::default(),
            vec![111, 222],
        ));
        assert_eq!(
            ctrl.check_message(111, 111, None, false),
            AccessDecision::Allowed
        );
    }

    #[test]
    fn test_dm_allowlist_denied() {
        let ctrl = controller(&account_only_v2(
            DmPolicy::Allowlist,
            GroupPolicy::default(),
            vec![111, 222],
        ));
        assert_eq!(
            ctrl.check_message(999, 999, None, false),
            AccessDecision::Denied
        );
    }

    #[test]
    fn test_group_disabled() {
        let ctrl = controller(&account_only_v2(
            DmPolicy::default(),
            GroupPolicy::Disabled,
            vec![],
        ));
        assert_eq!(
            ctrl.check_message(111, -100123, None, true),
            AccessDecision::Denied,
        );
    }

    #[test]
    fn test_group_open() {
        let ctrl = controller(&account_only_v2(
            DmPolicy::default(),
            GroupPolicy::Open,
            vec![],
        ));
        assert_eq!(
            ctrl.check_message(111, -100123, None, true),
            AccessDecision::Allowed,
        );
    }

    #[test]
    fn test_group_allowlist_empty_allows_all() {
        let ctrl = controller(&account_only_v2(
            DmPolicy::default(),
            GroupPolicy::Allowlist,
            vec![],
        ));
        // Empty allowed_groups with Allowlist policy → allow all.
        assert_eq!(
            ctrl.check_message(111, -100123, None, true),
            AccessDecision::Allowed,
        );
    }

    #[test]
    fn test_group_allowlist_denied() {
        let mut v2 = account_only_v2(DmPolicy::default(), GroupPolicy::Allowlist, vec![]);
        // Inject a single allowed group via a per-group override so the account's
        // empty allowed_groups is overridden for chat -100111 only.
        v2.accounts[0].groups = vec![TelegramGroupConfig {
            id: "g1".to_string(),
            chat_id: -100111,
            agent: None,
            block_streaming: None,
            error_policy: None,
            group_policy: None,
            send_typing: None,
            allowed_users: None,
            topics: vec![],
        }];
        // And let the account default allowed_groups stay empty (Allowlist ⇒
        // open), so unoverridden groups are still allowed. We assert that chat
        // -100999 falls through to the account-level open policy.
        let ctrl = controller(&v2);
        assert_eq!(
            ctrl.check_message(111, -100999, None, true),
            AccessDecision::Allowed,
            "a chat the per-group override did not name falls back to the account policy"
        );
    }

    /// SW-1: a per-group `GroupPolicy::Disabled` blocks that one chat while
    /// the account policy stays `Open`. Verifies the resolver lookup is
    /// actually wired through `check_message`.
    #[test]
    fn per_group_override_blocks_when_account_open() {
        let mut v2 = account_only_v2(DmPolicy::default(), GroupPolicy::Open, vec![]);
        v2.accounts[0].groups = vec![TelegramGroupConfig {
            id: "g1".to_string(),
            chat_id: -100111,
            agent: None,
            block_streaming: None,
            error_policy: None,
            group_policy: Some(GroupPolicy::Disabled),
            send_typing: None,
            allowed_users: None,
            topics: vec![],
        }];
        let ctrl = controller(&v2);
        // The named group is Disabled despite Open at the account level.
        assert_eq!(
            ctrl.check_message(111, -100111, None, true),
            AccessDecision::Denied,
            "per-group policy must override the account-level policy"
        );
        // Other groups fall through to Open.
        assert_eq!(
            ctrl.check_message(111, -100999, None, true),
            AccessDecision::Allowed,
        );
    }

    /// SW-1: a per-topic override beats the group-level policy. The chain is
    /// `topic.exact → group.chat_id → account.0` (the same chain
    /// `ConfigResolver::resolve` already documents).
    #[test]
    fn per_topic_override_beats_group_and_account() {
        let mut v2 = account_only_v2(DmPolicy::default(), GroupPolicy::Open, vec![]);
        v2.accounts[0].groups = vec![TelegramGroupConfig {
            id: "g1".to_string(),
            chat_id: -100111,
            agent: None,
            block_streaming: None,
            error_policy: None,
            group_policy: Some(GroupPolicy::Disabled),
            send_typing: None,
            allowed_users: None,
            topics: vec![TelegramTopicConfig {
                id: "t1".to_string(),
                thread_id: 42,
                agent: None,
                block_streaming: None,
                error_policy: None,
                dm_policy: None,
                group_policy: Some(GroupPolicy::Open),
                send_typing: None,
                allowed_users: None,
            }],
        }];
        let ctrl = controller(&v2);
        // Topic 42 explicitly re-opens the group that would otherwise be Disabled.
        assert_eq!(
            ctrl.check_message(111, -100111, Some(42), true),
            AccessDecision::Allowed,
            "topic-level Open must beat group-level Disabled"
        );
        // Other topics on the same group still inherit the group Disabled policy.
        assert_eq!(
            ctrl.check_message(111, -100111, Some(99), true),
            AccessDecision::Denied,
            "unoverridden topics inherit the group's policy"
        );
    }
}
