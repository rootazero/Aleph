//! Live, hot-swappable search provider registry.
//!
//! The `[search]` section is read once at boot into a [`SearchRegistry`] the
//! `search` builtin tool captures. A config edit alone never reaches that
//! capture — the same gap `providers::route_handle` closes for `[route]`.
//! This handle is the search half of that fix: one process-global,
//! lock-free cell the tool reads on *every* call and the config-write path
//! (`config::live_apply`'s `search` arm) swaps on every `[search]` write.
//! The next search sees the new registry with **no daemon restart**.
//!
//! Two differences from `route` are load-bearing:
//!
//! 1. **The vault travels with the handle.**
//!    [`crate::config::types::SearchBackendConfig::api_key`] is
//!    `skip_serializing` — a patched `Config` never carries search keys, so
//!    rebuilding a registry from `cfg` alone would silently drop every
//!    credential (the factory skips key-less backends, and the swap would
//!    replace a working registry with an empty one under a success
//!    response). The handle keeps the [`SharedTokenManager`] `Arc` boot was
//!    handed and re-resolves `search:<name>` keys at apply time, mirroring
//!    the boot hydration in `commands/start/mod.rs` ("Search backends:
//!    vault key \"search:<name>\"").
//! 2. **Rebuild failure keeps the old registry.** [`SearchHandle::apply_config`]
//!    returns `false` when the new registry could not be computed (a vault
//!    read error), leaving the previous generation serving and letting
//!    `apply_live_sections` downgrade the section's verdict to `Restart`
//!    honestly. A rebuild that yields *no usable backend* is NOT a failure:
//!    it stores the same registry a restart would produce (bare Tavily key,
//!    else empty), because that is what the config now says.

use arc_swap::ArcSwap;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::config::Config;
use crate::gateway::security::SharedTokenManager;
use crate::search::SearchRegistry;
use crate::sync_primitives::Arc;

/// Why a live `[search]` apply did not happen.
///
/// Library error type (thiserror, per project convention). The only failure
/// mode today is the vault read: every per-backend construction problem is
/// already absorbed by [`SearchRegistry::from_config`], which logs and skips
/// the offender rather than failing the build.
#[derive(Debug, thiserror::Error)]
pub enum SearchApplyError {
    /// A backend's API key could not be re-resolved from the vault. Building
    /// anyway would construct that backend credential-less — the factory
    /// would skip it, and the swap would quietly uninstall a working
    /// backend. Keeping the old registry and saying so is the honest answer.
    #[error("vault read for search backend '{backend}' failed: {message}")]
    VaultRead { backend: String, message: String },
}

/// Live search registry shared between the `search` tool (reader) and the
/// config-write path (writer).
///
/// The registry lives behind one [`ArcSwap`] inside an [`Arc`] so the tool
/// can hold the cell (`registry_cell`) without holding the vault half of
/// this struct. A hot-apply publishes the whole rebuilt registry atomically;
/// a caller racing a writer gets the whole old registry or the whole new one
/// for one call, never a torn mix.
pub struct SearchHandle {
    registry: Arc<ArcSwap<SearchRegistry>>,
    /// Boot's vault handle, kept so a live rebuild can re-resolve the API
    /// keys a patched `Config` provably does not carry (see the module doc).
    vault: Arc<SharedTokenManager>,
}

impl SearchHandle {
    /// Seed a handle from the registry boot resolved for the tool face —
    /// i.e. the output of the configured/`from_env_only`/empty decision,
    /// so the very first snapshot matches what the tool would have captured
    /// before this handle existed.
    #[must_use]
    pub fn new(initial: Arc<SearchRegistry>, vault: Arc<SharedTokenManager>) -> Self {
        Self {
            registry: Arc::new(ArcSwap::new(initial)),
            vault,
        }
    }

    /// The swap cell, for the tool face to hold. Reading it is lock-free.
    #[must_use]
    pub fn registry_cell(&self) -> Arc<ArcSwap<SearchRegistry>> {
        Arc::clone(&self.registry)
    }

    /// One coherent snapshot of the live registry (single lock-free RCU load).
    #[must_use]
    pub fn snapshot(&self) -> Arc<SearchRegistry> {
        self.registry.load_full()
    }

