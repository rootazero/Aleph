//! Publication point for a started Feishu channel's authenticated API client.
//!
//! # Why this exists
//!
//! The inbound path builds a `FeishuEventEmitter` **per message**
//! (`inbound_router::executor::try_create_feishu_emitter`), and it needs an
//! authenticated `FeishuApi`. It had no way to reach the one the channel is
//! already holding, so it built a second `TokenManager` and forced a token
//! round-trip on every single inbound message — while the channel's own
//! `spawn_token_refresh` task was keeping a perfectly valid token warm a few
//! structs away.
//!
//! The comment that stood here recorded the cost as mitigated ("the lazy
//! `get_token()` in TokenManager mitigates the worst case"), which was true of
//! the mechanism and false of the code: the call site used `refresh_token()`,
//! which is precisely the one that never consults the cache.
//!
//! # It publishes the config too, and that is not a convenience
//!
//! The caller used to rebuild `FeishuConfig` from `Config.channels`, and that
//! parse **cannot succeed on a deployment that has saved a channel**: the
//! secret migration moves `app_secret` into the vault and removes it from
//! `config.toml`, while `FeishuConfig::app_secret` is required. The `.ok()?`
//! swallowed it, so `try_create_feishu_emitter` returned `None` every time and
//! the streaming/typing-indicator emitter was structurally unreachable — no
//! error, no log, just the plain reply path forever.
//!
//! The started channel is holding a fully hydrated `FeishuConfig` already.
//! Handing it over is the "read it back rather than compute it again" rule:
//! the executor has no vault handle and no business acquiring one to answer a
//! question the channel has already answered. It is also more correct than
//! re-reading the file — the emitter should honour the config the *running*
//! channel started with, not whatever has been edited since.
//!
//! # Why a `Weak`, and why a fallback remains
//!
//! Entries are `Weak` so a channel that is dropped without `stop()` cannot
//! keep a client — and a background refresh task — alive behind everyone's
//! back, and so a dead entry answers `None` rather than handing out a client
//! whose token will silently stop being refreshed.
//!
//! `None` is a normal answer, not an error: the channel may not have started
//! (test mode, or an inbound message racing boot). The caller keeps its
//! original construction path for that case. That is one function with a fast
//! path, not two sources of truth — the fallback builds exactly what this
//! table would have handed back.

use std::collections::HashMap;
use std::sync::{OnceLock, Weak};

use crate::sync_primitives::{Arc, Mutex as StdMutex};

use super::api::FeishuApi;
use super::config::FeishuConfig;

/// What a started channel publishes about itself.
///
/// The client is `Weak` (see the module docs); the config is a plain clone —
/// it is small, and a snapshot is exactly what a consumer wants: the settings
/// this channel is *running* with.
struct Live {
    api: Weak<FeishuApi>,
    config: FeishuConfig,
}

type Table = StdMutex<HashMap<String, Live>>;

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, Live>> {
    table().lock().unwrap_or_else(|e| e.into_inner())
}

/// Announce the client a channel just authenticated, and the config it is
/// running with. Called from `start`.
pub(crate) fn publish(channel_id: &str, api: &Arc<FeishuApi>, config: &FeishuConfig) {
    lock().insert(
        channel_id.to_string(),
        Live {
            api: Arc::downgrade(api),
            config: config.clone(),
        },
    );
}

/// Drop a channel's entry. Called from `stop`.
///
/// Dropping the channel's own `Arc` already makes the entry unusable; removing
/// it keeps the table from accumulating one dead key per restart.
pub(crate) fn withdraw(channel_id: &str) {
    lock().remove(channel_id);
}

/// The live client and running config for `channel_id`, if a started channel
/// published them.
pub(crate) fn get(channel_id: &str) -> Option<(Arc<FeishuApi>, FeishuConfig)> {
    let mut table = lock();
    match table
        .get(channel_id)
        .and_then(|live| live.api.upgrade().map(|api| (api, live.config.clone())))
    {
        Some(found) => Some(found),
        None => {
            // The channel went away without `stop()` (abort, panic, drop).
            table.remove(channel_id);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::feishu::auth::TokenManager;

    fn client(id: &str) -> Arc<FeishuApi> {
        let http = reqwest::Client::new();
        let auth = Arc::new(TokenManager::new(
            id,
            "secret",
            "http://127.0.0.1:1",
            http.clone(),
        ));
        Arc::new(FeishuApi::new(auth, "http://127.0.0.1:1", http))
    }

    fn config(app_id: &str) -> FeishuConfig {
        serde_json::from_value(serde_json::json!({
            "app_id": app_id,
            "app_secret": "hydrated-from-the-vault",
            "streaming": false,
        }))
        .expect("fixture config must parse")
    }

    #[test]
    fn a_published_client_comes_back_by_channel_id() {
        let api = client("pub-a");
        publish("chan-pub-a", &api, &config("cli_a"));
        assert!(get("chan-pub-a").is_some());
        withdraw("chan-pub-a");
        assert!(get("chan-pub-a").is_none());
    }

    /// The config travels with the client because the consumer cannot rebuild
    /// it: `app_secret` lives in the vault, not in `Config.channels`, and it
    /// is a required field.
    #[test]
    fn the_running_config_travels_with_the_client() {
        let api = client("pub-c");
        publish("chan-pub-c", &api, &config("cli_c"));
        let (_, cfg) = get("chan-pub-c").expect("published");
        assert_eq!(cfg.app_id, "cli_c");
        assert!(
            !cfg.streaming,
            "the snapshot must be what start() was given"
        );
        withdraw("chan-pub-c");
    }

    /// The whole point of `Weak`: an entry must not outlive the channel that
    /// made it. A stale `Arc` here would be a client whose token refresher is
    /// gone — it works until the token expires, then fails on a path nobody
    /// would think to look at.
    #[test]
    fn a_dropped_client_stops_being_handed_out() {
        let api = client("pub-b");
        publish("chan-pub-b", &api, &config("cli_b"));
        drop(api);
        assert!(
            get("chan-pub-b").is_none(),
            "a dead Weak must read as absent so the caller rebuilds",
        );
        assert!(
            !lock().contains_key("chan-pub-b"),
            "and the dead key must be reaped, or a restarting channel leaks one entry per cycle",
        );
    }
}
