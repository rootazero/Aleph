//! Integration tests for agent definitions and bindings in Config

use crate::config::Config;

#[test]
fn test_config_with_agents_and_bindings() {
    let toml_str = r#"
        [agents.defaults]
        model = "claude-opus-4-6"

        [[agents.list]]
        id = "main"
        default = true
        name = "Aleph"

        [[agents.list]]
        id = "coding"
        name = "Code Expert"
        profile = "coding"

        [[bindings]]
        agent_id = "coding"
        [bindings.match]
        channel = "slack"
        account_id = "*"
        team_id = "T12345"

        [[bindings]]
        agent_id = "main"
        [bindings.match]
        channel = "telegram"
        account_id = "*"
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.agents.list.len(), 2);
    assert_eq!(config.bindings.len(), 2);
    assert_eq!(config.bindings[0].agent_id, "coding");
    assert_eq!(
        config.bindings[0].match_rule.team_id.as_deref(),
        Some("T12345")
    );
}

#[test]
fn test_config_without_agents_backward_compat() {
    let toml_str = r#"
        [general]
        language = "en"
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(config.agents.list.is_empty());
    assert!(config.bindings.is_empty());
}
