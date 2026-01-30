//! Webhook Configuration
//!
//! Defines configuration structures for webhook endpoints.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Signature verification format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SignatureFormat {
    /// GitHub style: X-Hub-Signature-256 header with sha256=<hex> format
    Github,
    /// Stripe style: Stripe-Signature header with t=timestamp,v1=signature format
    Stripe,
    /// Generic style: X-Webhook-Signature header with hex signature
    #[default]
    Generic,
    /// No signature verification
    None,
}

impl SignatureFormat {
    /// Get the header name for this format
    pub fn header_name(&self) -> Option<&'static str> {
        match self {
            SignatureFormat::Github => Some("X-Hub-Signature-256"),
            SignatureFormat::Stripe => Some("Stripe-Signature"),
            SignatureFormat::Generic => Some("X-Webhook-Signature"),
            SignatureFormat::None => None,
        }
    }
}

/// Configuration for a single webhook endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEndpointConfig {
    /// Unique identifier for this webhook
    pub id: String,

    /// URL path for this webhook (e.g., "/webhooks/github")
    pub path: String,

    /// HMAC secret for signature verification
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,

    /// Signature format to use
    #[serde(default)]
    pub signature_format: SignatureFormat,

    /// Target agent ID to route messages to
    pub agent: String,

    /// Session key template with variable substitution
    /// Supports: {webhook_id}, {event_type}, {source_id}
    #[serde(default = "default_session_key_template")]
    pub session_key_template: String,

    /// Allowed event types (empty = all allowed)
    #[serde(default)]
    pub allowed_events: Vec<String>,

    /// Whether this endpoint is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Custom headers to extract and include in context
    #[serde(default)]
    pub extract_headers: Vec<String>,

    /// Maximum body size in bytes (default: 1MB)
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,

    /// Description for documentation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn default_session_key_template() -> String {
    "task:webhook:{webhook_id}".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_body_size() -> usize {
    1024 * 1024 // 1MB
}

impl Default for WebhookEndpointConfig {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            path: "/webhooks/default".to_string(),
            secret: None,
            signature_format: SignatureFormat::Generic,
            agent: "main".to_string(),
            session_key_template: default_session_key_template(),
            allowed_events: vec![],
            enabled: true,
            extract_headers: vec![],
            max_body_size: default_max_body_size(),
            description: None,
        }
    }
}

impl WebhookEndpointConfig {
    /// Create a new webhook endpoint configuration
    pub fn new(id: impl Into<String>, path: impl Into<String>, agent: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            agent: agent.into(),
            ..Default::default()
        }
    }

    /// Set the HMAC secret
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = Some(secret.into());
        self
    }

    /// Set signature format
    pub fn with_signature_format(mut self, format: SignatureFormat) -> Self {
        self.signature_format = format;
        self
    }

    /// Set session key template
    pub fn with_session_key_template(mut self, template: impl Into<String>) -> Self {
        self.session_key_template = template.into();
        self
    }

    /// Add allowed events
    pub fn with_allowed_events(mut self, events: Vec<String>) -> Self {
        self.allowed_events = events;
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        if self.id.is_empty() {
            return Err(ConfigValidationError::EmptyId);
        }

        if self.path.is_empty() {
            return Err(ConfigValidationError::EmptyPath);
        }

        if !self.path.starts_with('/') {
            return Err(ConfigValidationError::InvalidPath(
                "Path must start with /".to_string(),
            ));
        }

        if self.agent.is_empty() {
            return Err(ConfigValidationError::EmptyAgent);
        }

        // Signature verification requires a secret
        if self.signature_format != SignatureFormat::None && self.secret.is_none() {
            return Err(ConfigValidationError::MissingSecret);
        }

        Ok(())
    }

    /// Check if an event type is allowed
    pub fn is_event_allowed(&self, event_type: &str) -> bool {
        if self.allowed_events.is_empty() {
            return true;
        }
        self.allowed_events.iter().any(|e| e == event_type)
    }
}

/// Webhooks system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhooksConfig {
    /// Whether the webhook system is enabled
    pub enabled: bool,

    /// Port to listen on (0 = use gateway port)
    pub port: u16,

    /// Bind address
    pub bind: String,

    /// Maximum number of endpoints
    pub max_endpoints: usize,

    /// Webhook endpoint configurations
    #[serde(default)]
    pub endpoints: Vec<WebhookEndpointConfig>,
}

impl Default for WebhooksConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 0, // Use gateway port by default
            bind: "127.0.0.1".to_string(),
            max_endpoints: 50,
            endpoints: vec![],
        }
    }
}

impl WebhooksConfig {
    /// Validate the webhooks configuration
    pub fn validate(&self, valid_agents: &[&str]) -> Result<(), ConfigValidationError> {
        if !self.enabled {
            return Ok(());
        }

        if self.endpoints.len() > self.max_endpoints {
            return Err(ConfigValidationError::TooManyEndpoints {
                count: self.endpoints.len(),
                max: self.max_endpoints,
            });
        }

        // Check for duplicate paths
        let mut paths = HashMap::new();
        for endpoint in &self.endpoints {
            endpoint.validate()?;

            if let Some(existing_id) = paths.insert(&endpoint.path, &endpoint.id) {
                return Err(ConfigValidationError::DuplicatePath {
                    path: endpoint.path.clone(),
                    id1: existing_id.clone(),
                    id2: endpoint.id.clone(),
                });
            }

            // Validate agent reference
            if !valid_agents.contains(&endpoint.agent.as_str()) {
                return Err(ConfigValidationError::InvalidAgentRef {
                    webhook_id: endpoint.id.clone(),
                    agent: endpoint.agent.clone(),
                });
            }
        }

        Ok(())
    }

