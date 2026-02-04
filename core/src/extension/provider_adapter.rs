//! Provider adapter for plugin-provided AI models
//!
//! This module provides an adapter that wraps plugin-provided AI model providers
//! to interface with the core Aether systems. It allows plugins to expose
//! custom AI backends (e.g., local LLMs, custom API endpoints) that can be
//! used by the agent system.
//!
//! # Architecture
//!
//! ```text
//! Core Agent System
//!        │
//!        ▼
//! PluginProviderAdapter  (this module)
//!        │
//!        ├── ProviderRegistration (metadata)
//!        │
//!        ▼
//! PluginLoader → Plugin Runtime (Node.js/WASM)
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::extension::{PluginProviderAdapter, ProviderChatRequest};
//!
//! // Create adapter from a provider registration
//! let adapter = PluginProviderAdapter::new(registration, loader);
//!
//! // List available models
//! let models = adapter.list_models().await?;
//!
//! // Generate a chat completion
//! let request = ProviderChatRequest {
//!     model: "my-model".to_string(),
//!     messages: vec![...],
//!     temperature: Some(0.7),
//!     max_tokens: Some(1000),
//!     stream: false,
//! };
//! let response = adapter.chat(request).await?;
//! ```

use std::sync::Arc;
use tokio::sync::RwLock;

use super::plugin_loader::PluginLoader;
use super::registry::ProviderRegistration;
use super::types::{ProviderChatRequest, ProviderChatResponse, ProviderModelInfo};
use super::ExtensionResult;

/// Adapter that wraps a plugin provider to interface with core systems.
///
/// The `PluginProviderAdapter` provides a unified interface for interacting with
/// AI model providers implemented by plugins. It handles:
///
/// - Model discovery (listing available models)
/// - Chat completions (non-streaming)
/// - Future: Streaming support with async generators
///
/// # Thread Safety
///
/// The adapter holds an `Arc<RwLock<PluginLoader>>` to allow concurrent read access
/// to provider metadata while ensuring exclusive access during plugin IPC calls.
/// Chat and model listing operations require a write lock on the loader because
/// they involve IPC with the plugin runtime.
pub struct PluginProviderAdapter {
    /// Provider registration containing metadata
    registration: ProviderRegistration,
    /// Shared plugin loader for IPC calls
    loader: Arc<RwLock<PluginLoader>>,
}

impl PluginProviderAdapter {
    /// Create a new provider adapter.
    ///
    /// # Arguments
    ///
    /// * `registration` - The provider registration from the plugin
    /// * `loader` - Shared plugin loader for making IPC calls
    pub fn new(registration: ProviderRegistration, loader: Arc<RwLock<PluginLoader>>) -> Self {
        Self {
            registration,
            loader,
        }
    }

    /// Get the provider ID.
    ///
    /// This is the unique identifier for the provider (e.g., "anthropic", "openai").
    pub fn id(&self) -> &str {
        &self.registration.id
    }

    /// Get the provider display name.
    ///
    /// This is the human-readable name for the provider (e.g., "Anthropic", "OpenAI").
    pub fn name(&self) -> &str {
        &self.registration.name
    }

    /// Get the plugin ID that provides this provider.
    pub fn plugin_id(&self) -> &str {
        &self.registration.plugin_id
    }

    /// Get the list of static model IDs from the registration.
    ///
    /// This returns the models declared in the provider registration.
    /// For dynamic model discovery, use [`list_models`](#method.list_models).
    pub fn static_models(&self) -> &[String] {
        &self.registration.models
    }

    /// Get the provider registration.
    pub fn registration(&self) -> &ProviderRegistration {
        &self.registration
    }

