#[cfg(test)]
use super::config_v2::ErrorPolicyMode;
use super::config_v2::{
    DmPolicy, ErrorPolicy, GroupPolicy, LinkPreviewMode, StreamingMode, StreamingOptions,
    TelegramAccountConfig, TelegramConfigV2, TelegramGroupConfig, TelegramTopicConfig,
};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub account_id: String,
    pub bot_token: String,
    pub bot_username: Option<String>,
    pub default_agent: Option<String>,
    pub dm_policy: DmPolicy,
    pub group_policy: GroupPolicy,
    pub send_typing: bool,
    pub allowed_users: Vec<i64>,
    pub allowed_groups: Vec<i64>,
    pub streaming: StreamingOptions,
    pub error_policy: ErrorPolicy,
    pub max_retries: u32,
    pub html_fallback: bool,
    pub link_preview: LinkPreviewMode,
}

#[derive(Debug, Clone)]
pub struct ConfigResolver {
    lookup: HashMap<(String, i64, Option<i32>), ResolvedConfig>,
}

impl ConfigResolver {
    #[must_use]
    pub fn from_v2(config: &TelegramConfigV2) -> Self {
        let mut lookup = HashMap::new();
        for account in &config.accounts {
            let account_default = Self::resolve_account_defaults(account);
            let account_key = (account.id.clone(), 0, None);
            assert!(
                lookup
                    .insert(account_key, account_default.clone())
                    .is_none(),
                "duplicate account id: {}",
                account.id
            );
            for group in &account.groups {
                let group_resolved = Self::merge_group(&account_default, group);
                let group_key = (account.id.clone(), group.chat_id, None);
                assert!(
                    lookup.insert(group_key, group_resolved.clone()).is_none(),
                    "duplicate group chat_id {} for account {}",
                    group.chat_id,
                    account.id
                );
                for topic in &group.topics {
                    // A topic inherits the group's resolved streaming.
                    // `block_streaming: true` blocks it, `false` explicitly
                    // unblocks it back to the ACCOUNT setting, and `None`
                    // keeps the group's decision.
                    let mut topic_resolved = Self::merge_topic(&group_resolved, topic);
                    match topic.block_streaming {
                        Some(true) => topic_resolved.streaming.enabled = false,
                        Some(false) => topic_resolved.streaming = account_default.streaming.clone(),
                        None => {}
                    }
                    let topic_key = (account.id.clone(), group.chat_id, Some(topic.thread_id));
                    assert!(
                        lookup.insert(topic_key, topic_resolved.clone()).is_none(),
                        "duplicate topic thread_id {} for group {} account {}",
                        topic.thread_id,
                        group.chat_id,
                        account.id
                    );
                }
            }
        }
        Self { lookup }
    }

    #[must_use]
    pub fn resolve(
        &self,
        account_id: &str,
        chat_id: i64,
        thread_id: Option<i32>,
    ) -> Option<&ResolvedConfig> {
        self.lookup
            .get(&(account_id.to_string(), chat_id, thread_id))
            .or_else(|| self.lookup.get(&(account_id.to_string(), chat_id, None)))
            .or_else(|| self.lookup.get(&(account_id.to_string(), 0, None)))
    }

    fn resolve_account_defaults(account: &TelegramAccountConfig) -> ResolvedConfig {
        let error_policy = account.error_policy.clone().unwrap_or_default();
        let mut streaming = account.streaming.clone().unwrap_or_default();
        if matches!(streaming.mode, StreamingMode::Disabled) {
            streaming.enabled = false;
        }
        ResolvedConfig {
            account_id: account.id.clone(),
            bot_token: account.bot_token.clone(),
            bot_username: account.bot_username.clone(),
            default_agent: account.default_agent.clone(),
            dm_policy: account.dm_policy.clone().unwrap_or_default(),
            group_policy: account.group_policy.clone().unwrap_or_default(),
            send_typing: account.send_typing.unwrap_or(true),
            allowed_users: account.allowed_users.clone().unwrap_or_default(),
            allowed_groups: account.allowed_groups.clone().unwrap_or_default(),
            streaming,
            // Send-retry budget follows the configured error policy (default 3)
            // instead of a hardcoded constant.
            max_retries: error_policy.max_retries,
            error_policy,
            html_fallback: account.html_fallback.unwrap_or(true),
            link_preview: account.link_preview.unwrap_or_default(),
        }
    }

    fn merge_group(base: &ResolvedConfig, group: &TelegramGroupConfig) -> ResolvedConfig {
        let error_policy = group
            .error_policy
            .clone()
            .unwrap_or_else(|| base.error_policy.clone());
        // A group has no streaming block of its own: it inherits the account
        // configuration and may only turn streaming OFF (block_streaming).
        let mut streaming = base.streaming.clone();
        if group.block_streaming.unwrap_or(false) {
            streaming.enabled = false;
        }
        ResolvedConfig {
            account_id: base.account_id.clone(),
            bot_token: base.bot_token.clone(),
            bot_username: base.bot_username.clone(),
            default_agent: group.agent.clone().or_else(|| base.default_agent.clone()),
            dm_policy: base.dm_policy.clone(),
            group_policy: group
                .group_policy
                .clone()
                .unwrap_or_else(|| base.group_policy.clone()),
            send_typing: group.send_typing.unwrap_or(base.send_typing),
            allowed_users: group
                .allowed_users
                .clone()
                .unwrap_or_else(|| base.allowed_users.clone()),
            allowed_groups: base.allowed_groups.clone(),
            streaming,
            max_retries: error_policy.max_retries,
            error_policy,
            html_fallback: base.html_fallback,
            link_preview: base.link_preview,
        }
    }

