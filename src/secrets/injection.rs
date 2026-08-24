//! Host-side secret injection pipeline.
//!
//! Resolves `{{secret:NAME}}` placeholders at the host boundary
//! just before outbound requests. The resolved values are tracked
//! for downstream leak detection.

use std::hash::{Hash, Hasher};

use super::placeholder::extract_secret_refs;
use super::types::{DecryptedSecret, SecretError};

pub(crate) const INJECTED_HASH_KEY0: u64 = 0x517c_c1b7_2722_0a95;
pub(crate) const INJECTED_HASH_KEY1: u64 = 0x6c62_272e_07bb_0142;

/// Trait for resolving secret names to decrypted values.
#[async_trait::async_trait]
pub trait AsyncSecretResolver: Send + Sync {
    /// Resolve a secret name to its decrypted value.
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError>;
}

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
    #[must_use]
    pub fn from_value(name: &str, value: &str) -> Self {
        let mut hasher =
            siphasher::sip::SipHasher::new_with_keys(INJECTED_HASH_KEY0, INJECTED_HASH_KEY1);
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
///
/// A prompt that legitimately uses `{{secret:NAME}}` five times previously
/// decrypted the same vault entry five times per render. This implementation
/// makes one resolve per unique name — the resulting `DecryptedSecret` is
/// retained in a small per-render map and `expose()`d once per occurrence,
/// and one `InjectedSecret` is emitted per unique name (downstream
/// fingerprint registration is per-name, not per-occurrence).
pub async fn render_with_secrets(
    input: &str,
    resolver: &dyn AsyncSecretResolver,
) -> Result<(String, Vec<InjectedSecret>), SecretError> {
    let refs = extract_secret_refs(input)?;

    if refs.is_empty() {
        return Ok((input.to_string(), Vec::new()));
    }

    // Phase 1: resolve each unique name exactly once. Order of first
    // occurrence is preserved so the returned `injected` Vec is
    // deterministic for the same input.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut unique_order: Vec<&str> = Vec::new();
    for r in &refs {
        if seen.insert(r.name.as_str()) {
            unique_order.push(r.name.as_str());
        }
    }

    // `resolved_storage` owns the `DecryptedSecret`s for the duration of
    // the call; the borrow map below points into it. Drop the storage
    // (and zeroize via `secrecy`) on function return.
    let mut resolved_storage: Vec<DecryptedSecret> = Vec::with_capacity(unique_order.len());
    let mut injected = Vec::with_capacity(unique_order.len());
    for name in &unique_order {
        let decrypted = resolver.resolve(name).await?;
        injected.push(InjectedSecret::from_value(name, decrypted.expose()));
        resolved_storage.push(decrypted);
    }
    let resolved: std::collections::HashMap<&str, &DecryptedSecret> = unique_order
        .iter()
        .zip(resolved_storage.iter())
        .map(|(name, d)| (*name, d))
        .collect();

    // Phase 2: substitute each `{{secret:NAME}}` occurrence with the cached
    // plaintext. No further resolver calls.
    let mut result = String::with_capacity(input.len());
    let mut last_end = 0usize;
    for secret_ref in &refs {
        result.push_str(&input[last_end..secret_ref.start]);
        let decrypted = resolved
            .get(secret_ref.name.as_str())
            .expect("unique names resolved in phase 1");
        result.push_str(decrypted.expose());
        last_end = secret_ref.end;
    }
    result.push_str(&input[last_end..]);

    Ok((result, injected))
}

#[cfg(test)]
mod tests {
    use super::super::types::DecryptedSecret;
    use super::*;

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
    impl AsyncSecretResolver for MockResolver {
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
        let (_, injected) = render_with_secrets("{{secret:key}}", &resolver)
            .await
            .unwrap();
        let record = &injected[0];
        assert_eq!(record.name, "key");
        assert_eq!(record.value_len, "my-secret-value".len());
        assert_ne!(record.value_hash, 0);
    }

    #[tokio::test]
    async fn test_render_preserves_surrounding_text() {
        let resolver = MockResolver::new().with("token", "abc123");
        let input = "before {{secret:token}} after";
        let (rendered, _) = render_with_secrets(input, &resolver).await.unwrap();
        assert_eq!(rendered, "before abc123 after");
    }

    #[tokio::test]
    async fn test_render_value_contains_placeholder_not_replaced() {
        // If a secret value happens to contain a placeholder literal,
        // it must NOT be treated as a new placeholder to resolve.
        let resolver = MockResolver::new()
            .with("a", "{{secret:b}}")
            .with("b", "REAL_B");
        let input = "A={{secret:a}} B={{secret:b}}";
        let (rendered, injected) = render_with_secrets(input, &resolver).await.unwrap();
        // secret:a resolves to literal "{{secret:b}}", which stays as-is.
        assert_eq!(rendered, "A={{secret:b}} B=REAL_B");
        assert_eq!(injected.len(), 2);
    }
}
