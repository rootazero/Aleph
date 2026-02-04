//! Template engine wrapper for protocol request/response transformation
//!
//! This module provides a Handlebars-based template engine for transforming
//! YAML protocol templates into actual HTTP requests and parsing responses.
//!
//! # Template Variables
//!
//! Templates support the following context variables:
//! - `{{config.model}}` - Model name from provider config
//! - `{{config.temperature}}` - Temperature parameter
//! - `{{config.max_tokens}}` - Max tokens parameter
//! - `{{input}}` - User input text
//! - `{{system_prompt}}` - System prompt text
//! - `{{messages}}` - Array of message objects
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::providers::protocols::{TemplateContext, TemplateRenderer};
//!
//! let renderer = TemplateRenderer::new()?;
//! let context = TemplateContext::new()
//!     .with_config(provider_config)
//!     .with_input("Hello, AI!")
//!     .build();
//!
//! let result = renderer.render("Model: {{config.model}}", &context)?;
//! assert_eq!(result, "Model: gpt-4o");
//! ```

use crate::config::types::provider::ProviderConfig;
use crate::error::{AetherError, Result};
use handlebars::Handlebars;
use serde_json::{json, Value};

/// Builder for creating template context data
///
/// Use the builder pattern to construct a context object with all necessary
/// variables for template rendering.
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    config: Option<Value>,
    input: Option<String>,
    system_prompt: Option<String>,
    messages: Option<Vec<Value>>,
}

impl TemplateContext {
    /// Create a new empty template context
    pub fn new() -> Self {
        Self::default()
    }

    /// Add provider configuration to the context
    ///
    /// The config will be available as `{{config.model}}`, `{{config.temperature}}`, etc.
    pub fn with_config(mut self, config: &ProviderConfig) -> Self {
        // Serialize config to JSON value
        let config_value = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "top_k": config.top_k,
            "frequency_penalty": config.frequency_penalty,
            "presence_penalty": config.presence_penalty,
            "stop_sequences": config.stop_sequences,
            "thinking_level": config.thinking_level,
            "media_resolution": config.media_resolution,
            "repeat_penalty": config.repeat_penalty,
            "system_prompt_mode": config.system_prompt_mode,
        });
        self.config = Some(config_value);
        self
    }

    /// Add user input text to the context
    ///
    /// Available as `{{input}}` in templates.
    pub fn with_input(mut self, input: impl Into<String>) -> Self {
        self.input = Some(input.into());
        self
    }

    /// Add system prompt to the context
    ///
    /// Available as `{{system_prompt}}` in templates.
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    /// Add messages array to the context
    ///
    /// Available as `{{messages}}` in templates.
    pub fn with_messages(mut self, messages: Vec<Value>) -> Self {
        self.messages = Some(messages);
        self
    }

    /// Build the final context as a serde_json::Value
    ///
    /// Returns a JSON object containing all context data.
    pub fn build(self) -> Value {
        json!({
            "config": self.config.unwrap_or(Value::Null),
            "input": self.input.unwrap_or_default(),
            "system_prompt": self.system_prompt.unwrap_or_default(),
            "messages": self.messages.unwrap_or_default(),
        })
    }
}

/// Handlebars template renderer for protocol transformations
///
/// Wraps the Handlebars engine to provide simple template rendering
/// with proper error handling.
pub struct TemplateRenderer {
    handlebars: Handlebars<'static>,
}

impl TemplateRenderer {
    /// Create a new template renderer
    ///
    /// Initializes a Handlebars instance with strict mode disabled
    /// to allow missing variables.
    pub fn new() -> Result<Self> {
        let mut handlebars = Handlebars::new();

        // Disable strict mode to allow missing variables (they'll render as empty strings)
        handlebars.set_strict_mode(false);

        Ok(Self { handlebars })
    }

    /// Render a template string with the given context
    ///
    /// Returns the rendered string or an error if rendering fails.
    ///
    /// # Arguments
    ///
    /// * `template` - Template string with Handlebars syntax (e.g., "{{variable}}")
    /// * `context` - Context data as a serde_json::Value
    ///
    /// # Errors
    ///
    /// Returns `AetherError::ProviderError` if template rendering fails.
    pub fn render(&self, template: &str, context: &Value) -> Result<String> {
        self.handlebars
            .render_template(template, context)
            .map_err(|e| {
                AetherError::provider(format!("Template rendering failed: {}", e))
            })
    }

