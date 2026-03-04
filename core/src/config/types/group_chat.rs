//! Group Chat Configuration
//!
//! Configuration for the multi-agent group chat system including
//! persona limits, round limits, and preset persona definitions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const MAX_SYSTEM_PROMPT_LEN: usize = 2000;

/// Group Chat Configuration
///
/// Controls the behavior of multi-agent group chat sessions including
/// persona limits, round limits, and coordinator visibility.
///
/// # Example Configuration (config.toml)
///
/// ```toml
/// [group_chat]
/// max_personas_per_session = 6
/// max_rounds = 10
/// coordinator_visible = false
/// default_coordinator_model = "claude-sonnet-4-20250514"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GroupChatConfig {
    /// Maximum number of personas allowed in a single session
    /// Default: 6
    #[serde(default = "default_max_personas_per_session")]
    pub max_personas_per_session: usize,

    /// Maximum number of discussion rounds before auto-ending
    /// Default: 10
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,

    /// Whether the coordinator's internal messages are visible to the user
    /// Default: false
    #[serde(default)]
    pub coordinator_visible: bool,

    /// Default model to use for the coordinator (if not specified per-session)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_coordinator_model: Option<String>,
}

fn default_max_personas_per_session() -> usize {
    6
}

fn default_max_rounds() -> usize {
    10
}

impl Default for GroupChatConfig {
    fn default() -> Self {
        Self {
            max_personas_per_session: default_max_personas_per_session(),
            max_rounds: default_max_rounds(),
            coordinator_visible: false,
            default_coordinator_model: None,
        }
    }
}

impl GroupChatConfig {
    /// Create a new GroupChatConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.max_personas_per_session == 0 {
            return Err("max_personas_per_session must be greater than 0".to_string());
        }
        if self.max_rounds == 0 {
            return Err("max_rounds must be greater than 0".to_string());
        }
        Ok(())
    }
}

/// Preset Persona Configuration
///
/// Defines a persona that can be used in group chat sessions.
/// Personas are stored in the config file and referenced by ID.
///
/// # Example Configuration (config.toml)
///
/// ```toml
/// [[personas]]
/// id = "architect"
/// name = "Software Architect"
/// system_prompt = "You are a senior software architect..."
/// provider = "anthropic"
/// model = "claude-sonnet-4-20250514"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PersonaConfig {
    /// Unique identifier for this persona
    pub id: String,

    /// Display name shown in conversation
    pub name: String,

    /// System prompt that defines the persona's character, expertise, and behavior
    pub system_prompt: String,

    /// Optional AI provider override (e.g., "anthropic", "openai")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// Optional model override (e.g., "claude-sonnet-4-20250514")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Optional thinking level override (e.g., "low", "medium", "high")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
}

impl Default for PersonaConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            system_prompt: String::new(),
            provider: None,
            model: None,
            thinking_level: None,
        }
    }
}

impl PersonaConfig {
    /// Validate the persona configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("persona id must not be empty".to_string());
        }
        if self.name.is_empty() {
            return Err("persona name must not be empty".to_string());
        }
        if self.system_prompt.is_empty() {
            return Err("persona system_prompt must not be empty".to_string());
        }
        if self.system_prompt.len() > MAX_SYSTEM_PROMPT_LEN {
            return Err(format!(
                "persona system_prompt exceeds maximum length of {} characters",
                MAX_SYSTEM_PROMPT_LEN
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GroupChatConfig::default();
        assert_eq!(config.max_personas_per_session, 6);
        assert_eq!(config.max_rounds, 10);
        assert!(!config.coordinator_visible);
        assert!(config.default_coordinator_model.is_none());
    }

    #[test]
    fn test_config_validation_valid() {
        let config = GroupChatConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_zero_personas() {
        let config = GroupChatConfig {
            max_personas_per_session: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_zero_rounds() {
        let config = GroupChatConfig {
            max_rounds: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_persona_config_validation_valid() {
        let p = PersonaConfig {
            id: "arch".into(),
            name: "Architect".into(),
            system_prompt: "You are an architect".into(),
            provider: None,
            model: None,
            thinking_level: None,
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn test_persona_config_validation_empty_id() {
        let p = PersonaConfig {
            id: "".into(),
            name: "Architect".into(),
            system_prompt: "prompt".into(),
            ..Default::default()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn test_persona_config_validation_prompt_too_long() {
        let p = PersonaConfig {
            id: "test".into(),
            name: "Test".into(),
            system_prompt: "x".repeat(2001),
            ..Default::default()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn test_config_serialization() {
        let config = GroupChatConfig::default();
        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("max_personas_per_session"));
        assert!(serialized.contains("max_rounds"));
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            max_personas_per_session = 4
            max_rounds = 5
            coordinator_visible = true
        "#;
        let config: GroupChatConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.max_personas_per_session, 4);
        assert_eq!(config.max_rounds, 5);
        assert!(config.coordinator_visible);
        assert!(config.default_coordinator_model.is_none());
    }
}
