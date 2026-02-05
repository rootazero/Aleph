//! Protocol Context for BDD tests
//!
//! Provides shared state for testing the configurable protocol system.

use std::sync::Arc;

use alephcore::providers::adapter::{ProtocolAdapter, RequestPayload};
use alephcore::providers::protocols::{ConfigurableProtocol, ProtocolDefinition};
use alephcore::providers::AiProvider;
use std::sync::Arc as StdArc;
use alephcore::ProviderConfig;
use tempfile::TempDir;

/// Protocol test context
#[derive(Default)]
pub struct ProtocolContext {
    /// YAML content for protocol definition
    pub yaml_content: Option<String>,
    /// Parsed protocol definition
    pub protocol_def: Option<ProtocolDefinition>,
    /// Created ConfigurableProtocol
    pub protocol: Option<Arc<dyn ProtocolAdapter>>,
    /// Protocol name for registry operations
    pub protocol_name: Option<String>,
    /// Created provider
    pub provider: Option<StdArc<dyn AiProvider>>,
    /// Provider config for testing
    pub provider_config: Option<ProviderConfig>,
    /// Last operation result
    pub last_result: Option<Result<(), String>>,
    /// Request build result
    pub request_result: Option<Result<(), String>>,
    /// Temporary directory for file-based tests
    pub temp_dir: Option<TempDir>,
    /// Multiple protocol names for batch testing
    pub protocol_names: Vec<String>,
    /// Multiple protocol definitions
    pub protocol_defs: Vec<ProtocolDefinition>,
}

impl std::fmt::Debug for ProtocolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtocolContext")
            .field("yaml_content", &self.yaml_content.as_ref().map(|_| "..."))
            .field("protocol_def", &self.protocol_def)
            .field("protocol", &self.protocol.as_ref().map(|_| "dyn ProtocolAdapter"))
            .field("protocol_name", &self.protocol_name)
            .field("provider", &self.provider.as_ref().map(|_| "dyn AIProvider"))
            .field("provider_config", &self.provider_config)
            .field("last_result", &self.last_result)
            .field("request_result", &self.request_result)
            .field("temp_dir", &self.temp_dir.as_ref().map(|_| "TempDir"))
            .field("protocol_names", &self.protocol_names)
            .finish()
    }
}

impl ProtocolContext {
    /// Initialize the global protocol registry
    pub fn init_registry() {
        use alephcore::providers::protocols::ProtocolRegistry;
        let registry = ProtocolRegistry::global();
        if registry.list_protocols().is_empty() {
            registry.register_builtin();
        }
    }

    /// Set YAML content for parsing
    pub fn set_yaml(&mut self, yaml: &str) {
        self.yaml_content = Some(yaml.to_string());
    }

    /// Parse the YAML into a ProtocolDefinition
    pub fn parse_yaml(&mut self) -> Result<(), String> {
        let yaml = self.yaml_content.as_ref().ok_or("No YAML content set")?;
        match serde_yaml::from_str::<ProtocolDefinition>(yaml) {
            Ok(def) => {
                self.protocol_name = Some(def.name.clone());
                self.protocol_def = Some(def);
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Create a ConfigurableProtocol from the definition
    pub fn create_protocol(&mut self) -> Result<(), String> {
        let def = self.protocol_def.clone().ok_or("No protocol definition")?;
        match ConfigurableProtocol::new(def, reqwest::Client::new()) {
            Ok(p) => {
                self.protocol = Some(Arc::new(p));
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Register protocol in the global registry
    pub fn register_protocol(&mut self) -> Result<(), String> {
        use alephcore::providers::protocols::ProtocolRegistry;

        let name = self.protocol_name.clone().ok_or("No protocol name")?;
        let protocol = self.protocol.clone().ok_or("No protocol created")?;

        ProtocolRegistry::global()
            .register(name, protocol)
            .map_err(|e| e.to_string())
    }

    /// Unregister protocol from the global registry
    pub fn unregister_protocol(&mut self, name: &str) {
        use alephcore::providers::protocols::ProtocolRegistry;
        ProtocolRegistry::global().unregister(name);
    }

    /// Check if protocol is in registry
    pub fn is_protocol_registered(&self, name: &str) -> bool {
        use alephcore::providers::protocols::ProtocolRegistry;
        ProtocolRegistry::global().get(name).is_some()
    }

    /// Create provider using protocol
    pub fn create_provider(&mut self, protocol_name: &str) -> Result<(), String> {
        use alephcore::providers::create_provider;

        let mut config = self.provider_config.clone()
            .unwrap_or_else(|| ProviderConfig::test_config("test-model"));
        config.protocol = Some(protocol_name.to_string());
        if config.api_key.is_none() {
            config.api_key = Some("test-key".to_string());
        }
        if config.base_url.is_none() {
            config.base_url = Some(format!("https://api.{}.com", protocol_name));
        }

        match create_provider(protocol_name, config) {
            Ok(p) => {
                self.provider = Some(p);
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Build a request with the protocol
    pub fn build_request(&mut self) -> Result<(), String> {
        use alephcore::providers::protocols::ProtocolRegistry;

        let name = self.protocol_name.clone().ok_or("No protocol name")?;
        let protocol = ProtocolRegistry::global()
            .get(&name)
            .ok_or(format!("Protocol {} not in registry", name))?;

        let payload = RequestPayload::new("Test input");
        let mut config = self.provider_config.clone()
            .unwrap_or_else(|| ProviderConfig::test_config("test-model"));
        config.protocol = Some(name);
        config.api_key = Some("test-key".to_string());
        if config.base_url.is_none() {
            config.base_url = Some("https://api.test.com".to_string());
        }

        match protocol.build_request(&payload, &config, false) {
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Set provider config values
    pub fn configure_provider(&mut self, model: &str, max_tokens: Option<u32>, temperature: Option<f32>) {
        let mut config = ProviderConfig::test_config(model);
        config.max_tokens = max_tokens;
        config.temperature = temperature;
        self.provider_config = Some(config);
    }

    /// Create temp dir for file-based tests
    pub fn create_temp_dir(&mut self) -> &TempDir {
        self.temp_dir.get_or_insert_with(|| tempfile::tempdir().unwrap())
    }

    /// Cleanup registered protocols
    pub fn cleanup_protocols(&mut self) {
        use alephcore::providers::protocols::ProtocolRegistry;

        let names_to_cleanup: Vec<String> = self.protocol_names.drain(..).collect();
        for name in names_to_cleanup {
            ProtocolRegistry::global().unregister(&name);
        }
        if let Some(name) = self.protocol_name.take() {
            ProtocolRegistry::global().unregister(&name);
        }
    }
}

impl Drop for ProtocolContext {
    fn drop(&mut self) {
        self.cleanup_protocols();
    }
}