    /// Rebuild the registry from `cfg` — re-resolving backend API keys from
    /// the vault — and publish it as one atomic swap. `false` means the old
    /// registry is still serving and the caller must not report the change
    /// as live (`apply_live_sections` binds this to `landed`).
    #[must_use]
    pub fn apply_config(&self, cfg: &Config) -> bool {
        match rebuild_registry(cfg, &self.vault) {
            Ok(registry) => {
                self.registry.store(registry);
                true
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "[search] live apply aborted; the previous registry keeps serving"
                );
                false
            }
        }
    }
}

/// Build the registry `cfg` describes, hydrating API keys from `vault`.
///
/// Split from [`SearchHandle::apply_config`] so the decision is testable
/// without the swap cell. Mirrors the boot path exactly: vault hydration
/// (`start/mod.rs`) then [`SearchRegistry::from_config`], and when that
/// yields nothing usable, the same fallback the tool face resolves at boot
/// ([`SearchRegistry::for_tool`] over a bare Tavily key, else an empty
/// registry) — because that fallback is what a restart would install.
fn rebuild_registry(
    cfg: &Config,
    vault: &SharedTokenManager,
) -> Result<Arc<SearchRegistry>, SearchApplyError> {
    let mut search = cfg.search.clone();
    if let Some(ref mut search_cfg) = search {
        for (name, backend) in &mut search_cfg.backends {
            if backend.api_key.is_none() {
                // The one definition of this key format lives next to the
                // RPC write path that stores the secret.
                let key = crate::gateway::handlers::search_config::dto::vault_key(name);
                match vault.get_secret(&key) {
                    Ok(Some(secret)) => backend.api_key = Some(secret.expose().to_string()),
                    // No stored key: the factory skips the backend with a
                    // warning, exactly as it does for a keyless boot config.
                    Ok(None) => {}
                    Err(e) => {
                        return Err(SearchApplyError::VaultRead {
                            backend: name.clone(),
                            message: e.to_string(),
                        });
                    }
                }
            }
        }
    }

    let registry =
        match SearchRegistry::from_config(search.as_ref(), cfg.ssrf.allow_private_network) {
            Some(registry) => Arc::new(registry),
            None => {
                let tavily_key = search
                    .as_ref()
                    .and_then(|s| s.backends.get(&s.default_provider))
                    .and_then(|b| b.api_key.clone());
                SearchRegistry::for_tool(None, tavily_key.as_deref())
            }
        };
    Ok(registry)
}

/// Process-global live handle. Installed once at boot from the loaded config
/// (when the agent stack is built at all); swapped in place thereafter via
/// the handle's interior [`ArcSwap`].
///
/// `MissingSemantics::ConsumerDecides`: the one reader is
/// `config::live_apply`'s `search` arm, and its decision is the honest
/// downgrade — absent handle (CLI process, test, or a boot that never built
/// the agent stack) means the write persisted but nothing runtime-side
/// received it, so the verdict must be `Restart`, not `Live`.
static GLOBAL_SEARCH_HANDLE: CapabilitySlot<Arc<SearchHandle>> =
    CapabilitySlot::new("search/registry-handle", MissingSemantics::ConsumerDecides);

/// The handle above, type-erased for the roster — same shape as every other
/// slot accessor (`spend::global_policy_slot` et al.), so the roster reads
/// the return type and never the value.
pub(crate) const fn global_search_handle_slot() -> &'static dyn SlotStatus {
    &GLOBAL_SEARCH_HANDLE
}

/// Install the boot-built handle. Returns `false` when one was already
/// installed — the same idempotence every `CapabilitySlot` install promises.
pub fn install_global_search_handle(handle: Arc<SearchHandle>) -> bool {
    GLOBAL_SEARCH_HANDLE.install(handle)
}

/// Record that boot reached this slot and could not install it — called from
/// the `else` of the gate that would have built the agent stack, so the
/// capability-wiring diagnostic can say WHY a `[search]` write needs a
/// restart in this process instead of reading "never reached".
pub fn decline_global_search_handle(because: &'static str) {
    GLOBAL_SEARCH_HANDLE.decline(because);
}

