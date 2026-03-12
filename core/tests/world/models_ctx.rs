//! Models Context for BDD tests
//!
//! Provides shared state for testing models.* and chat.* RPC handlers.

use std::sync::Arc;
use tokio::sync::RwLock;

use alephcore::gateway::handlers::chat::{ClearParams, HistoryParams, SendParams};
use alephcore::gateway::protocol::JsonRpcResponse;
use alephcore::{Config, ProviderConfig};
use serde_json::Value;

/// Models test context
#[derive(Debug, Default)]
pub struct ModelsContext {
    /// Config under test (read-only)
    pub config: Option<Arc<Config>>,
    /// Mutable config for handlers that need write access
    pub mutable_config: Option<Arc<RwLock<Config>>>,
    /// Last JSON-RPC response
    pub response: Option<JsonRpcResponse>,
    /// JSON value for param deserialization
    pub json_value: Option<Value>,
    /// Deserialized SendParams
    pub send_params: Option<SendParams>,
    /// Deserialized HistoryParams
    pub history_params: Option<HistoryParams>,
    /// Deserialized ClearParams
    pub clear_params: Option<ClearParams>,
}

impl ModelsContext {
    /// Create empty config for models testing
    pub fn init_empty_config(&mut self) {
        self.config = Some(Arc::new(Config::default()));
    }

    /// Create config with multiple providers
    pub fn init_config_with_providers(&mut self, providers: Vec<(&str, &str)>) {
        let mut config = Config::default();
        for (name, model) in providers {
            let mut provider_config = ProviderConfig::test_config(model);
            // Set protocol based on provider name
            provider_config.protocol = Some(Self::protocol_for_provider(name));
            config.providers.insert(
                name.to_string(),
                provider_config,
            );
        }
        self.config = Some(Arc::new(config));
    }

    /// Map provider name to protocol
    fn protocol_for_provider(name: &str) -> String {
        match name {
            "openai" => "openai".to_string(),
            "anthropic" | "claude" => "anthropic".to_string(),
            "gemini" => "gemini".to_string(),
            "ollama" => "ollama".to_string(),
            _ => "openai".to_string(),
        }
    }

    /// Create config with enabled and disabled providers
    pub fn init_config_with_mixed_providers(
        &mut self,
        enabled: Vec<(&str, &str)>,
        disabled: Vec<(&str, &str)>,
    ) {
        let mut config = Config::default();
        for (name, model) in enabled {
            let mut provider_config = ProviderConfig::test_config(model);
            provider_config.protocol = Some(Self::protocol_for_provider(name));
            config.providers.insert(
                name.to_string(),
                provider_config,
            );
        }
        for (name, model) in disabled {
            let mut provider_config = ProviderConfig::test_config(model);
            provider_config.protocol = Some(Self::protocol_for_provider(name));
            provider_config.enabled = false;
            config.providers.insert(name.to_string(), provider_config);
        }
        self.config = Some(Arc::new(config));
    }

    /// Set default provider
    pub fn set_default_provider(&mut self, provider: &str) {
        let config = Arc::make_mut(self.config.as_mut().expect("Config not initialized"));
        config.general.default_provider = Some(provider.to_string());
    }

    /// Create mutable config with multiple providers for handlers that need write access
    pub fn init_mutable_config_with_providers(&mut self, providers: Vec<(&str, &str)>) {
        let mut config = Config::default();
        for (name, model) in providers {
            let mut provider_config = ProviderConfig::test_config(model);
            provider_config.protocol = Some(Self::protocol_for_provider(name));
            config.providers.insert(name.to_string(), provider_config);
        }
        self.mutable_config = Some(Arc::new(RwLock::new(config)));
    }

    /// Get mutable config
    pub fn get_mutable_config(&self) -> Arc<RwLock<Config>> {
        self.mutable_config.clone().expect("Mutable config not initialized")
    }

    /// Get config
    pub fn get_config(&self) -> Arc<Config> {
        self.config.clone().expect("Config not initialized")
    }

    /// Check if response is successful
    pub fn is_response_successful(&self) -> bool {
        self.response
            .as_ref()
            .map(|r| r.result.is_some() && r.error.is_none())
            .unwrap_or(false)
    }

    /// Get response result
    pub fn get_result(&self) -> Option<&Value> {
        self.response.as_ref().and_then(|r| r.result.as_ref())
    }

    /// Get response error
    pub fn get_error(&self) -> Option<&alephcore::gateway::protocol::JsonRpcError> {
        self.response.as_ref().and_then(|r| r.error.as_ref())
    }

    /// Get models array from response
    pub fn get_models_array(&self) -> Option<&Vec<Value>> {
        self.get_result()
            .and_then(|r| r.get("models"))
            .and_then(|m| m.as_array())
    }

    /// Get single model from response
    pub fn get_model(&self) -> Option<&Value> {
        self.get_result().and_then(|r| r.get("model"))
    }

    /// Get capabilities from response
    pub fn get_capabilities(&self) -> Option<&Vec<Value>> {
        self.get_result()
            .and_then(|r| r.get("capabilities"))
            .and_then(|c| c.as_array())
    }
}