    /// Render a template and parse the result as JSON
    ///
    /// This is useful for rendering JSON request bodies from templates.
    ///
    /// # Arguments
    ///
    /// * `template` - Template string that should produce valid JSON
    /// * `context` - Context data as a serde_json::Value
    ///
    /// # Errors
    ///
    /// Returns `AetherError::ProviderError` if rendering or JSON parsing fails.
    pub fn render_json(&self, template: &str, context: &Value) -> Result<Value> {
        let rendered = self.render(template, context)?;
        serde_json::from_str(&rendered).map_err(|e| {
            AetherError::provider(format!(
                "Failed to parse rendered template as JSON: {}. Rendered output: {}",
                e, rendered
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::provider::ProviderConfig;

    #[test]
    fn test_template_context_building() {
        // Test building context with all fields
        let config = ProviderConfig::test_config("gpt-4o");
        let context = TemplateContext::new()
            .with_config(&config)
            .with_input("Hello, world!")
            .with_system_prompt("You are a helpful assistant.")
            .with_messages(vec![
                json!({"role": "user", "content": "Test message"}),
            ])
            .build();

        // Verify structure
        assert!(context.is_object());
        let obj = context.as_object().unwrap();

        // Check config
        assert!(obj.contains_key("config"));
        let config_obj = obj["config"].as_object().unwrap();
        assert_eq!(config_obj["model"].as_str(), Some("gpt-4o"));

        // Check input
        assert_eq!(obj["input"].as_str(), Some("Hello, world!"));

        // Check system_prompt
        assert_eq!(obj["system_prompt"].as_str(), Some("You are a helpful assistant."));

        // Check messages
        assert!(obj["messages"].is_array());
        let messages = obj["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"].as_str(), Some("user"));
    }

    #[test]
    fn test_template_context_partial_building() {
        // Test building context with only some fields
        let context = TemplateContext::new()
            .with_input("Partial input")
            .build();

        let obj = context.as_object().unwrap();
        assert_eq!(obj["input"].as_str(), Some("Partial input"));
        assert_eq!(obj["config"], Value::Null);
        assert_eq!(obj["system_prompt"].as_str(), Some(""));
        assert_eq!(obj["messages"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_template_renderer() {
        let renderer = TemplateRenderer::new().unwrap();

        // Test simple variable substitution
        let config = ProviderConfig::test_config("gpt-4o");
        let context = TemplateContext::new()
            .with_config(&config)
            .with_input("Hello!")
            .build();

        let result = renderer.render("Model: {{config.model}}, Input: {{input}}", &context).unwrap();
        assert_eq!(result, "Model: gpt-4o, Input: Hello!");
    }

    #[test]
    fn test_template_renderer_missing_variables() {
        let renderer = TemplateRenderer::new().unwrap();

        // Test that missing variables render as empty strings (strict mode disabled)
        let context = TemplateContext::new().build();

        let result = renderer.render("Config: {{config.model}}", &context).unwrap();
        assert_eq!(result, "Config: ");
    }

    #[test]
    fn test_template_renderer_json() {
        let renderer = TemplateRenderer::new().unwrap();

        // Test rendering JSON template
        let config = ProviderConfig::test_config("gpt-4o");
        let context = TemplateContext::new()
            .with_config(&config)
            .with_input("Hello, AI!")
            .build();

        let template = r#"{"model": "{{config.model}}", "prompt": "{{input}}"}"#;
        let result = renderer.render_json(template, &context).unwrap();

        assert_eq!(result["model"].as_str(), Some("gpt-4o"));
        assert_eq!(result["prompt"].as_str(), Some("Hello, AI!"));
    }

    #[test]
    fn test_template_renderer_json_invalid() {
        let renderer = TemplateRenderer::new().unwrap();

        // Test that invalid JSON is caught
        let context = TemplateContext::new().build();
        let template = r#"{"invalid": json}"#;

        let result = renderer.render_json(template, &context);
        assert!(result.is_err());

        // Verify it's a ProviderError
        match result {
            Err(AetherError::ProviderError { message, .. }) => {
                assert!(message.contains("Failed to parse"));
            }
            _ => panic!("Expected ProviderError"),
        }
    }

    #[test]
    fn test_template_renderer_with_optional_params() {
        let renderer = TemplateRenderer::new().unwrap();

        // Test with optional parameters in config
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.temperature = Some(0.7);
        config.max_tokens = Some(1024);

        let context = TemplateContext::new()
            .with_config(&config)
            .build();

        let template = r#"{"model": "{{config.model}}", "temperature": {{config.temperature}}, "max_tokens": {{config.max_tokens}}}"#;
        let result = renderer.render_json(template, &context).unwrap();

        assert_eq!(result["model"].as_str(), Some("claude-3-5-sonnet"));
        // Use approximate comparison for floating point
        let temp = result["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.01, "Temperature should be ~0.7, got {}", temp);
        assert_eq!(result["max_tokens"].as_u64(), Some(1024));
    }

    #[test]
    fn test_template_renderer_with_messages_array() {
        let renderer = TemplateRenderer::new().unwrap();

        // Test rendering with messages array (for chat completions)
        let messages = vec![
            json!({"role": "system", "content": "You are helpful."}),
            json!({"role": "user", "content": "Hello!"}),
        ];

        let context = TemplateContext::new()
            .with_messages(messages)
            .build();

        // Note: Handlebars needs special syntax for arrays, but we can test basic rendering
        let result = renderer.render("Messages: {{messages}}", &context).unwrap();
        assert!(result.contains("Messages:"));
    }
}
