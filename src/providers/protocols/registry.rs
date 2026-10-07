//! Protocol registry for dynamic protocol management

use crate::error::Result;
use crate::providers::adapter::ProtocolAdapter;
use crate::providers::protocols::openai_responses::ResponsesVariant;
use crate::providers::protocols::{
    AnthropicProtocol, GeminiProtocol, OpenAiProtocol, OpenAiResponsesProtocol,
};
use crate::sync_primitives::{Arc, RwLock};
use once_cell::sync::Lazy;
use reqwest::Client;
use std::collections::HashMap;

/// Acquire a write lock, recovering from poisoning.
///
/// Centralises the recovery policy: a poisoned mutex is a signal that a prior
/// holder panicked mid-update, and the lock is still usable. The standard
/// library's `Mutex::lock().unwrap_or_else(|e| e.into_inner())` does the same
/// thing inline; this helper keeps the recovery decision in one place so future
/// audits of "do we want to panic on poison instead?" only touch this site.
fn write_recover<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|e| e.into_inner())
}

/// Acquire a read lock, recovering from poisoning. See [`write_recover`] for
/// the rationale.
fn read_recover<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|e| e.into_inner())
}

/// Global protocol registry instance (built-in protocols registered at init time)
pub static PROTOCOL_REGISTRY: Lazy<ProtocolRegistry> = Lazy::new(|| {
    let registry = ProtocolRegistry::new();
    registry.register_builtin();
    registry
});

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
    #[must_use]
    pub fn new() -> Self {
        Self {
            dynamic: RwLock::new(HashMap::new()),
            builtin: RwLock::new(HashMap::new()),
        }
    }

    /// Get the global registry instance
    #[must_use]
    pub fn global() -> &'static Self {
        &PROTOCOL_REGISTRY
    }

    /// Register built-in protocols
    pub fn register_builtin(&self) {
        let mut builtin = write_recover(&self.builtin);

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

        // Codex variant — same wire format as Responses API, different endpoint + fields
        let codex_factory: ProtocolFactory = |client| {
            Arc::new(OpenAiResponsesProtocol::new(
                client,
                ResponsesVariant::codex(),
            )) as Arc<dyn ProtocolAdapter>
        };
        builtin.insert("codex".to_string(), codex_factory);
        // Backward compatibility: "chatgpt" maps to the same Codex protocol
        builtin.insert("chatgpt".to_string(), codex_factory);

        builtin.insert(
            "openai-responses".to_string(),
            (|client| {
                Arc::new(OpenAiResponsesProtocol::new(
                    client,
                    ResponsesVariant::default(),
                )) as Arc<dyn ProtocolAdapter>
            }) as ProtocolFactory,
        );
    }

    /// Register a dynamic protocol
    pub fn register(&self, name: String, protocol: Arc<dyn ProtocolAdapter>) -> Result<()> {
        write_recover(&self.dynamic).insert(name, protocol);
        Ok(())
    }

    /// Unregister a dynamic protocol
    pub fn unregister(&self, name: &str) {
        write_recover(&self.dynamic).remove(name);
    }

    /// Get a protocol by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn ProtocolAdapter>> {
        // 1. Check dynamic protocols first
        if let Some(protocol) = read_recover(&self.dynamic).get(name) {
            return Some(protocol.clone());
        }

        // 2. Fall back to built-in protocols
        read_recover(&self.builtin).get(name).map(|factory| {
            let client = crate::providers::protocols::http_client::build_provider_http_client();
            factory(client)
        })
    }

    /// List all available protocol names
    pub fn list_protocols(&self) -> Vec<String> {
        let mut protocols: Vec<String> = read_recover(&self.builtin).keys().cloned().collect();
        protocols.extend(read_recover(&self.dynamic).keys().cloned());
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
    fn test_codex_protocol_registered() {
        let registry = ProtocolRegistry::new();
        registry.register_builtin();
        assert!(
            registry.get("codex").is_some(),
            "codex protocol should be registered"
        );
        // Backward compatibility
        assert!(
            registry.get("chatgpt").is_some(),
            "chatgpt alias should still work"
        );
    }

    #[test]
    fn test_openai_responses_protocol_registered() {
        let registry = ProtocolRegistry::new();
        registry.register_builtin();
        assert!(registry.get("openai-responses").is_some());
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
