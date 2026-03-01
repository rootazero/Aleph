//! Host-side secret injection pipeline.
//!
//! Resolves `{{secret:NAME}}` placeholders at the host boundary
//! just before outbound requests. The resolved values are tracked
//! for downstream leak detection.

use std::hash::{Hash, Hasher};

use super::placeholder::extract_secret_refs;
use super::router::AsyncSecretResolver;
use super::types::SecretError;

/// Record of a secret injected during rendering.
///
/// Contains hashes and metadata only — never plaintext secret values.
#[derive(Debug, Clone)]
pub struct InjectedSecret {
    pub name: String,
    pub value_hash: u64,
    pub value_len: usize,
}

impl InjectedSecret {
    fn from_value(name: &str, value: &str) -> Self {
        // Use non-zero fixed keys to avoid trivial rainbow table attacks
        let mut hasher = siphasher::sip::SipHasher::new_with_keys(
            0x517c_c1b7_2722_0a95,
            0x6c62_272e_07bb_0142,
        );
        value.hash(&mut hasher);
        let hash = hasher.finish();

        Self {
            name: name.to_string(),
            value_hash: hash,
            value_len: value.len(),
        }
    }
}

/// Render a string by replacing all `{{secret:NAME}}` placeholders.
///
/// Returns the rendered string and a list of injected secrets
/// (with hashes, never plaintext) for downstream leak detection.
pub async fn render_with_secrets(
    input: &str,
    resolver: &dyn AsyncSecretResolver,
) -> Result<(String, Vec<InjectedSecret>), SecretError> {
    let refs = extract_secret_refs(input)?;

    if refs.is_empty() {
        return Ok((input.to_string(), vec![]));
    }

    let mut result = input.to_string();
    let mut injected = Vec::with_capacity(refs.len());

    for secret_ref in &refs {
        let decrypted = resolver.resolve(&secret_ref.name).await?;
        let value = decrypted.expose();

        injected.push(InjectedSecret::from_value(&secret_ref.name, value));
        result = result.replace(&secret_ref.raw, value);
    }

    Ok((result, injected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::DecryptedSecret;

    struct MockResolver {
        secrets: std::collections::HashMap<String, String>,
    }

    impl MockResolver {
        fn new() -> Self {
            Self {
                secrets: std::collections::HashMap::new(),
            }
        }
        fn with(mut self, name: &str, value: &str) -> Self {
            self.secrets.insert(name.to_string(), value.to_string());
            self
        }
    }

    #[async_trait::async_trait]
    impl super::super::router::AsyncSecretResolver for MockResolver {
        async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
            self.secrets
                .get(name)
                .map(|v| DecryptedSecret::new(v.clone()))
                .ok_or_else(|| SecretError::NotFound(name.to_string()))
        }
    }

    #[tokio::test]
    async fn test_render_replaces_placeholder() {
        let resolver = MockResolver::new().with("api_key", "sk-ant-secret-123");
        let input = "Authorization: Bearer {{secret:api_key}}";
        let (rendered, injected) = render_with_secrets(input, &resolver).await.unwrap();
        assert_eq!(rendered, "Authorization: Bearer sk-ant-secret-123");
        assert_eq!(injected.len(), 1);
        assert_eq!(injected[0].name, "api_key");
        assert!(!rendered.contains("{{secret:"));
    }

    #[tokio::test]
    async fn test_render_multiple_placeholders() {
        let resolver = MockResolver::new()
            .with("key1", "value1")
            .with("key2", "value2");
        let input = "{{secret:key1}} and {{secret:key2}}";
        let (rendered, injected) = render_with_secrets(input, &resolver).await.unwrap();
        assert_eq!(rendered, "value1 and value2");
        assert_eq!(injected.len(), 2);
    }

    #[tokio::test]
    async fn test_render_no_placeholders() {
        let resolver = MockResolver::new();
        let input = "Just plain text";
        let (rendered, injected) = render_with_secrets(input, &resolver).await.unwrap();
        assert_eq!(rendered, "Just plain text");
        assert!(injected.is_empty());
    }

    #[tokio::test]
    async fn test_render_missing_secret_returns_error() {
        let resolver = MockResolver::new();
        let input = "Bearer {{secret:nonexistent}}";
        let result = render_with_secrets(input, &resolver).await;
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_injected_secret_tracks_hash_not_value() {
        let resolver = MockResolver::new().with("key", "my-secret-value");
        let (_, injected) = render_with_secrets("{{secret:key}}", &resolver).await.unwrap();
        let record = &injected[0];
        assert_eq!(record.name, "key");
        assert_eq!(record.value_len, "my-secret-value".len());
        assert_ne!(record.value_hash, 0);
        // prefix field removed — InjectedSecret must never store plaintext
    }

    #[tokio::test]
    async fn test_render_preserves_surrounding_text() {
        let resolver = MockResolver::new().with("token", "abc123");
        let input = "before {{secret:token}} after";
        let (rendered, _) = render_with_secrets(input, &resolver).await.unwrap();
        assert_eq!(rendered, "before abc123 after");
    }
}
