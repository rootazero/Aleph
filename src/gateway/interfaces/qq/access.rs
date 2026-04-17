use crate::gateway::interfaces::qq::config::{QQAccountConfig, QQDmPolicy, QQGroupPolicy};

pub struct AccessController {
    config: QQAccountConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDecision {
    Allow,
    Deny,
}

impl AccessController {
    pub fn new(config: QQAccountConfig) -> Self {
        Self { config }
    }

    pub fn check(&self, openid: &str, group_openid: Option<&str>) -> AccessDecision {
        if !self.config.enabled {
            return AccessDecision::Deny;
        }

        match group_openid {
            Some(gid) => self.check_group(gid),
            None => self.check_dm(openid),
        }
    }

    fn check_dm(&self, openid: &str) -> AccessDecision {
        match self.config.dm_policy {
            QQDmPolicy::Open => AccessDecision::Allow,
            QQDmPolicy::Allowlist => {
                if self.config.allowed_users.is_empty()
                    || self.config.allowed_users.contains(&openid.to_string())
                {
                    AccessDecision::Allow
                } else {
                    AccessDecision::Deny
                }
            }
        }
    }

    fn check_group(&self, group_openid: &str) -> AccessDecision {
        match self.config.group_policy {
            QQGroupPolicy::Open => AccessDecision::Allow,
            QQGroupPolicy::MentionOnly | QQGroupPolicy::Allowlist => {
                if self.config.allowed_groups.is_empty()
                    || self
                        .config
                        .allowed_groups
                        .contains(&group_openid.to_string())
                {
                    AccessDecision::Allow
                } else {
                    AccessDecision::Deny
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(allowed_users: Vec<String>, allowed_groups: Vec<String>) -> QQAccountConfig {
        QQAccountConfig {
            id: "default".into(),
            app_id: "app".into(),
            client_secret: "secret".into(),
            enabled: true,
            allowed_users,
            allowed_groups,
            dm_policy: QQDmPolicy::Allowlist,
            group_policy: QQGroupPolicy::Allowlist,
        }
    }

    #[test]
    fn test_dm_allowlist_empty() {
        let ctrl = AccessController::new(make_config(vec![], vec![]));
        assert_eq!(ctrl.check("user1", None), AccessDecision::Allow);
    }

    #[test]
    fn test_dm_allowlist_blocked() {
        let ctrl = AccessController::new(make_config(vec!["allowed".into()], vec![]));
        assert_eq!(ctrl.check("allowed", None), AccessDecision::Allow);
        assert_eq!(ctrl.check("other", None), AccessDecision::Deny);
    }

    #[test]
    fn test_group_allowlist_empty() {
        let ctrl = AccessController::new(make_config(vec![], vec![]));
        assert_eq!(ctrl.check("user1", Some("group1")), AccessDecision::Allow);
    }
}
