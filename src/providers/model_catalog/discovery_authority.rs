//! Process-local invalidation fence for configured model discovery.

use std::sync::{Mutex, MutexGuard};
use tokio::sync::RwLockReadGuard;

use crate::config::Config;

static REVISION: Mutex<u64> = Mutex::new(0);

/// An opaque process-local revision, not a credential identifier or durable cache key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiscoveryRevision(u64);

/// Capture while holding the live config read guard, **before** resolving
/// the effective endpoint, protocol and credential. After resolution, reject
/// the capture if a second sample differs. Release the config guard before I/O.
pub fn discovery_revision(_config: &RwLockReadGuard<'_, Config>) -> DiscoveryRevision {
    DiscoveryRevision(*mutation_fence().0)
}

/// Run a synchronous cache/publication commit only if the revision still holds.
/// The caller must already have re-resolved and compared the complete effective
/// identity under this same config guard. Do not read the vault, acquire config,
/// or await inside `commit`; acquire the publication slot last.
pub fn commit_if_current<T>(
    _config: &RwLockReadGuard<'_, Config>,
    expected: DiscoveryRevision,
    commit: impl FnOnce() -> T,
) -> Option<T> {
    let fence = mutation_fence();
    if *fence.0 != expected.0 {
        return None;
    }
    Some(commit())
}

