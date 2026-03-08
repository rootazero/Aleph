use serde::{Deserialize, Serialize};

use super::security::SecurityScheme;

/// A2A Agent Card — metadata describing an agent's capabilities.
/// Served at `/.well-known/agent-card.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<AgentInterface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<AgentSkill>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<SecurityScheme>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<AgentExtension>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_input_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_output_modes: Vec<String>,
}

/// Information about the agent's provider/organization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProvider {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// An interface endpoint the agent exposes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterface {
    pub url: String,
    pub protocol: TransportProtocol,
}

/// Transport protocol for agent communication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransportProtocol {
    JsonRpc,
    Grpc,
    HttpJson,
}

/// A skill the agent can perform
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_types: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_types: Option<Vec<String>>,
}

/// An extension declared by the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExtension {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_agent_card() -> AgentCard {
        AgentCard {
            id: "aleph-agent".to_string(),
            name: "Aleph".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Personal AI assistant".to_string()),
            provider: Some(AgentProvider {
                name: "Aleph".to_string(),
                url: Some("https://aleph.ai".to_string()),
            }),
            documentation_url: None,
            interfaces: vec![AgentInterface {
                url: "http://localhost:8080/a2a".to_string(),
                protocol: TransportProtocol::JsonRpc,
            }],
            skills: vec![AgentSkill {
                id: "code-review".to_string(),
                name: "Code Review".to_string(),
                description: Some("Review code changes".to_string()),
                aliases: Some(vec!["review".to_string()]),
                examples: Some(vec!["Review this PR".to_string()]),
                input_types: Some(vec!["text".to_string()]),
                output_types: Some(vec!["text".to_string()]),
            }],
            security: vec![],
            extensions: vec![],
            default_input_modes: vec!["text".to_string()],
            default_output_modes: vec!["text".to_string()],
        }
    }

    #[test]
    fn agent_card_serde_roundtrip() {
        let card = sample_agent_card();
        let json = serde_json::to_string(&card).unwrap();
        let back: AgentCard = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, card.id);
        assert_eq!(back.name, card.name);
        assert_eq!(back.version, card.version);
        assert_eq!(back.skills.len(), 1);
        assert_eq!(back.interfaces.len(), 1);
    }

    #[test]
    fn agent_card_json_format_matches_spec() {
        let card = sample_agent_card();
        let json = serde_json::to_value(&card).unwrap();

        // Verify camelCase field names
        assert!(json.get("id").is_some());
        assert!(json.get("name").is_some());
        assert!(json.get("version").is_some());
        assert!(json.get("description").is_some());
        assert!(json.get("provider").is_some());
        assert!(json.get("defaultInputModes").is_some());
        assert!(json.get("defaultOutputModes").is_some());

        // Verify nested camelCase
        let skill = &json["skills"][0];
        assert!(skill.get("id").is_some());
        assert!(skill.get("inputTypes").is_some());
        assert!(skill.get("outputTypes").is_some());

        let iface = &json["interfaces"][0];
        assert!(iface.get("url").is_some());
        assert!(iface.get("protocol").is_some());
    }

    #[test]
    fn agent_card_empty_collections_omitted() {
        let card = AgentCard {
            id: "test".to_string(),
            name: "Test".to_string(),
            version: "0.1.0".to_string(),
            description: None,
            provider: None,
            documentation_url: None,
            interfaces: vec![],
            skills: vec![],
            security: vec![],
            extensions: vec![],
            default_input_modes: vec![],
            default_output_modes: vec![],
        };
        let json = serde_json::to_value(&card).unwrap();
        // Empty vecs should be skipped
        assert!(json.get("interfaces").is_none());
        assert!(json.get("skills").is_none());
        assert!(json.get("security").is_none());
        assert!(json.get("extensions").is_none());
        // None fields should be skipped
        assert!(json.get("description").is_none());
        assert!(json.get("provider").is_none());
        assert!(json.get("documentationUrl").is_none());
    }

    #[test]
    fn transport_protocol_serde() {
        let json = serde_json::to_string(&TransportProtocol::JsonRpc).unwrap();
        assert_eq!(json, "\"jsonRpc\"");

        let json = serde_json::to_string(&TransportProtocol::Grpc).unwrap();
        assert_eq!(json, "\"grpc\"");

        let json = serde_json::to_string(&TransportProtocol::HttpJson).unwrap();
        assert_eq!(json, "\"httpJson\"");
    }

    #[test]
    fn agent_extension_with_params() {
        let ext = AgentExtension {
            uri: "urn:aleph:ext:memory".to_string(),
            description: Some("Memory extension".to_string()),
            params: {
                let mut m = serde_json::Map::new();
                m.insert("ttl".to_string(), serde_json::Value::Number(3600.into()));
                m
            },
        };
        let json = serde_json::to_value(&ext).unwrap();
        assert_eq!(json["uri"], "urn:aleph:ext:memory");
        assert_eq!(json["params"]["ttl"], 3600);

        let back: AgentExtension = serde_json::from_value(json).unwrap();
        assert_eq!(back.uri, ext.uri);
    }
}
