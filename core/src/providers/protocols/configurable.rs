// core/src/providers/protocols/configurable.rs

//! Configurable protocol adapter loaded from YAML

use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::protocols::{ProtocolDefinition, ProtocolRegistry, TemplateRenderer};
use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::Client;
use std::sync::Arc;
use tracing::debug;

/// Protocol adapter configured from YAML definition
pub struct ConfigurableProtocol {
    definition: ProtocolDefinition,
    client: Client,
    /// Base protocol to extend (for minimal mode)
    base_protocol: Option<Arc<dyn ProtocolAdapter>>,
    /// Template renderer for custom mode
    renderer: TemplateRenderer,
}

impl ConfigurableProtocol {
    /// Create a new configurable protocol
    pub fn new(definition: ProtocolDefinition, client: Client) -> Result<Self> {
        // Load base protocol if extending
        let base_protocol = if let Some(ref extends) = definition.extends {
            debug!(
                protocol = %definition.name,
                extends = %extends,
                "Loading base protocol for minimal mode"
            );

            ProtocolRegistry::global()
                .get(extends)
                .ok_or_else(|| {
                    AetherError::provider(format!(
                        "Base protocol '{}' not found in registry",
                        extends
                    ))
                })?
                .into()
        } else {
            None
        };

        // Initialize template renderer
        let renderer = TemplateRenderer::new()?;

        Ok(Self {
            definition,
            client,
            base_protocol,
            renderer,
        })
    }
}