pub(crate) struct MutationFence(MutexGuard<'static, u64>);

/// Config/vault owners acquire this after their own lock, never the reverse.
pub(crate) fn mutation_fence() -> MutationFence {
    MutationFence(REVISION.lock().unwrap_or_else(|e| e.into_inner()))
}

impl MutationFence {
    pub(crate) fn advance(&mut self) {
        *self.0 = self.0.checked_add(1).expect("discovery revision exhausted");
    }
    pub(crate) fn provider_changed(&mut self, provider: &str) {
        self.advance();
        super::discovery::revoke_inventory(|name| name == provider);
    }

    pub(crate) fn vault_changed(&mut self, affected: impl Fn(&str) -> bool) {
        self.advance();
        super::discovery::revoke_vault_inventory(affected);
    }
}

/// Call under the live config write lock after an accepted replacement. Only
/// fields affecting listing identity or eligibility need to invalidate fetches.
pub(crate) fn config_replaced(previous: &Config, next: &Config) {
    let presets_changed = previous.presets_override.providers != next.presets_override.providers;
    let merged_route = |config: &Config, name: &str| {
        crate::providers::presets::get_merged_preset(name, &config.presets_override)
            .map(|preset| (preset.base_url, preset.protocol))
    };
    let affected = |name: &str| match (previous.providers.get(name), next.providers.get(name)) {
        (Some(old), Some(new)) => {
            old.base_url != new.base_url
                || old.protocol != new.protocol
                || old.api_key != new.api_key
                || old.enabled != new.enabled
                || (presets_changed && merged_route(previous, name) != merged_route(next, name))
        }
        (None, None) => false,
        _ => true,
    };
    let route_changed = previous
        .providers
        .keys()
        .chain(next.providers.keys())
        .any(|name| affected(name));
    let roster_changed = previous.providers.iter().any(|(name, old)| {
        next.providers.get(name).is_some_and(|new| {
            old.all_models() != new.all_models()
                || !old
                    .model_defs
                    .iter()
                    .map(|def| &def.id)
                    .eq(new.model_defs.iter().map(|def| &def.id))
        })
    });
    if presets_changed || route_changed || roster_changed {
        let mut fence = mutation_fence();
        if presets_changed || route_changed {
            fence.advance();
            super::discovery::withdraw_config_inventory(&affected);
        }
        crate::providers::session_model_handle::update_configured_selection(next, affected);
    }
}

/// Install a persisted candidate under the caller's config write lock.
pub(crate) fn replace_config(live: &mut Config, next: Config) {
    config_replaced(live, &next);
    *live = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::gateway::security::{store::SecurityStore, SharedTokenManager};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn configured() -> Config {
        let mut config = Config::default();
        config.providers.clear();
        let mut provider = ProviderConfig::test_config("test-model");
        provider.base_url = Some("https://a.invalid/v1".into());
        provider.protocol = Some("openai".into());
        config.providers.insert("tenant".into(), provider);
        config
    }

    async fn assert_invalidated(
        config: &RwLock<Config>,
        previous: DiscoveryRevision,
    ) -> DiscoveryRevision {
        let guard = config.read().await;
        assert!(commit_if_current(&guard, previous, || panic!("stale commit ran")).is_none());
        let current = discovery_revision(&guard);
        assert!(current.0 > previous.0);
        current
    }

    #[tokio::test]
    async fn configured_listing_changes_and_aba_reject_old_commits() {
        let original = configured();
        let live = RwLock::new(original.clone());
        let mut revision = discovery_revision(&live.read().await);
        let mut next = original.clone();
        next.providers.get_mut("tenant").unwrap().base_url = Some("https://b.invalid/v1".into());
        replace_config(&mut *live.write().await, next.clone());
        revision = assert_invalidated(&live, revision).await;
        next.providers.get_mut("tenant").unwrap().protocol = Some("anthropic".into());
        replace_config(&mut *live.write().await, next.clone());
        revision = assert_invalidated(&live, revision).await;
        next.providers.get_mut("tenant").unwrap().api_key = Some("runtime-key".into());
        replace_config(&mut *live.write().await, next.clone());
        revision = assert_invalidated(&live, revision).await;
        let provider = next.providers.remove("tenant").unwrap();
        next.providers.insert("renamed".into(), provider);
        replace_config(&mut *live.write().await, next.clone());
        let before_removal = assert_invalidated(&live, revision).await;
        next.providers.clear();
        replace_config(&mut *live.write().await, next);
        let after_removal = assert_invalidated(&live, before_removal).await;
        replace_config(&mut *live.write().await, original.clone());
        let a = assert_invalidated(&live, after_removal).await;
        let mut removed = original.clone();
        removed.providers.clear();
        replace_config(&mut *live.write().await, removed);
        replace_config(&mut *live.write().await, original);
        assert_invalidated(&live, a).await;
    }

    #[tokio::test]
    async fn preset_only_reload_aba_rejects_stale_commit() {
        let original = configured();
        let live = RwLock::new(original.clone());
        let a = discovery_revision(&live.read().await);
        let mut reloaded = original.clone();
        reloaded.presets_override.providers.insert(
            "tenant".into(),
            crate::config::presets_override::PartialProviderPreset {
                base_url: Some("https://override.invalid/v1".into()),
                protocol: Some("anthropic".into()),
                ..Default::default()
            },
        );
        replace_config(&mut *live.write().await, reloaded);
        let b = assert_invalidated(&live, a).await;
        replace_config(&mut *live.write().await, original);
        assert_invalidated(&live, b).await;
        assert_invalidated(&live, a).await;
    }

    #[tokio::test]
    async fn real_vault_crud_and_replacement_reject_old_commits() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = SharedTokenManager::new(store, dir.path().join("test.vault"));
        vault.generate_token().unwrap();
        let config = RwLock::new(configured());
        let key = "ai:tenant";
        vault.store_secret(key, "a").unwrap();
        let a = discovery_revision(&config.read().await);
        vault.store_secret(key, "b").unwrap();
        let b = assert_invalidated(&config, a).await;
        vault.store_secret(key, "a").unwrap();
        let a_again = assert_invalidated(&config, b).await;
        assert_invalidated(&config, a).await;
        assert!(vault.delete_secret(key).unwrap());
        let deleted = assert_invalidated(&config, a_again).await;
        vault.store_secret_batch([(key, "a")]).unwrap();
        assert_eq!(vault.get_secret(key).unwrap().unwrap().expose(), "a");
        assert_invalidated(&config, deleted).await;
    }
}