    /// Get an endpoint by ID
    pub fn get_endpoint(&self, id: &str) -> Option<&WebhookEndpointConfig> {
        self.endpoints.iter().find(|e| e.id == id)
    }

    /// Get an endpoint by path
    pub fn get_endpoint_by_path(&self, path: &str) -> Option<&WebhookEndpointConfig> {
        self.endpoints.iter().find(|e| e.path == path && e.enabled)
    }
}

/// Configuration validation errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("Webhook ID cannot be empty")]
    EmptyId,

    #[error("Webhook path cannot be empty")]
    EmptyPath,

    #[error("Invalid webhook path: {0}")]
    InvalidPath(String),

    #[error("Webhook agent cannot be empty")]
    EmptyAgent,

    #[error("Signature verification enabled but no secret provided")]
    MissingSecret,

    #[error("Too many webhook endpoints: {count} (max: {max})")]
    TooManyEndpoints { count: usize, max: usize },

    #[error("Duplicate webhook path '{path}' used by '{id1}' and '{id2}'")]
    DuplicatePath {
        path: String,
        id1: String,
        id2: String,
    },

    #[error("Webhook '{webhook_id}' references unknown agent '{agent}'")]
    InvalidAgentRef { webhook_id: String, agent: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_config_default() {
        let config = WebhookEndpointConfig::default();
        assert_eq!(config.id, "default");
        assert!(config.enabled);
        assert_eq!(config.signature_format, SignatureFormat::Generic);
    }

    #[test]
    fn test_endpoint_config_builder() {
        let config = WebhookEndpointConfig::new("github", "/webhooks/github", "main")
            .with_secret("secret123")
            .with_signature_format(SignatureFormat::Github)
            .with_allowed_events(vec!["push".to_string(), "pull_request".to_string()]);

        assert_eq!(config.id, "github");
        assert_eq!(config.path, "/webhooks/github");
        assert_eq!(config.secret, Some("secret123".to_string()));
        assert_eq!(config.signature_format, SignatureFormat::Github);
        assert!(config.is_event_allowed("push"));
        assert!(!config.is_event_allowed("issues"));
    }

    #[test]
    fn test_endpoint_validation_success() {
        let config = WebhookEndpointConfig::new("test", "/webhooks/test", "main")
            .with_secret("secret");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_endpoint_validation_empty_id() {
        let config = WebhookEndpointConfig::new("", "/webhooks/test", "main");
        assert!(matches!(
            config.validate(),
            Err(ConfigValidationError::EmptyId)
        ));
    }

    #[test]
    fn test_endpoint_validation_missing_secret() {
        let config = WebhookEndpointConfig::new("test", "/webhooks/test", "main");
        // No secret but signature verification is enabled by default
        assert!(matches!(
            config.validate(),
            Err(ConfigValidationError::MissingSecret)
        ));
    }

    #[test]
    fn test_endpoint_validation_no_signature() {
        let mut config = WebhookEndpointConfig::new("test", "/webhooks/test", "main");
        config.signature_format = SignatureFormat::None;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_webhooks_config_validation() {
        let mut config = WebhooksConfig::default();
        config.endpoints.push(
            WebhookEndpointConfig::new("github", "/webhooks/github", "main")
                .with_secret("secret1"),
        );
        config.endpoints.push(
            WebhookEndpointConfig::new("stripe", "/webhooks/stripe", "main")
                .with_secret("secret2"),
        );

        assert!(config.validate(&["main"]).is_ok());
    }

    #[test]
    fn test_webhooks_config_duplicate_path() {
        let mut config = WebhooksConfig::default();
        config.endpoints.push(
            WebhookEndpointConfig::new("webhook1", "/webhooks/same", "main").with_secret("s1"),
        );
        config.endpoints.push(
            WebhookEndpointConfig::new("webhook2", "/webhooks/same", "main").with_secret("s2"),
        );

        assert!(matches!(
            config.validate(&["main"]),
            Err(ConfigValidationError::DuplicatePath { .. })
        ));
    }

    #[test]
    fn test_webhooks_config_invalid_agent() {
        let mut config = WebhooksConfig::default();
        config.endpoints.push(
            WebhookEndpointConfig::new("test", "/webhooks/test", "nonexistent")
                .with_secret("secret"),
        );

        assert!(matches!(
            config.validate(&["main"]),
            Err(ConfigValidationError::InvalidAgentRef { .. })
        ));
    }

    #[test]
    fn test_signature_format_headers() {
        assert_eq!(
            SignatureFormat::Github.header_name(),
            Some("X-Hub-Signature-256")
        );
        assert_eq!(
            SignatureFormat::Stripe.header_name(),
            Some("Stripe-Signature")
        );
        assert_eq!(
            SignatureFormat::Generic.header_name(),
            Some("X-Webhook-Signature")
        );
        assert_eq!(SignatureFormat::None.header_name(), None);
    }

    #[test]
    fn test_get_endpoint_by_path() {
        let mut config = WebhooksConfig::default();
        config.endpoints.push(
            WebhookEndpointConfig::new("github", "/webhooks/github", "main").with_secret("s"),
        );

        assert!(config.get_endpoint_by_path("/webhooks/github").is_some());
        assert!(config.get_endpoint_by_path("/webhooks/unknown").is_none());
    }
}
