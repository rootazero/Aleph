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

#[test]
fn test_full_agent_pipeline() {
    use crate::config::agent_resolver::AgentDefinitionResolver;
    use crate::routing::config::SessionConfig;
    use crate::routing::resolve::{resolve_route, RouteInput};
    use std::collections::HashMap;

    // 1. Parse config with agents and bindings
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

    // 2. Verify agents parsed
    assert_eq!(config.agents.list.len(), 2);
    assert_eq!(config.bindings.len(), 2);

    // 3. Resolve agents
    let profiles = HashMap::new();
    let mut resolver = AgentDefinitionResolver::new();
    let resolved = resolver.resolve_all(&config.agents, &profiles);
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].model, "claude-opus-4-6");

    // 4. Test route resolution with bindings — slack routes to coding
    let session_cfg = SessionConfig::default();
    let route = resolve_route(
        &config.bindings,
        &session_cfg,
        "main",
        &RouteInput {
            channel: "slack".to_string(),
            account_id: None,
            peer: None,
            guild_id: None,
            team_id: Some("T12345".to_string()),
        },
    );
    assert_eq!(route.agent_id, "coding");

    // 5. Telegram routes to main
    let route2 = resolve_route(
        &config.bindings,
        &session_cfg,
        "main",
        &RouteInput {
            channel: "telegram".to_string(),
            account_id: None,
            peer: None,
            guild_id: None,
            team_id: None,
        },
    );
    assert_eq!(route2.agent_id, "main");
}
