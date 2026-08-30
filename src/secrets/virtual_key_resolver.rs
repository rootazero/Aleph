//! Virtual-key alias decorator for `AsyncSecretResolver`.
//!
//! Maps operator-defined aliases (`[secrets_config].virtual_keys`, an
//! alias -> `secret_name` map) to their real secret names before delegating to
//! the inner resolver. A `{{secret:ALIAS}}` placeholder therefore resolves to
//! the secret stored under the mapped name. Names without a matching alias
//! pass through unchanged, so this is a transparent, opt-in layer.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::secrets::injection::AsyncSecretResolver;
use crate::secrets::types::{DecryptedSecret, SecretError};
use crate::sync_primitives::Arc;

/// Resolver decorator that applies a virtual-key alias map before delegation.
pub struct VirtualKeyResolver {
    inner: Arc<dyn AsyncSecretResolver>,
    aliases: HashMap<String, String>,
}

impl VirtualKeyResolver {
    /// Wrap `inner`, translating any name present in `aliases` to its mapped
    /// secret name. With an empty map this is a pure pass-through.
    pub fn new(inner: Arc<dyn AsyncSecretResolver>, aliases: HashMap<String, String>) -> Self {
        Self { inner, aliases }
    }
}

#[async_trait]
impl AsyncSecretResolver for VirtualKeyResolver {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        let resolved = self.aliases.get(name).map_or(name, String::as_str);
        self.inner.resolve(resolved).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubResolver;

    #[async_trait]
    impl AsyncSecretResolver for StubResolver {
        async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
            // Echo the resolved name back as the secret value so tests can
            // assert which name the alias layer forwarded.
            Ok(DecryptedSecret::new(format!("value-for:{name}")))
        }
    }

    #[tokio::test]
    async fn maps_alias_to_real_secret_name() {
        let mut aliases = HashMap::new();
        aliases.insert("openai".to_string(), "prod_openai_key".to_string());
        let resolver = VirtualKeyResolver::new(Arc::new(StubResolver), aliases);

        let decrypted = resolver.resolve("openai").await.unwrap();
        assert_eq!(decrypted.expose(), "value-for:prod_openai_key");
    }

    #[tokio::test]
    async fn passes_through_unmapped_name() {
        let resolver = VirtualKeyResolver::new(Arc::new(StubResolver), HashMap::new());
        let decrypted = resolver.resolve("literal_name").await.unwrap();
        assert_eq!(decrypted.expose(), "value-for:literal_name");
    }
}
