//! Derived publication of the canonical Feishu channel's client and hydrated config.
//!
//! This cache cannot activate a provider. Lookup validates the original provider
//! instance and generation; callers must also fence retained clients at the API
//! boundary. Live notifications are not a durable recovery source.

use super::api::FeishuApi;
use super::config::FeishuConfig;
use crate::capability::provider::ProviderHandle;
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

// Called only inside ProviderHandle::with_ready_commit (provider -> cache).
pub(crate) fn publish(
    channel_id: &str,
    api: &Arc<FeishuApi>,
    config: &FeishuConfig,
    provider: &ProviderHandle,
    generation: u64,
) {
    lock().insert(
        channel_id.to_owned(),
        Live {
            api: Arc::downgrade(api),
            config: config.clone(),
            provider: provider.clone(),
            generation,
        },
    );
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
        ProviderCallError, ProviderId, ProviderLifecycle, ProviderState, ProviderTransport,
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
        p.with_ready_commit(generation, |_| {
            publish("publication-fenced", &api, &cfg, &p, generation);
            Ok::<_, ProviderCallError>(())
        })
        .unwrap();
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
    fn late_withdraw_and_dead_weak_cannot_remove_replacement() {
        let (old, cfg, p) = fixture("old-provider");
        let generation = p.generation();
        p.with_ready_commit(generation, |_| {
            publish("replacement-publication", &old, &cfg, &p, generation);
            Ok::<_, ProviderCallError>(())
        })
        .unwrap();
        let (new, cfg, replacement) = fixture("new-provider");
        let new_generation = replacement.generation();
        replacement
            .with_ready_commit(new_generation, |_| {
                publish(
                    "replacement-publication",
                    &new,
                    &cfg,
                    &replacement,
                    new_generation,
                );
                Ok::<_, ProviderCallError>(())
            })
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
