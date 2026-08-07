//! Ask a provider what it actually serves right now.
//!
//! Everything else in [`model_catalog`](super) is compiled in: the binary's
//! belief about the world, refreshed only when an operator upgrades Aleph.
//! That belief drifts, and the drift is not theoretical — the round that added
//! this module found the aggregator presets two model generations behind the
//! direct-vendor ones, four presets shipping an empty `default_model`, and two
//! ids that the vendor had retired the day before.
//!
//! Both references close that gap by going to the network. opencode pulls the
//! whole [models.dev](https://models.dev) catalog (disk cache, TTL, background
//! refresh, compiled-in snapshot as the offline fallback); kimi-cli asks each
//! configured platform for `GET {base_url}/models` and rewrites its config.
//! Aleph maps the *second* shape, because the first would make a third-party
//! service load-bearing for a core subsystem (R3) while the second reuses
//! credentials and endpoints Aleph already has.
//!
//! The scaffolding for it was already here and dead: `ProviderPreset`'s
//! `models_url` field, [`ProviderPreset::resolve_models_url`] and the
//! `supports_health_check` opt-out had **no production consumer** — one unit
//! test each. Under R10 that is a CUT-or-CONNECT decision; this is the CONNECT.
//!
//! # Stance
//!
//! * **On demand only.** No background timer, no refresh on boot. The model
//!   asks (`list_models { refresh: true }`) or an operator asks (the
//!   `providers.modelsRefresh` RPC). A daemon that quietly phones every
//!   configured vendor on a schedule is a surprise, not a feature.
//! * **Additive, never authoritative.** Discovery contributes *ids*. Windows,
//!   prices and lifecycle still come from the curated tables, because a
//!   `/models` response carries almost none of that. A discovered id with no
//!   curated row shows up honestly as "unknown capabilities", exactly like a
//!   custom relay model does today.
//! * **Fail-soft.** Every error path degrades to the static catalog. Model
//!   discovery is never on the request path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::providers::presets;

/// How long a cached listing is considered current. Mirrors opencode's 5-minute
/// freshness window: long enough that a burst of `list_models` calls in one
/// conversation hits the cache, short enough that "refresh" means something.
pub const CACHE_TTL: Duration = Duration::from_secs(300);

/// Hard cap on one `/models` round trip. The shared provider client
/// deliberately sets no overall request timeout (streaming chat responses are
/// long-lived); a metadata GET has no such excuse.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on the response body we will parse. A `/models` listing is a few KB;
/// anything past this is a misconfigured endpoint (or a captive portal), and
/// parsing it unbounded would be a memory footgun on the tool path.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// In-flight refresh locks, one per provider.
///
/// pi's `Models.refresh()` dedupes concurrent refreshes per provider with an
/// `inflightRefresh ??=` shared promise; this is the same single-flight
/// shape. Without it, a `list_models { refresh: true }` racing a picker's
/// `providers.modelsRefresh` (or two rapid tool calls) dials the same vendor
/// twice with the operator's key. The lock is per provider, so disjoint
/// providers still refresh concurrently.
static REFRESH_LOCKS: Lazy<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// One model id as the provider itself reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    /// The id to send on the wire.
    pub id: String,
    /// Vendor-supplied label, when the listing carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Context window the provider advertises. Only some listings report it;
    /// `None` means "the provider did not say", not "small".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}

/// A provider's live inventory, as cached on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredModels {
    pub provider: String,
    /// Unix seconds at which this listing was fetched.
    pub fetched_at: u64,
    /// The endpoint this listing was fetched from. A cache entry only answers
    /// for the same `base_url`: after the operator moves the endpoint, the
    /// old inventory belongs to a different host, and serving it would be the
    /// same class of wrong as applying a preset `models_url` override to a
    /// relocated endpoint. Entries written before this field existed carry
    /// `None` and are treated as another endpoint's — they cost one refetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub models: Vec<DiscoveredModel>,
}