    fn merge_topic(base: &ResolvedConfig, topic: &TelegramTopicConfig) -> ResolvedConfig {
        let error_policy = topic
            .error_policy
            .clone()
            .unwrap_or_else(|| base.error_policy.clone());
        ResolvedConfig {
            account_id: base.account_id.clone(),
            bot_token: base.bot_token.clone(),
            bot_username: base.bot_username.clone(),
            default_agent: topic.agent.clone().or_else(|| base.default_agent.clone()),
            dm_policy: topic
                .dm_policy
                .clone()
                .unwrap_or_else(|| base.dm_policy.clone()),
            group_policy: topic
                .group_policy
                .clone()
                .unwrap_or_else(|| base.group_policy.clone()),
            send_typing: topic.send_typing.unwrap_or(base.send_typing),
            allowed_users: topic
                .allowed_users
                .clone()
                .unwrap_or_else(|| base.allowed_users.clone()),
            allowed_groups: base.allowed_groups.clone(),
            streaming: base.streaming.clone(),
            max_retries: error_policy.max_retries,
            error_policy,
            html_fallback: base.html_fallback,
            link_preview: base.link_preview,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_default_inheritance() {
        let v2 = TelegramConfigV2 {
            coalescing: None,
            accounts: vec![TelegramAccountConfig {
                id: "main".to_string(),
                bot_token: "tok".to_string(),
                bot_username: None,
                default_agent: Some("default".to_string()),
                dm_policy: Some(DmPolicy::Pairing),
                group_policy: Some(GroupPolicy::Allowlist),
                send_typing: Some(false),
                require_mention: None,
                allowed_users: Some(vec![1]),
                allowed_groups: Some(vec![-1]),
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
        };
        let resolver = ConfigResolver::from_v2(&v2);
        let resolved = resolver.resolve("main", 0, None).unwrap();
        assert_eq!(resolved.default_agent, Some("default".to_string()));
        assert_eq!(resolved.error_policy.mode, ErrorPolicyMode::Silent);
        assert!(!resolved.send_typing);
    }

    #[test]
    fn block_streaming_inherits_and_topic_can_explicitly_unblock() {
        let config: TelegramConfigV2 = serde_json::from_value(serde_json::json!({
            "accounts": [{"id": "a", "bot_token": "123:abc", "groups": [{
                "id": "group", "chat_id": -100, "block_streaming": true,
                "topics": [{"id": "five", "thread_id": 5}, {"id": "six", "thread_id": 6, "block_streaming": false}]
            }]}]
        })).unwrap();
        let resolver = ConfigResolver::from_v2(&config);
        assert!(!resolver.resolve("a", -100, None).unwrap().streaming.enabled);
        assert!(
            !resolver
                .resolve("a", -100, Some(5))
                .unwrap()
                .streaming
                .enabled
        );
        assert!(
            resolver
                .resolve("a", -100, Some(6))
                .unwrap()
                .streaming
                .enabled
        );
        assert!(resolver.resolve("a", -200, None).unwrap().streaming.enabled);
    }

    #[test]
    fn test_topic_overrides_group() {
        let v2 = TelegramConfigV2 {
            coalescing: None,
            accounts: vec![TelegramAccountConfig {
                id: "main".to_string(),
                bot_token: "tok".to_string(),
                bot_username: None,
                default_agent: Some("default".to_string()),
                dm_policy: Some(DmPolicy::Pairing),
                group_policy: Some(GroupPolicy::Allowlist),
                send_typing: Some(true),
                require_mention: None,
                allowed_users: None,
                allowed_groups: None,
                streaming: None,
                error_policy: Some(ErrorPolicy {
                    mode: ErrorPolicyMode::Reply,
                    template: None,
                    max_retries: 3,
                }),
                html_fallback: None,
                link_preview: None,
                proxy_url: None,
                groups: vec![TelegramGroupConfig {
                    id: "g1".to_string(),
                    chat_id: -1001,
                    agent: Some("group_agent".to_string()),
                    block_streaming: None,
                    error_policy: Some(ErrorPolicy {
                        mode: ErrorPolicyMode::Once,
                        template: None,
                        max_retries: 3,
                    }),
                    group_policy: None,
                    send_typing: None,
                    allowed_users: None,
                    topics: vec![TelegramTopicConfig {
                        id: "t1".to_string(),
                        thread_id: 42,
                        agent: Some("topic_agent".to_string()),
                        block_streaming: None,
                        error_policy: Some(ErrorPolicy {
                            mode: ErrorPolicyMode::Silent,
                            template: None,
                            max_retries: 3,
                        }),
                        dm_policy: None,
                        group_policy: None,
                        send_typing: None,
                        allowed_users: None,
                    }],
                }],
                token_fingerprint: None,
            }],
        };
        let resolver = ConfigResolver::from_v2(&v2);
        let topic = resolver.resolve("main", -1001, Some(42)).unwrap();
        assert_eq!(topic.default_agent, Some("topic_agent".to_string()));
        assert_eq!(topic.error_policy.mode, ErrorPolicyMode::Silent);
        assert!(topic.send_typing); // inherited from account

        let group = resolver.resolve("main", -1001, None).unwrap();
        assert_eq!(group.default_agent, Some("group_agent".to_string()));
        assert_eq!(group.error_policy.mode, ErrorPolicyMode::Once);
    }
}
