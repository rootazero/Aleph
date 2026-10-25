//! Derived publication of the canonical Feishu channel's client and hydrated config.
//!
//! This cache cannot activate a provider. Lookup validates the original provider
//! instance and generation; callers must also fence retained clients at the API
//! boundary. Live notifications are not a durable recovery source.

use super::api::FeishuApi;
use super::config::FeishuConfig;
use crate::capability::provider::{ProviderCallError, ProviderHandle};
use crate::sync_primitives::{Arc, Mutex as StdMutex};
use std::collections::HashMap;
use std::sync::{OnceLock, Weak};

#[derive(Clone)]
struct Live {
    api: Weak<FeishuApi>,
    config: FeishuConfig,
    provider: ProviderHandle,
    generation: u64,
}

type Table = StdMutex<HashMap<String, Live>>;
fn lock() -> std::sync::MutexGuard<'static, HashMap<String, Live>> {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE
        .get_or_init(|| StdMutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PublicationError {
    #[error(transparent)]
    Provider(#[from] ProviderCallError),
    #[error("Feishu API instance/generation does not match publication")]
    ApiScopeMismatch,
    #[error("Feishu publication still belongs to another instance/generation")]
    Occupied,
}

/// Commit publication and Starting -> Ready under the same provider lock.
/// Lock order is provider -> cache; the old owner must withdraw before replacement.
pub(crate) fn publish(
    channel_id: &str,
    api: &Arc<FeishuApi>,
    config: &FeishuConfig,
    provider: &ProviderHandle,
    generation: u64,
) -> Result<(), PublicationError> {
    provider.with_ready_commit(generation, |_| {
        if !api.has_scope(provider, generation) {
            return Err(PublicationError::ApiScopeMismatch);
        }
        let mut table = lock();
        if table.get(channel_id).is_some_and(|live| {
            !live.provider.same_instance(provider) || live.generation != generation
        }) {
            return Err(PublicationError::Occupied);
        }
        table.insert(
            channel_id.to_owned(),
            Live {
                api: Arc::downgrade(api),
                config: config.clone(),
                provider: provider.clone(),
                generation,
            },
        );
        Ok(())
    })
}

/// An outgoing instance/generation cannot withdraw its replacement.
pub(crate) fn withdraw(channel_id: &str, provider: &ProviderHandle, generation: u64) {
    let mut table = lock();
    if table
        .get(channel_id)
        .is_some_and(|live| live.provider.same_instance(provider) && live.generation == generation)
    {
        table.remove(channel_id);
    }
}

pub(crate) fn get(channel_id: &str) -> Option<(Arc<FeishuApi>, FeishuConfig)> {
    // Never hold cache -> provider locks: publication takes the reverse order.
    let live = lock().get(channel_id).cloned()?;
    if live.provider.acquire_call(live.generation).is_err() {
        // Remove only this exact provider instance/generation. If a replacement
        // won the publication slot while the stale entry was being inspected,
        // `withdraw` leaves that replacement intact.
        withdraw(channel_id, &live.provider, live.generation);
        return None;
    }
    if let Some(api) = live.api.upgrade() {
        Some((api, live.config))
    } else {
        withdraw(channel_id, &live.provider, live.generation);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::provider::{
        ProviderId, ProviderLifecycle, ProviderState, ProviderTransport,
    };
    use crate::gateway::interfaces::feishu::auth::TokenManager;

    fn fixture(id: &str) -> (Arc<FeishuApi>, FeishuConfig, ProviderHandle) {
        let provider = ProviderHandle::new(ProviderLifecycle::new(
            ProviderId::new(id).unwrap(),
            ProviderTransport::InProcess,
        ));
        provider.transition(ProviderState::Cold).unwrap();
        provider.transition(ProviderState::Starting).unwrap();
        let http = reqwest::Client::new();
        let api = Arc::new(
            FeishuApi::new(
                Arc::new(TokenManager::new(
                    id,
                    "secret",
                    "http://127.0.0.1:1",
                    http.clone(),
                )),
                "http://127.0.0.1:1",
                http,
            )
            .with_provider(provider.clone(), provider.generation()),
        );
        let cfg = serde_json::from_value(serde_json::json!({
            "app_id": id, "app_secret": "hydrated-from-vault", "streaming": false
        }))
        .unwrap();
        (api, cfg, provider)
    }

    #[tokio::test]
    async fn publication_and_retained_client_are_generation_fenced() {
        let (api, cfg, p) = fixture("publication-fenced");
        assert!(get("publication-fenced").is_none());
        // Capture outside the commit callback: it must not re-enter the handle.
        let generation = p.generation();
        publish("publication-fenced", &api, &cfg, &p, generation).unwrap();
        let (_, found) = get("publication-fenced").unwrap();
        assert_eq!(found.app_secret, "hydrated-from-vault");
        assert!(!found.streaming);
        p.transition(ProviderState::Stopping).unwrap();
        assert!(get("publication-fenced").is_none());
        let error = api.send_text("chat", "stale", None).await.unwrap_err();
        assert!(
            matches!(error, super::super::api::FeishuSendError::Other(e) if e.contains("rejects call"))
        );
        withdraw("publication-fenced", &p, generation);
    }

    #[test]
    fn publication_rejects_wrong_api_scope_and_live_incumbent() {
        let (api, cfg, provider) = fixture("publication-scope");
        let (other_api, _, other) = fixture("publication-scope");
        let generation = provider.generation();
        assert!(matches!(
            publish("publication-scope", &other_api, &cfg, &provider, generation),
            Err(PublicationError::ApiScopeMismatch)
        ));
        assert_eq!(provider.state(), ProviderState::Starting);
        assert!(get("publication-scope").is_none());
        publish("publication-scope", &api, &cfg, &provider, generation).unwrap();
        assert!(matches!(
            publish(
                "publication-scope",
                &other_api,
                &cfg,
                &other,
                other.generation()
            ),
            Err(PublicationError::Occupied)
        ));
        assert_eq!(other.state(), ProviderState::Starting);
        assert!(Arc::ptr_eq(&get("publication-scope").unwrap().0, &api));
        withdraw("publication-scope", &other, other.generation());
        assert!(get("publication-scope").is_some());
        withdraw("publication-scope", &provider, generation);
    }

    #[test]
    fn old_generation_cannot_publish_or_withdraw_after_restart() {
        let (old, cfg, provider) = fixture("publication-restart");
        let first = provider.generation();
        publish("publication-restart", &old, &cfg, &provider, first).unwrap();
        provider.transition(ProviderState::Stopping).unwrap();
        withdraw("publication-restart", &provider, first);
        provider.transition(ProviderState::Stopped).unwrap();
        provider.transition(ProviderState::Starting).unwrap();
        let next = provider.generation();
        assert!(publish("publication-restart", &old, &cfg, &provider, first).is_err());
        assert!(matches!(
            publish("publication-restart", &old, &cfg, &provider, next),
            Err(PublicationError::ApiScopeMismatch)
        ));
        let http = reqwest::Client::new();
        let new = Arc::new(
            FeishuApi::new(
                Arc::new(TokenManager::new(
                    "id",
                    "secret",
                    "http://127.0.0.1:1",
                    http.clone(),
                )),
                "http://127.0.0.1:1",
                http,
            )
            .with_provider(provider.clone(), next),
        );
        publish("publication-restart", &new, &cfg, &provider, next).unwrap();
        assert!(publish("publication-restart", &old, &cfg, &provider, first).is_err());
        withdraw("publication-restart", &provider, first);
        assert!(Arc::ptr_eq(&get("publication-restart").unwrap().0, &new));
        withdraw("publication-restart", &provider, next);
    }

    #[test]
    fn late_publish_cannot_overwrite_replacement() {
        let (old, cfg, provider) = fixture("late-publication-provider");
        let generation = provider.generation();
        publish("late-publication", &old, &cfg, &provider, generation).unwrap();
        provider.transition(ProviderState::Stopping).unwrap();
        withdraw("late-publication", &provider, generation);
        let (new, cfg, replacement) = fixture("late-publication-provider");
        let next = replacement.generation();
        publish("late-publication", &new, &cfg, &replacement, next).unwrap();

        // Both instances deliberately have the same textual id and generation.
        assert!(publish("late-publication", &old, &cfg, &provider, generation).is_err());
        withdraw("late-publication", &provider, generation);
        assert!(Arc::ptr_eq(&get("late-publication").unwrap().0, &new));
        withdraw("late-publication", &replacement, next);
    }

    #[test]
    fn stale_get_removes_only_its_publication_and_allows_republish() {
        let (old, cfg, provider) = fixture("stale-get-old");
        let generation = provider.generation();
        publish("stale-get", &old, &cfg, &provider, generation).unwrap();
        provider.transition(ProviderState::Stopping).unwrap();

        // The stale lookup must clear the old publication so the channel can be
        // published again, while retaining instance/generation fencing for
        // late cleanup from the old owner.
        assert!(get("stale-get").is_none());
        assert!(!lock().contains_key("stale-get"));

        let (replacement, replacement_cfg, replacement_provider) = fixture("stale-get-new");
        let replacement_generation = replacement_provider.generation();
        publish(
            "stale-get",
            &replacement,
            &replacement_cfg,
            &replacement_provider,
            replacement_generation,
        )
        .unwrap();
        withdraw("stale-get", &provider, generation);
        assert!(Arc::ptr_eq(&get("stale-get").unwrap().0, &replacement));
        withdraw("stale-get", &replacement_provider, replacement_generation);
    }
    #[test]
    fn late_withdraw_and_dead_weak_cannot_remove_replacement() {
        let (old, cfg, p) = fixture("old-provider");
        let generation = p.generation();
        publish("replacement-publication", &old, &cfg, &p, generation).unwrap();
        p.transition(ProviderState::Stopping).unwrap();
        withdraw("replacement-publication", &p, generation);
        let (new, cfg, replacement) = fixture("new-provider");
        let new_generation = replacement.generation();
        publish(
            "replacement-publication",
            &new,
            &cfg,
            &replacement,
            new_generation,
        )
        .unwrap();
        withdraw("replacement-publication", &p, generation);
        assert!(Arc::ptr_eq(
            &get("replacement-publication").unwrap().0,
            &new
        ));
        drop(old);
        assert!(get("replacement-publication").is_some());
        drop(new);
        assert!(get("replacement-publication").is_none());
        assert!(!lock().contains_key("replacement-publication"));
    }
}