    /// List available models from the provider.
    ///
    /// This method calls the plugin's `listModels` handler to dynamically
    /// discover available models. If the handler fails or is not implemented,
    /// it falls back to the static models from the registration.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<ProviderModelInfo>)` - List of available models
    /// * `Err(ExtensionError)` - If the plugin call fails and no fallback is available
    pub async fn list_models(&self) -> ExtensionResult<Vec<ProviderModelInfo>> {
        // Try to call plugin's listModels handler
        let result = {
            let mut loader = self.loader.write().await;
            loader.call_tool(
                &self.registration.plugin_id,
                "listModels",
                serde_json::json!({
                    "provider_id": self.registration.id
                }),
            )
        };

        match result {
            Ok(value) => {
                // Try to parse the response as Vec<ProviderModelInfo>
                match serde_json::from_value::<Vec<ProviderModelInfo>>(value) {
                    Ok(models) => Ok(models),
                    Err(_) => {
                        // Parse failed, fall back to static models
                        tracing::warn!(
                            "Failed to parse listModels response for provider '{}', using static models",
                            self.registration.id
                        );
                        Ok(self.build_static_model_info())
                    }
                }
            }
            Err(e) => {
                // Handler failed or not implemented, fall back to static models
                tracing::debug!(
                    "listModels handler failed for provider '{}': {}, using static models",
                    self.registration.id,
                    e
                );
                Ok(self.build_static_model_info())
            }
        }
    }

    /// Generate a chat completion (non-streaming).
    ///
    /// This method sends a chat completion request to the plugin provider
    /// and returns the complete response.
    ///
    /// # Arguments
    ///
    /// * `request` - The chat request containing model, messages, and parameters
    ///
    /// # Returns
    ///
    /// * `Ok(ProviderChatResponse)` - The completion response
    /// * `Err(ExtensionError)` - If the plugin call fails
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let request = ProviderChatRequest {
    ///     model: "gpt-4".to_string(),
    ///     messages: vec![
    ///         ProviderMessage { role: "user".to_string(), content: "Hello!".to_string() }
    ///     ],
    ///     temperature: Some(0.7),
    ///     max_tokens: Some(1000),
    ///     stream: false,
    /// };
    /// let response = adapter.chat(request).await?;
    /// println!("Response: {}", response.content);
    /// ```
    pub async fn chat(&self, request: ProviderChatRequest) -> ExtensionResult<ProviderChatResponse> {
        // Serialize request to JSON
        let request_json = serde_json::to_value(&request)
            .map_err(|e| super::ExtensionError::Runtime(format!("Failed to serialize request: {}", e)))?;

        // Call plugin's chat handler
        let result = {
            let mut loader = self.loader.write().await;
            loader.call_tool(&self.registration.plugin_id, "chat", request_json)?
        };

        // Parse response
        serde_json::from_value(result)
            .map_err(|e| super::ExtensionError::Runtime(format!("Failed to parse chat response: {}", e)))
    }

    /// Check if a specific model is supported by this provider.
    ///
    /// This checks against the static model list from the registration.
    pub fn supports_model(&self, model_id: &str) -> bool {
        self.registration.models.iter().any(|m| m == model_id)
    }

    /// Build ProviderModelInfo from static model IDs.
    ///
    /// This creates basic model info from the model IDs in the registration.
    /// Dynamic properties (context window, capabilities) are set to defaults.
    fn build_static_model_info(&self) -> Vec<ProviderModelInfo> {
        self.registration
            .models
            .iter()
            .map(|model_id: &String| ProviderModelInfo {
                id: model_id.clone(),
                display_name: model_id.clone(),
                context_window: None,
                supports_tools: false,
                supports_vision: false,
            })
            .collect()
    }

    // =========================================================================
    // Future: Streaming Support
    // =========================================================================

    // TODO: Add streaming support with async generators
    //
    // The streaming API will look something like:
    //
    // ```rust,ignore
    // pub async fn chat_stream(&self, request: ProviderChatRequest)
    //     -> ExtensionResult<impl Stream<Item = ProviderStreamChunk>>
    // {
    //     // Implementation will use async channels to stream chunks
    //     // from the plugin runtime to the caller
    // }
    // ```
    //
    // This requires:
    // 1. PluginLoader support for streaming IPC
    // 2. Node.js runtime support for streaming responses
    // 3. Proper backpressure handling
}

impl std::fmt::Debug for PluginProviderAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginProviderAdapter")
            .field("id", &self.registration.id)
            .field("name", &self.registration.name)
            .field("plugin_id", &self.registration.plugin_id)
            .field("models", &self.registration.models)
            .finish()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_registration() -> ProviderRegistration {
        ProviderRegistration {
            id: "test-provider".to_string(),
            name: "Test Provider".to_string(),
            models: vec![
                "model-a".to_string(),
                "model-b".to_string(),
                "model-c".to_string(),
            ],
            plugin_id: "test-plugin".to_string(),
        }
    }