impl DiscoveredModels {
    /// Whether this listing is younger than `ttl`.
    ///
    /// A clock that moved backwards yields `false` (stale), which costs one
    /// refetch — the safe direction.
    #[must_use]
    pub fn is_fresh(&self, ttl: Duration) -> bool {
        now_secs()
            .checked_sub(self.fetched_at)
            .is_some_and(|age| age <= ttl.as_secs())
    }
}

/// Why a discovery attempt did not produce a listing.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// The preset opted out of `/models` probing (`no_health_check`): OAuth-only
    /// endpoints, per-deployment Azure resources and region-scoped clouds all
    /// 404 or rate-limit the listing route.
    #[error("provider '{0}' does not expose a model listing")]
    Unsupported(String),
    /// No API key was available for the provider.
    #[error("no credential configured for provider '{0}'")]
    MissingCredential(String),
    /// Transport failure (DNS, TLS, connection reset).
    #[error("model listing request to {url} failed: {message}")]
    Transport { url: String, message: String },
    /// The endpoint answered, but not with success.
    #[error("model listing at {url} returned HTTP {status}")]
    Status { url: String, status: u16 },
    /// The endpoint answered with something that is not a model listing.
    #[error("model listing at {url} had an unrecognised shape")]
    Shape { url: String },
    /// The round trip exceeded [`REQUEST_TIMEOUT`].
    #[error("model listing at {url} timed out")]
    Timeout { url: String },
}