/// Fetch the global handle if boot installed one. The config-write path uses
/// this to hot-apply: present in a running daemon, `None` in a CLI process,
/// a test, or before the agent stack is assembled — in which case the
/// on-disk write still takes effect at the next start, and the caller must
/// say so.
pub fn try_global_search_handle() -> Option<Arc<SearchHandle>> {
    GLOBAL_SEARCH_HANDLE.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{SearchBackendConfig, SearchConfigInternal};
    use crate::gateway::security::SecurityStore;

    fn vault_with_token() -> Arc<SharedTokenManager> {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(SecurityStore::in_memory().expect("store"));
        let vault = Arc::new(SharedTokenManager::new(store, dir.path().join("t.vault")));
        // Leak the tempdir guard deliberately: the vault path must outlive
        // the manager, and a test process exiting reclaims it anyway.
        std::mem::forget(dir);
        vault.generate_token().expect("token");
        vault
    }

    fn ddg_config(max_results: usize) -> SearchConfigInternal {
        SearchConfigInternal {
            enabled: true,
            default_provider: "ddg".to_string(),
            fallback_providers: None,
            max_results,
            timeout_seconds: 10,
            backends: std::collections::HashMap::from([(
                "ddg".to_string(),
                SearchBackendConfig {
                    provider_type: "duckduckgo".to_string(),
                    api_key: None,
                    base_url: None,
                    engine_id: None,
                    engines: None,
                    min_request_interval_ms: None,
                    verified: false,
                },
            )]),
            ..Default::default()
        }
    }

    /// The whole point of the handle: a config write swaps the registry, and
    /// the tool's very next snapshot reads the new generation — no restart.
    #[test]
    fn apply_config_swaps_the_registry_the_next_snapshot_reads() {
        let vault = vault_with_token();
        let handle = SearchHandle::new(Arc::new(SearchRegistry::new("none")), vault);

        let mut cfg = Config::default();
        cfg.search = Some(ddg_config(9));

        assert!(handle.apply_config(&cfg), "a buildable config must land");
        assert_eq!(handle.snapshot().default_options().max_results, 9);
    }

    /// A live apply must reach through the vault for keys the patched config
    /// provably does not carry (`api_key` is `skip_serializing`). Stored key
    /// in, hydrated backend out — a keyless rebuild would have skipped the
    /// backend and produced the empty-registry fallback instead.
    #[test]
    fn apply_config_re_resolves_backend_keys_from_the_vault() {
        let vault = vault_with_token();
        vault
            .store_secret("search:tavily", "tvly-test-key")
            .expect("store");
        let handle = SearchHandle::new(Arc::new(SearchRegistry::new("none")), vault);

        let mut search = ddg_config(5);
        search.default_provider = "tavily".to_string();
        search.backends.insert(
            "tavily".to_string(),
            SearchBackendConfig {
                provider_type: "tavily".to_string(),
                api_key: None, // as a patched config always arrives
                base_url: None,
                engine_id: None,
                engines: None,
                min_request_interval_ms: None,
                verified: false,
            },
        );
        let mut cfg = Config::default();
        cfg.search = Some(search);

        assert!(handle.apply_config(&cfg));
        assert!(
            handle.snapshot().get_provider("tavily").is_some(),
            "the vault-held key must have constructed the backend"
        );
    }

    /// The honest-failure half: when the vault cannot be read (no token
    /// loaded — the state `SharedTokenManager::get_secret` errors on), the
    /// old registry keeps serving and the caller is told the apply did not
    /// happen, so the write surface downgrades to `Restart` instead of
    /// reporting a swap that never occurred.
    #[test]
    fn a_rebuild_that_cannot_resolve_its_secrets_keeps_the_old_registry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(SecurityStore::in_memory().expect("store"));
        // No `generate_token`: every `get_secret` errors.
        let vault = Arc::new(SharedTokenManager::new(store, dir.path().join("t.vault")));
        std::mem::forget(dir);

        let initial = Arc::new(SearchRegistry::new("none"));
        let handle = SearchHandle::new(Arc::clone(&initial), vault);

        let mut search = ddg_config(5);
        // A keyless keyed backend forces the vault read that fails.
        search.backends.insert(
            "tavily".to_string(),
            SearchBackendConfig {
                provider_type: "tavily".to_string(),
                api_key: None,
                base_url: None,
                engine_id: None,
                engines: None,
                min_request_interval_ms: None,
                verified: false,
            },
        );
        let mut cfg = Config::default();
        cfg.search = Some(search);

        assert!(
            !handle.apply_config(&cfg),
            "a registry that could not be built correctly must not be stored"
        );
        assert!(
            Arc::ptr_eq(&handle.snapshot(), &initial),
            "the previous generation must still be serving"
        );
    }

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_search_handle_slot();
        assert_eq!(slot.id(), "search/registry-handle");
        assert!(matches!(slot.missing(), MissingSemantics::ConsumerDecides));
    }
}
