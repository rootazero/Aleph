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

use super::config_resolver::ResolvedConfig;
use super::config_v2::{DmPolicy, GroupPolicy};

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
/// Holds only the resolved (default-account) config; all pairing state has been
/// unified into the inbound router's `pairing_store`.
pub struct AccessController {
    resolved_config: ResolvedConfig,
}

impl AccessController {
    /// Create a new controller from the resolved channel config.
    #[must_use]
    pub const fn new(resolved_config: ResolvedConfig) -> Self {
        Self { resolved_config }
    }

    /// Classify an incoming message as allowed, needing pairing, or denied.
    #[must_use]
    pub fn check_message(&self, user_id: i64, chat_id: i64, is_group: bool) -> AccessDecision {
        if is_group {
            self.check_group(chat_id)
        } else {
            self.check_dm(user_id)
        }
    }

    /// Reference to the underlying resolved config.
    #[must_use]
    pub const fn config(&self) -> &ResolvedConfig {
        &self.resolved_config
    }

    // --- Private helpers ---

    fn check_dm(&self, user_id: i64) -> AccessDecision {
        match &self.resolved_config.dm_policy {
            DmPolicy::Disabled => AccessDecision::Denied,
            DmPolicy::Open => AccessDecision::Allowed,
            DmPolicy::Allowlist => {
                if self.is_user_allowlisted(user_id) {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Denied
                }
            }
            DmPolicy::Pairing => {
                if self.is_user_allowlisted(user_id) {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::NeedsPairing
                }
            }
        }
    }

    fn check_group(&self, chat_id: i64) -> AccessDecision {
        match &self.resolved_config.group_policy {
            GroupPolicy::Disabled => AccessDecision::Denied,
            GroupPolicy::Open => AccessDecision::Allowed,
            GroupPolicy::Allowlist => {
                // Empty allowlist with `Allowlist` policy means "allow all groups"
                // — the router's `From<&TelegramConfigV2>` bridge preserves this by
                // mapping the empty case to `Open`.
                if self.resolved_config.allowed_groups.is_empty()
                    || self.resolved_config.allowed_groups.contains(&chat_id)
                {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Denied
                }
            }
        }
    }

    /// Whether `user_id` is in the static config allowlist. Runtime-paired users
    /// are tracked by the router's `pairing_store`, not here.
    fn is_user_allowlisted(&self, user_id: i64) -> bool {
        self.resolved_config.allowed_users.contains(&user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::telegram::config_v2::{ErrorPolicy, StreamingOptions};

    fn make_config(dm: DmPolicy, group: GroupPolicy, users: Vec<i64>) -> ResolvedConfig {
        ResolvedConfig {
            account_id: "main".to_string(),
            bot_token: "123:ABC".to_string(),
            bot_username: None,
            default_agent: None,
            dm_policy: dm,
            group_policy: group,
            send_typing: true,
            max_retries: 3,
            allowed_users: users,
            allowed_groups: vec![],
            streaming: StreamingOptions::default(),
            error_policy: ErrorPolicy::default(),
            html_fallback: true,
            link_preview: crate::gateway::interfaces::telegram::config_v2::LinkPreviewMode::Enabled,
        }
    }

    #[test]
    fn test_dm_disabled() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Disabled,
            GroupPolicy::default(),
            vec![],
        ));
        assert_eq!(ctrl.check_message(111, 111, false), AccessDecision::Denied);
    }

    #[test]
    fn test_dm_open() {
        let ctrl =
            AccessController::new(make_config(DmPolicy::Open, GroupPolicy::default(), vec![]));
        assert_eq!(ctrl.check_message(111, 111, false), AccessDecision::Allowed);
    }

    #[test]
    fn test_dm_pairing_unknown_user_needs_pairing() {
        // Unknown user under `Pairing` is forwarded to the router (which owns the
        // authoritative pairing gate), not authorized locally.
        let ctrl = AccessController::new(make_config(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![],
        ));
        assert_eq!(
            ctrl.check_message(111, 111, false),
            AccessDecision::NeedsPairing,
        );
    }

    #[test]
    fn test_dm_pairing_allowlisted_user_allowed() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Pairing,
            GroupPolicy::default(),
            vec![111],
        ));
        assert_eq!(ctrl.check_message(111, 111, false), AccessDecision::Allowed);
    }

    #[test]
    fn test_dm_allowlist_allowed() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Allowlist,
            GroupPolicy::default(),
            vec![111, 222],
        ));
        assert_eq!(ctrl.check_message(111, 111, false), AccessDecision::Allowed);
    }

    #[test]
    fn test_dm_allowlist_denied() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::Allowlist,
            GroupPolicy::default(),
            vec![111, 222],
        ));
        assert_eq!(ctrl.check_message(999, 999, false), AccessDecision::Denied);
    }

    #[test]
    fn test_group_disabled() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::default(),
            GroupPolicy::Disabled,
            vec![],
        ));
        assert_eq!(
            ctrl.check_message(111, -100123, true),
            AccessDecision::Denied,
        );
    }

    #[test]
    fn test_group_open() {
        let ctrl =
            AccessController::new(make_config(DmPolicy::default(), GroupPolicy::Open, vec![]));
        assert_eq!(
            ctrl.check_message(111, -100123, true),
            AccessDecision::Allowed,
        );
    }

    #[test]
    fn test_group_allowlist_empty_allows_all() {
        let ctrl = AccessController::new(make_config(
            DmPolicy::default(),
            GroupPolicy::Allowlist,
            vec![],
        ));
        // Empty allowed_groups with Allowlist policy → allow all.
        assert_eq!(
            ctrl.check_message(111, -100123, true),
            AccessDecision::Allowed,
        );
    }

    #[test]
    fn test_group_allowlist_denied() {
        let mut config = make_config(DmPolicy::default(), GroupPolicy::Allowlist, vec![]);
        config.allowed_groups = vec![-100111];
        let ctrl = AccessController::new(config);
        assert_eq!(
            ctrl.check_message(111, -100999, true),
            AccessDecision::Denied,
        );
    }
}
