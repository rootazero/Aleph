//! Protocol registry for dynamic protocol management

use crate::error::Result;
use crate::providers::adapter::ProtocolAdapter;
use crate::providers::protocols::{AnthropicProtocol, GeminiProtocol, OpenAiProtocol};
use once_cell::sync::Lazy;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Global protocol registry instance
pub static PROTOCOL_REGISTRY: Lazy<ProtocolRegistry> = Lazy::new(ProtocolRegistry::new);

/// Protocol factory function type
type ProtocolFactory = fn(Client) -> Arc<dyn ProtocolAdapter>;

/// Protocol registry manages all available protocol adapters
pub struct ProtocolRegistry {
    /// Dynamically registered protocols (from YAML configs)
    dynamic: RwLock<HashMap<String, Arc<dyn ProtocolAdapter>>>,

    /// Built-in protocol factories
    builtin: RwLock<HashMap<String, ProtocolFactory>>,
}

impl ProtocolRegistry {
    /// Create a new protocol registry
    pub fn new() -> Self {
        Self {
            dynamic: RwLock::new(HashMap::new()),
            builtin: RwLock::new(HashMap::new()),
        }
    }

    /// Get the global registry instance
    pub fn global() -> &'static Self {
        &PROTOCOL_REGISTRY
    }

    /// Register built-in protocols
    pub fn register_builtin(&self) {
        let mut builtin = self.builtin.write().unwrap();

        builtin.insert(
            "openai".to_string(),
            (|client| Arc::new(OpenAiProtocol::new(client)) as Arc<dyn ProtocolAdapter>)
                as ProtocolFactory,
        );

        builtin.insert(
            "anthropic".to_string(),
            (|client| Arc::new(AnthropicProtocol::new(client)) as Arc<dyn ProtocolAdapter>)
                as ProtocolFactory,
        );

        builtin.insert(
            "gemini".to_string(),
            (|client| Arc::new(GeminiProtocol::new(client)) as Arc<dyn ProtocolAdapter>)
                as ProtocolFactory,
        );
    }

    /// Register a dynamic protocol
    pub fn register(&self, name: String, protocol: Arc<dyn ProtocolAdapter>) -> Result<()> {
        self.dynamic.write().unwrap().insert(name, protocol);
        Ok(())
    }

    /// Unregister a dynamic protocol
    pub fn unregister(&self, name: &str) {
        self.dynamic.write().unwrap().remove(name);
    }

    /// Get a protocol by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn ProtocolAdapter>> {
        // 1. Check dynamic protocols first
        if let Some(protocol) = self.dynamic.read().unwrap().get(name) {
            return Some(protocol.clone());
        }

        // 2. Fall back to built-in protocols
        self.builtin.read().unwrap().get(name).map(|factory| {
            let client = Client::new();
            factory(client)
        })
    }

    /// List all available protocol names
    pub fn list_protocols(&self) -> Vec<String> {
        let mut protocols: Vec<String> = self.builtin.read().unwrap().keys().cloned().collect();
        protocols.extend(self.dynamic.read().unwrap().keys().cloned());
        protocols.sort();
        protocols
    }
}

impl Default for ProtocolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::protocols::{AnthropicProtocol, GeminiProtocol, OpenAiProtocol};

    #[test]
    fn test_register_and_get_builtin() {
        let registry = ProtocolRegistry::new();
        registry.register_builtin();

        assert!(registry.get("openai").is_some());
        assert!(registry.get("anthropic").is_some());
        assert!(registry.get("gemini").is_some());
        assert!(registry.get("unknown").is_none());
    }

    #[test]
    fn test_list_protocols() {
        let registry = ProtocolRegistry::new();
        registry.register_builtin();

        let protocols = registry.list_protocols();
        assert!(protocols.contains(&"openai".to_string()));
        assert!(protocols.contains(&"anthropic".to_string()));
        assert!(protocols.contains(&"gemini".to_string()));
    }
}
