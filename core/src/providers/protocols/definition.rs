// core/src/providers/protocols/definition.rs

//! Protocol definition types for YAML-based configurable protocols

use serde::{Deserialize, Serialize};

/// Protocol definition loaded from YAML
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProtocolDefinition {
    /// Protocol name
    pub name: String,

    /// Base protocol to extend (openai, anthropic, gemini)
    #[serde(default)]
    pub extends: Option<String>,

    /// Base URL override
    #[serde(default)]
    pub base_url: Option<String>,

    /// Differences from base protocol (minimal config mode)
    #[serde(default)]
    pub differences: Option<ProtocolDifferences>,

    /// Custom protocol implementation (full template mode)
    #[serde(default)]
    pub custom: Option<CustomProtocol>,
}

/// Differences from base protocol (minimal configuration mode)
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProtocolDifferences {
    /// Authentication overrides
    #[serde(default)]
    pub auth: Option<AuthDifferences>,

    /// Request field overrides
    #[serde(default)]
    pub request_fields: Option<serde_json::Value>,

    /// Response path overrides
    #[serde(default)]
    pub response_paths: Option<serde_json::Value>,
}

/// Authentication differences
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthDifferences {
    /// Header name (e.g., "X-API-Key")
    pub header: String,

    /// Prefix for the value (e.g., "Bearer ")
    #[serde(default)]
    pub prefix: Option<String>,
}

/// Custom protocol implementation (full template mode)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CustomProtocol {
    /// Authentication configuration
    pub auth: AuthConfig,

    /// Endpoint templates
    pub endpoints: EndpointConfig,

    /// Request template
    pub request_template: serde_json::Value,

    /// Response mapping
    pub response_mapping: ResponseMapping,

    /// Stream configuration
    #[serde(default)]
    pub stream_config: Option<StreamConfig>,
}

/// Authentication configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    /// Auth type (header, query, etc.)
    #[serde(rename = "type")]
    pub auth_type: String,

    /// Header name or query parameter name
    #[serde(flatten)]
    pub config: serde_json::Value,
}

/// Endpoint configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EndpointConfig {
    /// Chat completion endpoint
    pub chat: String,

    /// Streaming endpoint (optional, defaults to chat with streaming flag)
    #[serde(default)]
    pub stream: Option<String>,
}

/// Response field mapping
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResponseMapping {
    /// Path to content field (JSONPath)
    pub content: String,

    /// Path to error field (JSONPath)
    #[serde(default)]
    pub error: Option<String>,
}

/// Stream configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StreamConfig {
    /// Stream format (sse, jsonl, etc.)
    pub format: String,

    /// Event prefix for SSE
    #[serde(default)]
    pub event_prefix: Option<String>,

    /// Done marker
    #[serde(default)]
    pub done_marker: Option<String>,

    /// Path to content in stream chunks
    pub content_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_protocol_definition() {
        let yaml = r#"
name: test-protocol
extends: openai
base_url: https://api.example.com
"#;
        let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.name, "test-protocol");
        assert_eq!(def.extends, Some("openai".to_string()));
        assert_eq!(def.base_url, Some("https://api.example.com".to_string()));
    }

    #[test]
    fn test_minimal_config_with_differences() {
        let yaml = r#"
name: custom-auth
extends: openai
differences:
  auth:
    header: X-API-Key
    prefix: ""
"#;
        let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.name, "custom-auth");
        assert!(def.differences.is_some());
    }

    #[test]
    fn test_parse_groq_custom_example() {
        // Test parsing the actual groq-custom.yaml example
        let yaml = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../examples/protocols/groq-custom.yaml"),
        )
        .expect("Failed to read groq-custom.yaml");

        let def: ProtocolDefinition =
            serde_yaml::from_str(&yaml).expect("Failed to parse groq-custom.yaml");
        assert_eq!(def.name, "groq-custom");
        assert_eq!(def.extends, Some("openai".to_string()));
        assert_eq!(
            def.base_url,
            Some("https://api.groq.com/openai/v1".to_string())
        );
        assert!(def.differences.is_some());

        let diff = def.differences.unwrap();
        assert!(diff.auth.is_some());
        assert!(diff.request_fields.is_some());

        let auth = diff.auth.unwrap();
        assert_eq!(auth.header, "X-API-Key");
        assert_eq!(auth.prefix, Some("".to_string()));
    }

    #[test]
    fn test_parse_exotic_ai_example() {
        // Test parsing the actual exotic-ai.yaml example
        let yaml = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../examples/protocols/exotic-ai.yaml"),
        )
        .expect("Failed to read exotic-ai.yaml");

        let def: ProtocolDefinition =
            serde_yaml::from_str(&yaml).expect("Failed to parse exotic-ai.yaml");
        assert_eq!(def.name, "exotic-ai");
        assert_eq!(
            def.base_url,
            Some("https://api.exotic.ai".to_string())
        );
        assert!(def.custom.is_some());

        let custom = def.custom.unwrap();
        assert_eq!(custom.auth.auth_type, "header");
        assert_eq!(custom.endpoints.chat, "/v2/completions");
        assert!(custom.endpoints.stream.is_some());
        assert_eq!(custom.endpoints.stream.unwrap(), "/v2/completions/stream");
        assert_eq!(custom.response_mapping.content, "$.output.generated_text");
        assert!(custom.stream_config.is_some());

        let stream = custom.stream_config.unwrap();
        assert_eq!(stream.format, "sse");
        assert_eq!(stream.content_path, "$.chunk.text");
    }
}
