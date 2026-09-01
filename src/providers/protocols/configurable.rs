// core/src/providers/protocols/configurable.rs

//! Configurable protocol adapter loaded from YAML

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, ProviderResponse, RequestPayload};
use crate::providers::delta::ProviderDelta;
use crate::providers::protocols::{
    extract_value, ProtocolDefinition, ProtocolRegistry, TemplateContext, TemplateRenderer,
};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::Client;
use tracing::debug;

/// Protocol adapter configured from YAML definition
pub struct ConfigurableProtocol {
    definition: ProtocolDefinition,
    client: Client,
    /// Base protocol to extend (for minimal mode)
    base_protocol: Option<Arc<dyn ProtocolAdapter>>,
    /// Template renderer for custom mode
    renderer: TemplateRenderer,
    /// Protocol name leaked once at construction to satisfy the `&'static str`
    /// trait signature. Leaking here (once per instance) avoids the per-call
    /// `Box::leak` that `name()` would otherwise perform on every invocation.
    name_static: &'static str,
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
                    AlephError::provider(format!("Base protocol '{extends}' not found in registry"))
                })?
                .into()
        } else {
            None
        };

        // Initialize template renderer
        let renderer = TemplateRenderer::new()?;

        // SAFETY/INTENT: leak the name once per protocol instance to obtain the
        // `&'static str` the trait requires. Protocols are constructed rarely
        // (load / hot-reload), so this is a bounded, one-time leak.
        let name_static: &'static str = Box::leak(definition.name.clone().into_boxed_str());

        Ok(Self {
            definition,
            client,
            base_protocol,
            renderer,
            name_static,
        })
    }

    /// Parse a custom-mode response using the protocol's response mapping.
    ///
    /// Used by `stream_deltas()` to bridge custom protocols that cannot produce
    /// fine-grained delta streams — reads the full body and wraps it as a
    /// single-shot delta stream.
    async fn parse_custom_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
        let custom = self.definition.custom.as_ref().ok_or_else(|| {
            AlephError::invalid_config(
                "Protocol must either extend a base protocol or provide custom configuration",
            )
        })?;

        debug!(
            protocol = %self.definition.name,
            "Parsing response using custom response mapping"
        );

        // Custom-mode response body read needs both a time bound and a size bound:
        // the shared error-body reader applies a 15s timeout, but the success path
        // here would otherwise read an arbitrarily large or stalled payload without
        // either. Both caps keep failover prompt and prevent an attacker-controlled
        // provider from stalling the turn.
        const CUSTOM_BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
        const MAX_CUSTOM_BODY_BYTES: usize = 8 * 1024 * 1024;
        let body = match tokio::time::timeout(CUSTOM_BODY_TIMEOUT, response.text()).await {
            Ok(Ok(b)) if b.len() <= MAX_CUSTOM_BODY_BYTES => b,
            Ok(Ok(_b)) => {
                return Err(AlephError::provider(format!(
                    "Custom protocol response body exceeded {MAX_CUSTOM_BODY_BYTES} bytes"
                )));
            }
            Ok(Err(e)) => {
                return Err(AlephError::provider(format!(
                    "Failed to read response body: {e}"
                )));
            }
            Err(_) => {
                return Err(AlephError::provider(
                    "Custom protocol response body read timed out".to_string(),
                ));
            }
        };

        let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            AlephError::provider(format!(
                "Failed to parse response as JSON: {e}. Body: {body}"
            ))
        })?;

        // Check for error if error path is specified
        if let Some(ref error_path) = custom.response_mapping.error {
            if let Ok(error_msg) = extract_value(&json, error_path) {
                if !error_msg.is_empty() && error_msg != "null" {
                    return Err(AlephError::provider(format!(
                        "Provider returned error: {error_msg}"
                    )));
                }
            }
        }

        let content = extract_value(&json, &custom.response_mapping.content)?;

        debug!(
            content_len = content.len(),
            "Successfully parsed custom protocol response"
        );

        Ok(ProviderResponse::text_only(content))
    }
}

