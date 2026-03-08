use crate::a2a::config::A2AServerConfig;
use crate::a2a::domain::*;

/// Builds Aleph's own `AgentCard` from server configuration.
///
/// The generated card is served at `/.well-known/agent-card.json` and
/// describes this Aleph instance's capabilities to remote A2A agents.
pub struct CardBuilder;

impl CardBuilder {
    /// Build Aleph's own AgentCard from server config.
    pub fn build(config: &A2AServerConfig, bind_addr: &str) -> AgentCard {
        let name = config
            .card_name
            .clone()
            .unwrap_or_else(|| "Aleph".to_string());
        let description = config.card_description.clone();
        let version = config
            .card_version
            .clone()
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

        let skills: Vec<AgentSkill> = config
            .skills
            .iter()
            .map(|s| AgentSkill {
                id: s.id.clone(),
                name: s.name.clone(),
                description: s.description.clone(),
                aliases: None,
                examples: None,
                input_types: Some(vec!["text".to_string()]),
                output_types: Some(vec!["text".to_string()]),
            })
            .collect();

        let mut security = vec![];
        if !config.security.tokens.is_empty() {
            security.push(SecurityScheme::Http {
                scheme: "bearer".to_string(),
                bearer_format: None,
            });
        }

        AgentCard {
            id: format!("aleph-{}", simple_hostname()),
            name,
            version,
            description,
            provider: Some(AgentProvider {
                name: "Aleph".to_string(),
                url: None,
            }),
            documentation_url: None,
            interfaces: vec![AgentInterface {
                url: format!("http://{}/a2a", bind_addr),
                protocol: TransportProtocol::JsonRpc,
            }],
            skills,
            security,
            extensions: vec![],
            default_input_modes: vec!["text".to_string()],
            default_output_modes: vec!["text".to_string()],
        }
    }
}

/// Best-effort hostname retrieval without external crates.
fn simple_hostname() -> String {
    // Try HOSTNAME env first, then fall back to a default
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::config::{A2ASecurityConfig, A2ASkillConfig};

    #[test]
    fn build_with_defaults() {
        let config = A2AServerConfig::default();
        let card = CardBuilder::build(&config, "127.0.0.1:8080");

        assert_eq!(card.name, "Aleph");
        assert!(card.id.starts_with("aleph-"));
        assert!(!card.version.is_empty());
        assert!(card.provider.is_some());
        assert_eq!(card.provider.as_ref().unwrap().name, "Aleph");
        assert_eq!(card.interfaces.len(), 1);
        assert_eq!(card.interfaces[0].url, "http://127.0.0.1:8080/a2a");
        assert!(card.skills.is_empty());
        assert!(card.security.is_empty());
    }

    #[test]
    fn build_with_custom_name_and_description() {
        let config = A2AServerConfig {
            card_name: Some("My Custom Agent".to_string()),
            card_description: Some("A specialized helper".to_string()),
            card_version: Some("2.0.0".to_string()),
            ..Default::default()
        };
        let card = CardBuilder::build(&config, "0.0.0.0:3000");

        assert_eq!(card.name, "My Custom Agent");
        assert_eq!(
            card.description.as_deref(),
            Some("A specialized helper")
        );
        assert_eq!(card.version, "2.0.0");
    }

    #[test]
    fn build_with_security_tokens_adds_scheme() {
        let config = A2AServerConfig {
            security: A2ASecurityConfig {
                local_bypass: true,
                tokens: vec!["secret-token".to_string()],
            },
            ..Default::default()
        };
        let card = CardBuilder::build(&config, "127.0.0.1:8080");

        assert_eq!(card.security.len(), 1);
        match &card.security[0] {
            SecurityScheme::Http { scheme, .. } => assert_eq!(scheme, "bearer"),
            other => panic!("Expected Http scheme, got {:?}", other),
        }
    }

    #[test]
    fn build_with_skills() {
        let config = A2AServerConfig {
            skills: vec![
                A2ASkillConfig {
                    id: "code-review".to_string(),
                    name: "Code Review".to_string(),
                    description: Some("Reviews code".to_string()),
                },
                A2ASkillConfig {
                    id: "summarize".to_string(),
                    name: "Summarize".to_string(),
                    description: None,
                },
            ],
            ..Default::default()
        };
        let card = CardBuilder::build(&config, "127.0.0.1:8080");

        assert_eq!(card.skills.len(), 2);
        assert_eq!(card.skills[0].id, "code-review");
        assert_eq!(card.skills[0].name, "Code Review");
        assert_eq!(
            card.skills[0].description.as_deref(),
            Some("Reviews code")
        );
        assert_eq!(card.skills[1].id, "summarize");
        assert!(card.skills[1].description.is_none());
    }
}
