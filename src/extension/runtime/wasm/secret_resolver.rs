//! Secret resolver for WASM plugin outbound HTTP requests.
//!
//! Plugins declare `CredentialBinding`s in their `[http.credentials]` manifest
//! block (host-pattern + secret name + injection strategy). The host resolves
//! the named secret through a [`SecretResolver`] **before** the request leaves
//! the sandbox, so the plugin guest never sees the secret value — the
//! resolved value is applied to the outbound request headers / URL by
//! [`super::credential_injector::inject_credential`].
//!
//! ## Trait
//!
//! ```ignore
//! pub trait SecretResolver: Send + Sync {
//!     fn resolve(&self, name: &str) -> Option<String>;
//! }
//! ```
//!
//! The resolver is `Send + Sync` because [`super::WasmCapabilityKernel`] lives
//! behind an `Arc` shared with the WASM host-function closures; resolvers may
//! block (Vault HTTP fetch, file read, in-memory map), so the resolver impl
//! itself owns the I/O strategy.
//!
//! ## What is installed today
//!
//! [`VaultBackedSecretResolver`], when the daemon has a shared token manager —
//! i.e. in any running `aleph-server`. [`DenyAllSecretResolver`] otherwise:
//! test binaries never install the global, so they keep denying without having
//! to opt in.
//!
//! Until 2026-08-19 it was deny-all everywhere. `WasmRuntime::load_plugin`
//! forwarded `None` and `load_plugin_with_resolver(_, Some(..))` had zero call
//! sites in the repo, so host-side credential injection was implemented and
//! unreachable: every `[capabilities.http.credentials]` binding turned that
//! plugin's `http_fetch` into a guaranteed `secret not found`.
//!
//! Before that, this module's doc claimed the opposite — that `load_plugin`
//! installs an [`InMemorySecretResolver`] "as the default" and that
//! `DenyAllSecretResolver` is a "test-only stub". Both were false. A security
//! feature whose documentation reports it as live is worse than one documented
//! as missing: it reads as a control in review.
//!
//! ## What had to land in the same change
//!
//! The gate. [`super::WasmCapabilityKernel::resolve_secret`] applied no
//! `check_secret_pattern` check, while `try_http_fetch` fed it
//! `binding.secret_name` straight from the manifest — so a real resolver would
//! have made `[capabilities.http.credentials]` a way to name any vault key and
//! bypass `[capabilities.secrets] allowed_patterns` entirely. That path had
//! never executed, and would have run for the first time on the day this wire
//! was connected. The gate now lives inside `resolve_secret` rather than at
//! its callers, so a third caller inherits it without knowing it exists.

use crate::sync_primitives::{Arc, RwLock};

/// Lookup a named secret and return its plaintext value.
///
/// Returning `None` means "I don't know that secret".
///
/// That is **not** "the request proceeds unchanged". `inject_credential`
/// skips a binding whose `host_patterns` do not match the URL, but a binding
/// that DOES match and whose secret is unknown returns `SecretNotFound`, and
/// `try_http_fetch` turns that into a hard error before any egress. The
/// failure direction is closed: there is no unauthenticated request and no
/// plaintext leak, but the call fails.
///
/// Implementations MUST NOT panic on unknown names; they MUST return `None`
/// so a misconfigured plugin doesn't take down the kernel.
pub trait SecretResolver: Send + Sync {
    /// Resolve `name` to a plaintext secret value.
    fn resolve(&self, name: &str) -> Option<String>;
}

/// In-memory secret store. Thread-safe (`RwLock`) so the same kernel can be
/// queried from the host-function closures on multiple guest calls.
///
/// Loaded from a `Vec<(String, String)>` at construction time. The intent is
/// for callers (manifest parser / vault bootstrap) to populate this once when
/// the plugin is loaded; mutations after `WasmRuntime::load_plugin` are not
/// observed by the running kernel because the kernel takes its own `Arc`
/// reference.
///
/// ## Default
///
/// `WasmRuntime::load_plugin` constructs an empty `InMemorySecretResolver`
/// when the manifest does not declare `[plugin.secrets]`. A plugin that
/// needs credentials must have its loader populate this resolver before
/// `load_plugin` returns — see the `WasmRuntime::with_secret_resolver`
/// builder for the intended extension point.
#[derive(Default, Debug)]
pub struct InMemorySecretResolver {
    secrets: RwLock<Vec<(String, String)>>,
}

impl InMemorySecretResolver {
    /// Build a resolver from an initial key/value list.
    ///
    /// Duplicate names are de-duplicated by keeping the first occurrence
    /// (matches the manifest's "first declaration wins" semantics).
    #[must_use]
    pub fn new(secrets: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut seen = std::collections::HashSet::new();
        let secrets = secrets
            .into_iter()
            .filter(|(name, _)| seen.insert(name.clone()))
            .collect();
        Self {
            secrets: RwLock::new(secrets),
        }
    }

    /// Insert a secret at runtime. No-op if the name is already present
    /// (avoids silently shadowing a pre-configured value).
    pub fn insert(&self, name: String, value: String) {
        let mut guard = self.secrets.write().unwrap_or_else(|e| e.into_inner());
        if guard.iter().any(|(n, _)| n == &name) {
            return;
        }
        guard.push((name, value));
    }
}

impl SecretResolver for InMemorySecretResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        let guard = self.secrets.read().unwrap_or_else(|e| e.into_inner());
        guard
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }
}