    #[test]
    fn test_provider_adapter_creation() {
        let registration = create_test_registration();
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let adapter = PluginProviderAdapter::new(registration.clone(), loader);

        assert_eq!(adapter.id(), "test-provider");
        assert_eq!(adapter.name(), "Test Provider");
        assert_eq!(adapter.plugin_id(), "test-plugin");
        assert_eq!(adapter.static_models().len(), 3);
    }

    #[test]
    fn test_provider_adapter_supports_model() {
        let registration = create_test_registration();
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let adapter = PluginProviderAdapter::new(registration, loader);

        assert!(adapter.supports_model("model-a"));
        assert!(adapter.supports_model("model-b"));
        assert!(adapter.supports_model("model-c"));
        assert!(!adapter.supports_model("model-d"));
        assert!(!adapter.supports_model("nonexistent"));
    }

    #[test]
    fn test_provider_adapter_static_model_info() {
        let registration = create_test_registration();
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let adapter = PluginProviderAdapter::new(registration, loader);

        let model_info = adapter.build_static_model_info();
        assert_eq!(model_info.len(), 3);

        // Check first model
        assert_eq!(model_info[0].id, "model-a");
        assert_eq!(model_info[0].display_name, "model-a");
        assert!(model_info[0].context_window.is_none());
        assert!(!model_info[0].supports_tools);
        assert!(!model_info[0].supports_vision);
    }

    #[test]
    fn test_provider_adapter_debug() {
        let registration = create_test_registration();
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let adapter = PluginProviderAdapter::new(registration, loader);

        let debug_str = format!("{:?}", adapter);
        assert!(debug_str.contains("test-provider"));
        assert!(debug_str.contains("Test Provider"));
        assert!(debug_str.contains("test-plugin"));
    }

    #[test]
    fn test_provider_adapter_registration_access() {
        let registration = create_test_registration();
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let adapter = PluginProviderAdapter::new(registration.clone(), loader);

        let reg = adapter.registration();
        assert_eq!(reg.id, registration.id);
        assert_eq!(reg.name, registration.name);
        assert_eq!(reg.plugin_id, registration.plugin_id);
        assert_eq!(reg.models, registration.models);
    }

    #[tokio::test]
    async fn test_provider_adapter_list_models_fallback() {
        let registration = create_test_registration();
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let adapter = PluginProviderAdapter::new(registration, loader);

        // Since no plugin is loaded, list_models should fall back to static models
        let models = adapter.list_models().await.unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "model-a");
        assert_eq!(models[1].id, "model-b");
        assert_eq!(models[2].id, "model-c");
    }

    #[tokio::test]
    async fn test_provider_adapter_chat_no_plugin() {
        let registration = create_test_registration();
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let adapter = PluginProviderAdapter::new(registration, loader);

        let request = ProviderChatRequest {
            model: "model-a".to_string(),
            messages: vec![super::super::types::ProviderMessage {
                role: "user".to_string(),
                content: "Hello!".to_string(),
            }],
            temperature: Some(0.7),
            max_tokens: Some(1000),
            stream: false,
        };

        // Since no plugin is loaded, chat should fail with PluginNotFound
        let result = adapter.chat(request).await;
        assert!(result.is_err());
    }
}