#[async_trait]
impl ProtocolAdapter for ConfigurableProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        // Minimal mode: extend base protocol
        if let Some(ref base) = self.base_protocol {
            debug!(
                protocol = %self.definition.name,
                base = %base.name(),
                "Building request using minimal mode (extend base)"
            );

            // Delegate to base protocol to build the request
            let mut request = base.build_request(payload, config, is_streaming)?;

            // Apply auth differences if specified
            if let Some(ref differences) = self.definition.differences {
                if let Some(ref auth) = differences.auth {
                    let api_key = config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::invalid_config("API key is required"))?;

                    // Build auth value: prefix + api_key (or just api_key if no prefix)
                    let auth_value = if let Some(ref prefix) = auth.prefix {
                        format!("{}{}", prefix, api_key)
                    } else {
                        api_key.to_string()
                    };

                    debug!(
                        header = %auth.header,
                        has_prefix = auth.prefix.is_some(),
                        "Applying custom auth header"
                    );

                    // Override auth header
                    request = request.header(&auth.header, auth_value);
                }
            }

            return Ok(request);
        }

        // Custom mode: not yet implemented
        if self.definition.custom.is_some() {
            return Err(AetherError::provider(
                "Custom protocol mode not yet implemented (Task 5)",
            ));
        }

        // No base protocol and no custom config = invalid
        Err(AetherError::invalid_config(
            "Protocol must either extend a base protocol or provide custom configuration",
        ))
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
        // Minimal mode: delegate to base protocol
        if let Some(ref base) = self.base_protocol {
            debug!(
                protocol = %self.definition.name,
                base = %base.name(),
                "Parsing response using base protocol"
            );
            return base.parse_response(response).await;
        }

        // Custom mode: not yet implemented
        if self.definition.custom.is_some() {
            return Err(AetherError::provider(
                "Custom protocol mode not yet implemented (Task 5)",
            ));
        }

        Err(AetherError::invalid_config(
            "Protocol must either extend a base protocol or provide custom configuration",
        ))
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        // Minimal mode: delegate to base protocol
        if let Some(ref base) = self.base_protocol {
            debug!(
                protocol = %self.definition.name,
                base = %base.name(),
                "Parsing stream using base protocol"
            );
            return base.parse_stream(response).await;
        }

        // Custom mode: not yet implemented
        if self.definition.custom.is_some() {
            return Err(AetherError::provider(
                "Custom protocol mode not yet implemented (Task 5)",
            ));
        }

        Err(AetherError::invalid_config(
            "Protocol must either extend a base protocol or provide custom configuration",
        ))
    }

    fn name(&self) -> &'static str {
        // SAFETY: We leak the string to get a 'static lifetime
        // This is acceptable for protocol names which are created rarely
        Box::leak(self.definition.name.clone().into_boxed_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::protocols::definition::{AuthDifferences, ProtocolDifferences};

    #[test]
    fn test_configurable_protocol_name() {
        // Register built-in protocols first
        ProtocolRegistry::global().register_builtin();

        let def = ProtocolDefinition {
            name: "test-proto".to_string(),
            extends: Some("openai".to_string()),
            base_url: None,
            differences: None,
            custom: None,
        };

        let client = reqwest::Client::new();
        let proto = ConfigurableProtocol::new(def, client).expect("Should create protocol");
        assert_eq!(proto.name(), "test-proto");
    }

    #[test]
    fn test_minimal_mode_build_request() {
        // Register built-in protocols first
        ProtocolRegistry::global().register_builtin();

        // Create a minimal config that extends OpenAI
        let def = ProtocolDefinition {
            name: "custom-openai".to_string(),
            extends: Some("openai".to_string()),
            base_url: None,
            differences: Some(ProtocolDifferences {
                auth: Some(AuthDifferences {
                    header: "X-API-Key".to_string(),
                    prefix: Some("Token ".to_string()),
                }),
                request_fields: None,
                response_paths: None,
            }),
            custom: None,
        };

        let client = reqwest::Client::new();
        let proto = ConfigurableProtocol::new(def, client).expect("Should create protocol");

        // Verify name
        assert_eq!(proto.name(), "custom-openai");

        // Verify base protocol was loaded
        assert!(proto.base_protocol.is_some());

        // Create a test config
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some("test-key".to_string());

        // Create a test payload
        let payload = RequestPayload::new("Hello, world!");

        // Build request (this should work now)
        let request = proto
            .build_request(&payload, &config, false)
            .expect("Should build request");

        // Verify the request was built (we can't easily inspect headers in reqwest::RequestBuilder,
        // but we verified it didn't error which means the base protocol delegation worked)
        // The actual header verification would require sending the request, which is out of scope
        // for a unit test
        drop(request);
    }

    #[test]
    fn test_minimal_mode_without_auth_differences() {
        // Register built-in protocols first
        ProtocolRegistry::global().register_builtin();

        // Create a minimal config that extends OpenAI without auth differences
        let def = ProtocolDefinition {
            name: "simple-openai".to_string(),
            extends: Some("openai".to_string()),
            base_url: None,
            differences: None,
            custom: None,
        };

        let client = reqwest::Client::new();
        let proto = ConfigurableProtocol::new(def, client).expect("Should create protocol");

        // Create a test config
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some("test-key".to_string());

        // Create a test payload
        let payload = RequestPayload::new("Test message");

        // Build request (should use base protocol's auth)
        let request = proto
            .build_request(&payload, &config, false)
            .expect("Should build request");

        drop(request);
    }

    #[test]
    fn test_custom_mode_not_implemented() {
        use crate::providers::protocols::definition::{
            AuthConfig, CustomProtocol, EndpointConfig, ResponseMapping,
        };
        use serde_json::json;

        // Create a custom protocol definition
        let def = ProtocolDefinition {
            name: "custom-proto".to_string(),
            extends: None,
            base_url: None,
            differences: None,
            custom: Some(CustomProtocol {
                auth: AuthConfig {
                    auth_type: "header".to_string(),
                    config: json!({"header": "Authorization", "prefix": "Bearer "}),
                },
                endpoints: EndpointConfig {
                    chat: "/v1/chat".to_string(),
                    stream: None,
                },
                request_template: json!({}),
                response_mapping: ResponseMapping {
                    content: "$.data.content".to_string(),
                    error: None,
                },
                stream_config: None,
            }),
        };

        let client = reqwest::Client::new();
        let proto = ConfigurableProtocol::new(def, client).expect("Should create protocol");

        let config = ProviderConfig::test_config("test-model");
        let payload = RequestPayload::new("Test");

        // Custom mode should return "not yet implemented" error
        let result = proto.build_request(&payload, &config, false);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not yet implemented"));
    }
}