/// Fetch a provider's live model list and replace its cache entry.
///
/// `base_url` and `api_key` come from the caller's already-resolved provider
/// config (config value ▸ vault), so this module never touches the vault or
/// the config lock itself — it stays a leaf.
///
/// Concurrent refreshes for the same provider are single-flighted (see
/// [`REFRESH_LOCKS`]): the loser of the race serves the winner's fresh
/// listing. On failure the caller decides whether to fall back to
/// [`cached_models`] (stale snapshot beats no snapshot — pi's
/// snapshot-recovery shape); this function itself reports the error.
pub async fn refresh_models(
    provider: &str,
    base_url: &str,
    protocol: &str,
    api_key: &str,
) -> Result<DiscoveredModels, DiscoveryError> {
    if api_key.trim().is_empty() {
        return Err(DiscoveryError::MissingCredential(provider.to_string()));
    }
    let preset = presets::get_preset(provider);
    if preset.is_some_and(|p| !p.supports_health_check) {
        return Err(DiscoveryError::Unsupported(provider.to_string()));
    }
    let url = models_url_for(provider, base_url);

    // Single-flight: wait out any in-flight refresh for this provider, then
    // serve the listing it just wrote instead of dialling again. The
    // timestamp check (not the TTL) is what keeps this honest for the
    // operator RPC's forced refresh — only a fetch that *raced* this call
    // counts, so "go look now" still goes and looks when nobody else just
    // did.
    let started = now_secs();
    let lock = {
        let mut locks = REFRESH_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            locks
                .entry(provider.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    };
    let _permit = lock.lock().await;
    if let Some(cached) = cached_models(provider, base_url) {
        if cached.fetched_at >= started {
            return Ok(cached);
        }
    }

    let client = crate::providers::protocols::http_client::build_provider_http_client();
    let request = auth_headers(client.get(&url), protocol, api_key);

    let response = match tokio::time::timeout(REQUEST_TIMEOUT, request.send()).await {
        Err(_) => return Err(DiscoveryError::Timeout { url }),
        Ok(Err(e)) => {
            return Err(DiscoveryError::Transport {
                url,
                message: e.to_string(),
            })
        }
        Ok(Ok(r)) => r,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(DiscoveryError::Status {
            url,
            status: status.as_u16(),
        });
    }
    let body = match tokio::time::timeout(REQUEST_TIMEOUT, response.text()).await {
        Err(_) => return Err(DiscoveryError::Timeout { url }),
        Ok(Err(e)) => {
            return Err(DiscoveryError::Transport {
                url,
                message: e.to_string(),
            })
        }
        Ok(Ok(b)) => b,
    };
    if body.len() > MAX_BODY_BYTES {
        return Err(DiscoveryError::Shape { url });
    }

    let models = parse_listing(&body).ok_or(DiscoveryError::Shape { url })?;
    let listing = DiscoveredModels {
        provider: provider.to_string(),
        fetched_at: now_secs(),
        base_url: Some(base_url.to_string()),
        models,
    };
    write_cache(&listing);
    Ok(listing)
}

/// Read a provider's cached listing, if one was ever written **for the same
/// `base_url`**.
///
/// Never fetches. Callers that want freshness check
/// [`DiscoveredModels::is_fresh`] and call [`refresh_models`] themselves —
/// keeping the network call an explicit, visible act at every call site.
///
/// The `base_url` fingerprint (Bifrost's keyconfig-snapshot shape: a config
/// change swaps the whole view) is what keeps a relocated endpoint from
/// inheriting the previous host's inventory. A generation counter was
/// considered and rejected: the per-provider single-flight lock already
/// serialises writers within the process, and a second aleph-server process
/// is a supported-against configuration (doctor's duplicate-instance check),
/// so there is no cross-writer race left for a counter to close.
#[must_use]
pub fn cached_models(provider: &str, base_url: &str) -> Option<DiscoveredModels> {
    let raw = std::fs::read_to_string(cache_path(provider)?).ok()?;
    let listing: DiscoveredModels = serde_json::from_str(&raw).ok()?;
    (listing.base_url.as_deref() == Some(base_url)).then_some(listing)
}

/// Resolve the listing endpoint for a provider.
///
/// The preset's `models_url` override is honoured **only when the operator has
/// not moved `base_url`** — the override is an absolute URL, so applying it to
/// a relocated endpoint (an Azure resource, a corporate relay, a local proxy)
/// would send the probe to the vendor instead of to the configured host.
fn models_url_for(provider: &str, base_url: &str) -> String {
    presets::get_preset(provider)
        .filter(|p| p.base_url == base_url)
        .map_or_else(
            || format!("{}/models", base_url.trim_end_matches('/')),
            presets::ProviderPreset::resolve_models_url,
        )
}

/// Apply the protocol's credential header. Unknown protocols get the OpenAI
/// bearer form, which is what every OpenAI-compatible relay expects.
fn auth_headers(
    builder: reqwest::RequestBuilder,
    protocol: &str,
    api_key: &str,
) -> reqwest::RequestBuilder {
    match protocol {
        "anthropic" => builder
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        "gemini" => builder.header("x-goog-api-key", api_key),
        _ => builder.bearer_auth(api_key),
    }
}

/// Parse a model listing, tolerating the shapes Aleph's presets actually
/// return.
///
/// * OpenAI and Anthropic both wrap the list in `data[]` (Anthropic adds
///   `display_name`).
/// * Gemini and Ollama's native route use `models[]`, with the id under `name`
///   and the window under `inputTokenLimit`.
///
/// Per-entry parsing is lenient by design (P7: validate at the boundary, then
/// trust): one malformed row must not discard a provider's whole inventory, so
/// entries without a usable id are skipped rather than failing the parse. A
/// response with neither array is a shape error.
fn parse_listing(body: &str) -> Option<Vec<DiscoveredModel>> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let rows = json
        .get("data")
        .or_else(|| json.get("models"))
        .and_then(serde_json::Value::as_array)?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row
            .get("id")
            .or_else(|| row.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(id) = id else { continue };
        let display_name = ["display_name", "displayName"]
            .iter()
            .find_map(|k| row.get(*k))
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let context_window = [
            "context_length",
            "context_window",
            "max_context_length",
            "inputTokenLimit",
        ]
        .iter()
        .find_map(|k| row.get(*k))
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .filter(|&v| v > 0);
        out.push(DiscoveredModel {
            id: id.to_string(),
            display_name,
            context_window,
        });
    }
    Some(out)
}

