//! YamlPolicy - Implements Policy trait for YAML-based rules

use crate::daemon::dispatcher::policy::{Policy, ProposedAction, ActionType, NotificationPriority, RiskLevel as PolicyRiskLevel};
use crate::daemon::dispatcher::yaml_policy::schema::{YamlRule, RiskLevel as YamlRiskLevel};
use crate::daemon::dispatcher::scripting::{create_sandboxed_engine, register_duration_helpers, HistoryApi};
use crate::daemon::events::DerivedEvent;
use crate::daemon::worldmodel::state::EnhancedContext;
use crate::daemon::worldmodel::WorldModel;
use std::sync::Arc;

pub struct YamlPolicy {
    rule: YamlRule,
    worldmodel: Arc<WorldModel>,
}

impl YamlPolicy {
    pub fn new(rule: YamlRule, worldmodel: Arc<WorldModel>) -> Self {
        Self { rule, worldmodel }
    }

    fn evaluate_conditions(&self, _event: &DerivedEvent) -> bool {
        if self.rule.conditions.is_empty() {
            return true; // No conditions = always match
        }

        // Create Rhai engine
        let mut engine = create_sandboxed_engine();
        register_duration_helpers(&mut engine);

        // Register HistoryApi
        let _history = HistoryApi::new(self.worldmodel.clone());
        // TODO: Register history into engine scope

        // Evaluate all conditions (AND logic)
        for condition in &self.rule.conditions {
            match engine.eval::<bool>(&condition.expr) {
                Ok(result) => {
                    if !result {
                        return false;
                    }
                }
                Err(e) => {
                    log::error!("Rule '{}' condition error: {}", self.rule.name, e);
                    return false; // Error = condition not met
                }
            }
        }

        true
    }
}

impl Policy for YamlPolicy {
    fn name(&self) -> &str {
        &self.rule.name
    }

    fn evaluate(
        &self,
        _context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Option<ProposedAction> {
        if !self.rule.enabled {
            return None;
        }

        // Check trigger event type
        // TODO: Parse trigger.event to match DerivedEvent variant

        // Evaluate conditions
        if !self.evaluate_conditions(event) {
            return None;
        }

        // Build action
        let action_type = self.parse_action_type();
        let risk_level = self.yaml_risk_to_policy_risk(self.rule.risk);

        Some(ProposedAction {
            action_type,
            reason: format!("Rule '{}' triggered", self.rule.name),
            risk_level,
            metadata: self.rule.metadata.clone(),
        })
    }
}

impl YamlPolicy {
    fn parse_action_type(&self) -> ActionType {
        match self.rule.action.action_type.as_str() {
            "mute_system_audio" => ActionType::MuteSystemAudio,
            "unmute_system_audio" => ActionType::UnmuteSystemAudio,
            "enable_do_not_disturb" => ActionType::EnableDoNotDisturb,
            "disable_do_not_disturb" => ActionType::DisableDoNotDisturb,
            "notify" => ActionType::NotifyUser {
                message: self.rule.action.message.clone().unwrap_or_default(),
                priority: self.parse_priority(),
            },
            _ => {
                log::warn!("Unknown action type: {}", self.rule.action.action_type);
                ActionType::NotifyUser {
                    message: "Unknown action".to_string(),
                    priority: NotificationPriority::Low,
                }
            }
        }
    }

    fn parse_priority(&self) -> NotificationPriority {
        match self.rule.action.priority.as_deref() {
            Some("high") => NotificationPriority::High,
            Some("normal") => NotificationPriority::Normal,
            _ => NotificationPriority::Low,
        }
    }

    fn yaml_risk_to_policy_risk(&self, yaml_risk: YamlRiskLevel) -> PolicyRiskLevel {
        match yaml_risk {
            YamlRiskLevel::Low => PolicyRiskLevel::Low,
            YamlRiskLevel::Medium => PolicyRiskLevel::Medium,
            YamlRiskLevel::High => PolicyRiskLevel::High,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::worldmodel::WorldModelConfig;
    use crate::daemon::event_bus::DaemonEventBus;
    use crate::daemon::worldmodel::state::ActivityType;
    use chrono::Utc;

    #[tokio::test]
    async fn test_yaml_policy_simple_rule() {
        let yaml = r#"
name: "Test Rule"
enabled: true
trigger:
  event: activity_changed
action:
  type: notify
  message: "Test notification"
  priority: high
risk: low
"#;
        let rule: YamlRule = serde_yaml::from_str(yaml).unwrap();

        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let policy = YamlPolicy::new(rule, worldmodel);

        let context = EnhancedContext::default();
        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Idle,
            new_activity: ActivityType::Programming {
                language: Some("rust".to_string()),
                project: None,
            },
            confidence: 0.9,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_some());

        let action = result.unwrap();
        assert_eq!(action.risk_level as u8, PolicyRiskLevel::Low as u8);
    }

    #[tokio::test]
    async fn test_yaml_policy_disabled_rule() {
        let yaml = r#"
name: "Disabled Rule"
enabled: false
trigger:
  event: activity_changed
action:
  type: notify
risk: low
"#;
        let rule: YamlRule = serde_yaml::from_str(yaml).unwrap();

        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let policy = YamlPolicy::new(rule, worldmodel);

        let context = EnhancedContext::default();
        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Idle,
            new_activity: ActivityType::Idle,
            confidence: 1.0,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_none());
    }
}