#[async_trait]
impl ProtocolAdapter for ConfigurableProtocol {
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        // Minimal mode: extend base protocol
        if let Some(ref base) = self.base_protocol {
            debug!(
                protocol = %self.definition.name,
                base = %base.name(),
                "Building request using minimal mode (extend base)"
            );

            // Delegate to base protocol to build the request (always streaming)
            let mut request = base.build_request(payload, config)?;

            // Apply auth differences if specified
            if let Some(ref differences) = self.definition.differences {
                if let Some(ref auth) = differences.auth {
                    let api_key = config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

                    // Build auth value: prefix + api_key (or just api_key if no prefix)
                    let auth_value = if let Some(ref prefix) = auth.prefix {
                        format!("{prefix}{api_key}")
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

        // Custom mode: use template rendering
        if let Some(ref custom) = self.definition.custom {
            debug!(
                protocol = %self.definition.name,
                "Building request using custom mode (template rendering)"
            );

            // Determine base URL
            let base_url = self.definition.base_url.as_deref().ok_or_else(|| {
                AlephError::invalid_config("base_url is required for custom protocols")
            })?;

            // Defence in depth: reject non-HTTP schemes before reqwest sees the URL.
            // Custom protocols come from operator-loaded YAML so the same
            // file:// / javascript: / etc. smuggling risk as the built-in
            // adapters applies here.
            if let Err(e) =
                crate::providers::protocols::http_client::validate_provider_base_url(base_url)
            {
                return Err(AlephError::invalid_config(e.to_string()));
            }

            // Always prefer stream endpoint if available (stream-first architecture)
            let endpoint = custom
                .endpoints
                .stream
                .as_deref()
                .unwrap_or(&custom.endpoints.chat);

            // Build full URL
            let url = format!("{base_url}{endpoint}");

            debug!(
                url = %url,
                "Building custom protocol request"
            );

            // Extract last user message text as template input
            let input_text = payload
                .messages
                .iter()
                .rev()
                .find_map(|m| match m {
                    crate::providers::message::UnifiedMessage::User { content } => Some(
                        content
                            .iter()
                            .filter_map(|b| b.as_text())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    _ => None,
                })
                .unwrap_or_default();

            // Build template context (per-request overrides applied after config)
            let context = TemplateContext::new()
                .with_config(config)
                .with_payload_overrides(payload)
                .with_input(&input_text)
                .with_system_prompt(payload.system_prompt.unwrap_or(""))
                .build();

            // Render request template as JSON
            let request_body = if custom.request_template.is_string() {
                // Template is a string, render it
                let template_str = custom.request_template.as_str().ok_or_else(|| {
                    AlephError::invalid_config("request_template string conversion failed")
                })?;
                self.renderer.render_json(template_str, &context)?
            } else {
                // Template is already a JSON object, render it as string first
                let template_str =
                    serde_json::to_string(&custom.request_template).map_err(|e| {
                        AlephError::provider(format!("Failed to serialize request_template: {e}"))
                    })?;
                self.renderer.render_json(&template_str, &context)?
            };

            debug!(
                request_body = ?request_body,
                "Rendered request template"
            );

            // Start building request
            let mut request = self.client.post(&url).json(&request_body);

            // Add authentication
            let api_key = config
                .api_key
                .as_ref()
                .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

            // Parse auth config
            if custom.auth.auth_type == "header" {
                // Extract header name and prefix from config
                let header = custom
                    .auth
                    .config
                    .get("header")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AlephError::invalid_config("auth.config.header is required for header auth")
                    })?;

                let prefix = custom
                    .auth
                    .config
                    .get("prefix")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let auth_value = format!("{prefix}{api_key}");

                debug!(
                    header = %header,
                    has_prefix = !prefix.is_empty(),
                    "Adding custom auth header"
                );

                request = request.header(header, auth_value);
            } else {
                return Err(AlephError::invalid_config(format!(
                    "Unsupported auth type: {}",
                    custom.auth.auth_type
                )));
            }

            return Ok(request);
        }

        // No base protocol and no custom config = invalid
        Err(AlephError::invalid_config(
            "Protocol must either extend a base protocol or provide custom configuration",
        ))
    }

    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>> {
        // Minimal mode: delegate entirely to the base protocol
        if let Some(ref base) = self.base_protocol {
            debug!(
                protocol = %self.definition.name,
                base = %base.name(),
                "Streaming deltas using base protocol (minimal mode)"
            );
            return base.stream_deltas(response).await;
        }

        // Custom mode: arbitrary API formats cannot produce fine-grained deltas.
        // Bridge via parse_custom_response() → response_to_delta_stream_result().
        if self.definition.custom.is_some() {
            debug!(
                protocol = %self.definition.name,
                "Streaming deltas via bridge (custom mode — parse_custom_response wrapping)"
            );
            let provider_response = self.parse_custom_response(response).await?;
            return Ok(crate::providers::delta::response_to_delta_stream_result(
                provider_response,
            ));
        }

        Err(AlephError::invalid_config(
            "Protocol must either extend a base protocol or provide custom configuration",
        ))
    }

    fn name(&self) -> &'static str {
        // Leaked once at construction (see `name_static`); no per-call allocation.
        self.name_static
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
        let msgs = [crate::providers::message::UnifiedMessage::user(
            "Hello, world!",
        )];
        let payload = RequestPayload::new(&msgs);

        // Build request (this should work now)
        let request = proto
            .build_request(&payload, &config)
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
        let msgs = [crate::providers::message::UnifiedMessage::user(
            "Test message",
        )];
        let payload = RequestPayload::new(&msgs);

        // Build request (should use base protocol's auth)
        let request = proto
            .build_request(&payload, &config)
            .expect("Should build request");

        drop(request);
    }

    #[test]
    fn test_custom_mode_build_request() {
        use crate::providers::protocols::definition::{
            AuthConfig, CustomProtocol, EndpointConfig, ResponseMapping,
        };
        use serde_json::json;

        // Create a custom protocol definition
        let def = ProtocolDefinition {
            name: "custom-proto".to_string(),
            extends: None,
            base_url: Some("https://api.example.com".to_string()),
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
                request_template: json!(
                    r#"{"model": "{{config.model}}", "messages": [{"role": "user", "content": "{{input}}"}]}"#
                ),
                response_mapping: ResponseMapping {
                    content: "$.choices[0].message.content".to_string(),
                    error: None,
                },
                stream_config: None,
            }),
        };

        let client = reqwest::Client::new();
        let proto = ConfigurableProtocol::new(def, client).expect("Should create protocol");

        // Verify it doesn't delegate to base protocol
        assert!(proto.base_protocol.is_none());

        let mut config = ProviderConfig::test_config("test-model");
        config.api_key = Some("test-key-123".to_string());
        let msgs = [crate::providers::message::UnifiedMessage::user(
            "Hello, AI!",
        )];
        let payload = RequestPayload::new(&msgs);

        // Build request should work now
        let result = proto.build_request(&payload, &config);
        assert!(result.is_ok(), "Should build custom request successfully");

        // The request was built (we can't easily inspect the body in unit tests,
        // but we verified it didn't error which means template rendering worked)
    }
}