/// `~/.aleph/cache/models/<provider>.json`.
///
/// Provider ids are operator-authored config keys, so the file stem is
/// sanitised rather than trusted: anything outside `[A-Za-z0-9._-]` becomes
/// `_`, which makes `../../etc/passwd` an ordinary (ugly) filename.
fn cache_path(provider: &str) -> Option<PathBuf> {
    let safe: String = provider
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() || safe.chars().all(|c| c == '.') {
        return None;
    }
    let dir = crate::utils::paths::get_config_dir().ok()?;
    Some(
        dir.join("cache")
            .join("models")
            .join(format!("{safe}.json")),
    )
}

/// Persist a listing, replacing any prior entry.
///
/// Best-effort and deliberately silent on failure: a read-only or full disk
/// must degrade discovery to "works, just not cached", never fail the caller's
/// refresh. Written via temp-file + rename so a concurrent reader never sees a
/// half-written listing.
fn write_cache(listing: &DiscoveredModels) {
    let Some(path) = cache_path(&listing.provider) else {
        return;
    };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(json) = serde_json::to_string(listing) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_shape() {
        let body = r#"{"object":"list","data":[
            {"id":"gpt-5.6","object":"model"},
            {"id":"gpt-5.4-mini","object":"model","context_length":400000}
        ]}"#;
        let models = parse_listing(body).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5.6");
        assert_eq!(models[1].context_window, Some(400_000));
    }

    #[test]
    fn parses_anthropic_shape_with_display_name() {
        let body = r#"{"data":[
            {"type":"model","id":"claude-sonnet-5","display_name":"Claude Sonnet 5"}
        ]}"#;
        let models = parse_listing(body).unwrap();
        assert_eq!(models[0].id, "claude-sonnet-5");
        assert_eq!(models[0].display_name.as_deref(), Some("Claude Sonnet 5"));
    }

    #[test]
    fn parses_gemini_shape_from_models_array() {
        let body = r#"{"models":[
            {"name":"models/gemini-3.1-pro-preview","displayName":"Gemini 3.1 Pro",
             "inputTokenLimit":1048576}
        ]}"#;
        let models = parse_listing(body).unwrap();
        assert_eq!(models[0].id, "models/gemini-3.1-pro-preview");
        assert_eq!(models[0].context_window, Some(1_048_576));
    }

    #[test]
    fn skips_unusable_rows_without_discarding_the_listing() {
        let body = r#"{"data":[{"object":"model"},{"id":"  "},{"id":"kimi-k2.6"}]}"#;
        let models = parse_listing(body).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "kimi-k2.6");
    }

    #[test]
    fn rejects_responses_that_are_not_listings() {
        assert!(parse_listing(r#"{"error":{"message":"unauthorized"}}"#).is_none());
        assert!(parse_listing("not json").is_none());
        assert!(parse_listing(r#"{"data":"nope"}"#).is_none());
    }

    #[test]
    fn models_url_prefers_preset_override_only_for_the_preset_base_url() {
        // Anthropic is the one preset carrying an explicit override.
        let preset = presets::get_preset("claude").unwrap();
        assert_eq!(
            models_url_for("claude", preset.base_url),
            "https://api.anthropic.com/v1/models"
        );
        // Relocated base_url ⇒ the override must not send the probe to the vendor.
        assert_eq!(
            models_url_for("claude", "https://relay.internal/anthropic"),
            "https://relay.internal/anthropic/models"
        );
        // No override ⇒ derived from base_url, trailing slash normalised.
        assert_eq!(
            models_url_for("openai", "https://api.openai.com/v1/"),
            "https://api.openai.com/v1/models"
        );
    }

    #[test]
    fn cache_path_sanitises_traversal_attempts() {
        let path = cache_path("../../etc/passwd").unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name, ".._.._etc_passwd.json");
        assert!(!name.contains('/'));
        // Degenerate ids get no path at all rather than a dotfile.
        assert!(cache_path("").is_none());
        assert!(cache_path("..").is_none());
    }

    #[tokio::test]
    async fn opted_out_presets_are_rejected_before_any_request() {
        // `chatgpt` is `no_health_check()` — OAuth-only, no listing route.
        let err = refresh_models("chatgpt", "https://chatgpt.com", "codex", "key")
            .await
            .unwrap_err();
        assert!(matches!(err, DiscoveryError::Unsupported(_)), "{err:?}");
    }

    #[tokio::test]
    async fn missing_credential_is_rejected_before_any_request() {
        let err = refresh_models("openai", "https://api.openai.com/v1", "openai", "  ")
            .await
            .unwrap_err();
        assert!(
            matches!(err, DiscoveryError::MissingCredential(_)),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn concurrent_refreshes_are_single_flighted() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Tiny HTTP server counting `/models` hits, slow enough that two
        // concurrent refreshes genuinely overlap.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_server = Arc::clone(&hits);
        let server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                hits_server.fetch_add(1, Ordering::SeqCst);
                // Drain the request head before answering. Dropping a socket
                // that still holds unread bytes sends RST, not FIN — and on
                // Windows that reset discards the response sitting in the
                // client's receive buffer (WSAECONNRESET 10054), failing the
                // test deterministically while Unix happens to win the race.
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while let Ok(n) = socket.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    head.extend_from_slice(&buf[..n]);
                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                let body = r#"{"data":[{"id":"probe-model"}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                // Graceful FIN so the client reads EOF, never a reset.
                let _ = socket.shutdown().await;
            }
        });

        let url = format!("http://{addr}/v1");
        let (a, b) = tokio::join!(
            refresh_models("singleflight-probe", &url, "openai", "k"),
            refresh_models("singleflight-probe", &url, "openai", "k"),
        );
        server.abort();
        assert!(a.is_ok(), "first refresh failed: {:?}", a.err());
        assert!(b.is_ok(), "second refresh failed: {:?}", b.err());
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "the losing refresh must serve the winner's listing, not redial"
        );
        if let Some(path) = cache_path("singleflight-probe") {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn freshness_window_tracks_fetch_time() {
        let fresh = DiscoveredModels {
            provider: "openai".into(),
            fetched_at: now_secs(),
            base_url: Some("https://api.openai.com/v1".into()),
            models: Vec::new(),
        };
        assert!(fresh.is_fresh(CACHE_TTL));
        let stale = DiscoveredModels {
            fetched_at: now_secs() - CACHE_TTL.as_secs() - 1,
            ..fresh
        };
        assert!(!stale.is_fresh(CACHE_TTL));
    }

    #[test]
    fn cache_only_answers_for_the_same_base_url() {
        let provider = "fingerprint-probe";
        let url_a = "https://a.example/v1";
        let url_b = "https://b.example/v1";
        write_cache(&DiscoveredModels {
            provider: provider.into(),
            fetched_at: now_secs(),
            base_url: Some(url_a.into()),
            models: Vec::new(),
        });

        assert!(
            cached_models(provider, url_a).is_some(),
            "same endpoint reads its cache"
        );
        assert!(
            cached_models(provider, url_b).is_none(),
            "a moved endpoint must not inherit the previous host's inventory"
        );

        if let Some(path) = cache_path(provider) {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn legacy_cache_without_fingerprint_is_not_served() {
        // Entries written before the fingerprint existed carry no `base_url`;
        // they are treated as another endpoint's inventory and cost one
        // refetch rather than a guess.
        let provider = "legacy-fingerprint-probe";
        let path = cache_path(provider).unwrap();
        let parent = path.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({
                "provider": provider,
                "fetched_at": now_secs(),
                "models": [],
            })
            .to_string(),
        )
        .unwrap();

        assert!(cached_models(provider, "https://a.example/v1").is_none());

        let _ = std::fs::remove_file(path);
    }
}
