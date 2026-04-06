use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockAction {
    Notify,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAction {
    AutoExecute,
    AskUser,
    Block { action: BlockAction },
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalDecision {
    pub action: ApprovalAction,
    pub reason: String,
}

impl<'de> Deserialize<'de> for ApprovalDecision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawApprovalDecision {
            action: String,
            #[serde(default)]
            block_action: Option<String>,
            #[serde(default)]
            reason: String,
        }

        let raw = RawApprovalDecision::deserialize(deserializer)?;

        let action = match raw.action.as_str() {
            "auto_execute" => ApprovalAction::AutoExecute,
            "ask_user" => ApprovalAction::AskUser,
            "block" => {
                let block_action = match raw.block_action.as_deref() {
                    Some("notify") => BlockAction::Notify,
                    Some("retry") => BlockAction::Retry,
                    _ => return Err(D::Error::custom("invalid block_action value")),
                };
                ApprovalAction::Block {
                    action: block_action,
                }
            }
            _ => return Err(D::Error::custom("invalid action value")),
        };

        Ok(Self {
            action,
            reason: raw.reason,
        })
    }
}

impl Default for ApprovalDecision {
    fn default() -> Self {
        Self {
            action: ApprovalAction::AskUser,
            reason: "parse failure fallback".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ApprovalConfig {
    pub always_confirm: HashSet<String>,
}

impl ApprovalConfig {
    pub fn is_always_confirm(&self, tool_name: &str) -> bool {
        self.always_confirm.contains(tool_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_execute_deserialization() {
        let json = r#"{"action":"auto_execute","reason":"safe read"}"#;
        let decision: ApprovalDecision = serde_json::from_str(json).unwrap();
        assert!(matches!(decision.action, ApprovalAction::AutoExecute));
        assert_eq!(decision.reason, "safe read");
    }

    #[test]
    fn test_ask_user_deserialization() {
        let json = r#"{"action":"ask_user","reason":"uncertain"}"#;
        let decision: ApprovalDecision = serde_json::from_str(json).unwrap();
        assert!(matches!(decision.action, ApprovalAction::AskUser));
    }

    #[test]
    fn test_block_notify_deserialization() {
        let json = r#"{"action":"block","block_action":"notify","reason":"dangerous op"}"#;
        let decision: ApprovalDecision = serde_json::from_str(json).unwrap();
        assert!(matches!(
            decision.action,
            ApprovalAction::Block {
                action: BlockAction::Notify
            }
        ));
    }

    #[test]
    fn test_block_retry_deserialization() {
        let json = r#"{"action":"block","block_action":"retry","reason":"alternative available"}"#;
        let decision: ApprovalDecision = serde_json::from_str(json).unwrap();
        assert!(matches!(
            decision.action,
            ApprovalAction::Block {
                action: BlockAction::Retry
            }
        ));
    }

    #[test]
    fn test_default_is_ask_user() {
        let decision = ApprovalDecision::default();
        assert!(matches!(decision.action, ApprovalAction::AskUser));
        assert_eq!(decision.reason, "parse failure fallback");
    }

    #[test]
    fn test_approval_config_always_confirm() {
        let mut config = ApprovalConfig::default();
        config.always_confirm.insert("bash_exec".to_string());
        config.always_confirm.insert("file_delete".to_string());

        assert!(config.is_always_confirm("bash_exec"));
        assert!(config.is_always_confirm("file_delete"));
        assert!(!config.is_always_confirm("read_file"));
    }

    #[test]
    fn test_empty_always_confirm() {
        let config = ApprovalConfig::default();
        assert!(!config.is_always_confirm("any_tool"));
    }
}