/// Resolver that denies every lookup.
///
/// Still the fallback whenever no vault is available — a build with no shared
/// token manager installed (every test binary, and the daemon before boot
/// completes) resolves nothing rather than panicking.
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAllSecretResolver;

impl SecretResolver for DenyAllSecretResolver {
    fn resolve(&self, _name: &str) -> Option<String> {
        None
    }
}

/// Resolver backed by Aleph's encrypted vault.
///
/// The sync twin of [`crate::secrets::VaultSecretResolver`], which MCP uses.
/// Two impls rather than one because the two consumers need different shapes:
/// MCP resolves during an async spawn and takes `AsyncSecretResolver`, while
/// the WASM host functions run inside a synchronous guest call and cannot
/// await. `SharedTokenManager::get_secret` is itself synchronous, so neither
/// wrapper does I/O the other has to fake — they differ only in signature.
///
/// A missing vault entry, a locked vault, or a decryption failure all resolve
/// to `None`: the trait's contract is that an unknown name is `None`, never a
/// panic, and `try_http_fetch` turns `None` on a *matching* binding into a
/// hard error before any egress. The failure direction is closed — no
/// unauthenticated request goes out, and no placeholder leaks into one.
pub struct VaultBackedSecretResolver {
    inner: Arc<crate::gateway::security::shared_token::SharedTokenManager>,
}

impl VaultBackedSecretResolver {
    #[must_use]
    pub const fn new(
        inner: Arc<crate::gateway::security::shared_token::SharedTokenManager>,
    ) -> Self {
        Self { inner }
    }

    /// The resolver a plugin load should use: vault-backed when a shared token
    /// manager has been installed, deny-all otherwise.
    ///
    /// Reads the process-global rather than taking a handle through
    /// `PluginLoader::new`. That is deliberate: a constructor parameter is
    /// inherited only by the construction sites that remember it, and this is
    /// a security-relevant capability whose failure mode is "one loader
    /// resolves secrets, another silently does not". Installing it on the
    /// lookup instead means a `PluginLoader` built somewhere nobody has
    /// thought about yet gets the same answer.
    ///
    /// Test binaries never install the global, so they keep the deny-all
    /// behaviour without opting into it.
    #[must_use]
    pub fn for_current_process() -> Arc<dyn SecretResolver> {
        crate::gateway::security::shared_token::SharedTokenManager::global().map_or_else(
            || Arc::new(DenyAllSecretResolver) as Arc<dyn SecretResolver>,
            |mgr| Arc::new(Self::new(mgr)) as Arc<dyn SecretResolver>,
        )
    }
}

impl SecretResolver for VaultBackedSecretResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        match self.inner.get_secret(name) {
            Ok(Some(secret)) => Some(secret.expose().to_string()),
            Ok(None) => None,
            Err(e) => {
                // Loud: "the vault is locked" and "there is no such key" are
                // very different problems for a plugin author, and both look
                // like `secret not found` at the call site.
                tracing::warn!(
                    secret = %name,
                    error = %e,
                    "vault lookup failed for a plugin credential"
                );
                None
            }
        }
    }
}

/// Convenience: wrap a trait object in an `Arc` so it can be installed on
/// [`super::WasmCapabilityKernel`] without lifetime gymnastics.
#[must_use]
pub fn shared_resolver<R: SecretResolver + 'static>(resolver: R) -> Arc<dyn SecretResolver> {
    Arc::new(resolver)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_resolver_returns_present_secret() {
        let resolver = InMemorySecretResolver::new(vec![(
            "slack_token".to_string(),
            "xoxb-secret".to_string(),
        )]);
        assert_eq!(
            resolver.resolve("slack_token"),
            Some("xoxb-secret".to_string())
        );
    }

    #[test]
    fn in_memory_resolver_returns_none_for_unknown() {
        let resolver = InMemorySecretResolver::new(vec![("a".to_string(), "1".to_string())]);
        assert_eq!(resolver.resolve("b"), None);
    }

    #[test]
    fn in_memory_resolver_dedupes_duplicates() {
        let resolver = InMemorySecretResolver::new(vec![
            ("k".to_string(), "first".to_string()),
            ("k".to_string(), "second".to_string()),
        ]);
        assert_eq!(resolver.resolve("k"), Some("first".to_string()));
    }

    #[test]
    fn in_memory_resolver_insert_skips_existing() {
        let resolver = InMemorySecretResolver::new(vec![("k".to_string(), "first".to_string())]);
        resolver.insert("k".to_string(), "second".to_string());
        assert_eq!(resolver.resolve("k"), Some("first".to_string()));
    }

    #[test]
    fn in_memory_resolver_insert_adds_new() {
        let resolver = InMemorySecretResolver::default();
        resolver.insert("k".to_string(), "v".to_string());
        assert_eq!(resolver.resolve("k"), Some("v".to_string()));
    }

    #[test]
    fn deny_all_resolver_never_returns() {
        let resolver = DenyAllSecretResolver;
        assert_eq!(resolver.resolve("anything"), None);
    }

    #[test]
    fn shared_resolver_wraps_in_arc() {
        let arc: Arc<dyn SecretResolver> = shared_resolver(InMemorySecretResolver::new(vec![(
            "k".to_string(),
            "v".to_string(),
        )]));
        assert_eq!(arc.resolve("k"), Some("v".to_string()));
    }
}
