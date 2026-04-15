use super::config::{DmPolicy, GroupPolicy, StreamingOptions};
use super::config_v2::{
    ErrorPolicy, TelegramAccountConfig, TelegramConfigV2, TelegramGroupConfig, TelegramTopicConfig,
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
}

pub struct ConfigResolver {
    lookup: HashMap<(String, i64, Option<i32>), ResolvedConfig>,
}

impl ConfigResolver {
    pub fn from_v2(config: &TelegramConfigV2) -> Self {
        let mut lookup = HashMap::new();
        for account in &config.accounts {
            let account_default = Self::resolve_account_defaults(account);
            if account.groups.is_empty() {
                lookup.insert((account.id.clone(), 0, None), account_default.clone());
            }
            for group in &account.groups {
                let group_resolved = Self::merge_group(&account_default, group);
                if group.topics.is_empty() {
                    lookup.insert(
                        (account.id.clone(), group.chat_id, None),
                        group_resolved.clone(),
                    );
                }
                for topic in &group.topics {
                    let topic_resolved = Self::merge_topic(&group_resolved, topic);
                    lookup.insert(
                        (account.id.clone(), group.chat_id, Some(topic.thread_id)),
                        topic_resolved,
                    );
                }
            }
        }
        Self { lookup }
    }

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
            streaming: account.streaming.clone().unwrap_or_default(),
            error_policy: account.error_policy.clone().unwrap_or_default(),
        }
    }

    fn merge_group(base: &ResolvedConfig, group: &TelegramGroupConfig) -> ResolvedConfig {
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
            streaming: base.streaming.clone(),
            error_policy: group
                .error_policy
                .clone()
                .unwrap_or_else(|| base.error_policy.clone()),
        }
    }

    fn merge_topic(base: &ResolvedConfig, topic: &TelegramTopicConfig) -> ResolvedConfig {
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
            error_policy: topic
                .error_policy
                .clone()
                .unwrap_or_else(|| base.error_policy.clone()),
        }
    }
}
