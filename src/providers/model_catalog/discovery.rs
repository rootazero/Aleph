//! Scoped, process-local provider model discovery.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;

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
///
/// Enforced by [`read_bounded`] **while** reading. Checking `body.len()` after
/// `response.text()` describes what already fitted in memory; it does not stop
/// anything from getting there.
const MAX_BODY_BYTES: usize = 1024 * 1024;

// One model as the provider itself reports it. The shape is the wire's:
// `providers.modelsRefresh` hands these rows straight to a client, so a local
// copy would be a second definition of the same three fields.
pub use aleph_protocol::providers::DiscoveredModel;

/// A provider's live inventory, accepted in this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredModels {
    /// Distinguishes accepted inventories even when timestamps and rows match.
    acceptance: Option<(u64, u64)>,
    pub provider: String,
    /// Unix seconds at which this listing was fetched.
    pub fetched_at: u64,
    pub models: Vec<DiscoveredModel>,
}

impl DiscoveredModels {
    pub(crate) fn same_acceptance(&self, other: &Self) -> bool {
        self.acceptance.is_some() && self.acceptance == other.acceptance
    }

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
#[derive(Debug, Clone, thiserror::Error)]
pub enum DiscoveryError {
    #[error("model listing scope changed for provider '{0}'")]
    Superseded(String),
    /// The preset opted out of `/models` probing (`no_health_check`): OAuth-only
    /// endpoints, per-deployment Azure resources and region-scoped clouds all
    /// 404 or rate-limit the listing route.
    #[error("provider '{0}' does not expose a model listing")]
    Unsupported(String),
    /// The requested provider does not have a linked configuration.
    #[error("provider '{0}' is not configured")]
    NotConfigured(String),
    /// No API key was available for the provider.
    #[error("no credential configured for provider '{0}'")]
    MissingCredential(String),
    /// Transport failure (DNS, TLS, connection reset).
    #[error("model listing request for provider '{provider}' failed")]
    Transport { provider: String },
    /// The endpoint answered, but not with success.
    #[error("model listing for provider '{provider}' returned HTTP {status}")]
    Status { provider: String, status: u16 },
    /// The endpoint answered with something that is not a model listing.
    #[error("model listing for provider '{provider}' had an unrecognised shape")]
    Shape { provider: String },
    /// The round trip exceeded `REQUEST_TIMEOUT`.
    #[error("model listing for provider '{provider}' timed out")]
    Timeout { provider: String },
}

use super::discovery_authority::{commit_if_current, discovery_revision};
use crate::config::{Config, ProviderConfig};
use tokio::sync::RwLock;

#[derive(Clone, Copy)]
pub enum RefreshMode {
    PreferFreshCache,
    Force,
    /// A named admin request may inspect a disabled configuration, but may not publish it.
    ForceNamed,
}

#[derive(Clone, Debug)]
pub enum RefreshOutcome {
    Fresh(DiscoveredModels),
    CacheHit(DiscoveredModels),
    Stale {
        listing: DiscoveredModels,
        cause: DiscoveryError,
    },
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Scope {
    config: usize,
    endpoint_digest: [u8; 32],
    protocol: String,
    credential: [u8; 32],
    vault_backed: bool,
    enabled: bool,
}

type Outcome = Result<RefreshOutcome, DiscoveryError>;
type Flight = tokio::sync::Mutex<Option<Outcome>>;
type FlightKey = (String, Scope, u64);
type Inventory = (Weak<RwLock<Config>>, Scope, DiscoveredModels);
static FLIGHTS: Lazy<Mutex<HashMap<FlightKey, Weak<Flight>>>> = Lazy::new(Default::default);
static INVENTORIES: Lazy<Mutex<HashMap<String, Inventory>>> = Lazy::new(Default::default);

/// Per-provider scope epoch.
///
/// The epoch counts how often a provider's effective listing scope may have
/// changed while a discovery request was in flight. It is the ABA guard: the
/// scope comparison alone cannot see A -> B -> A, because by the time the
/// result comes back the scope is exactly where it started. The epoch is
/// bumped by the mutation paths that can change a provider's listing identity
/// (config replacement, credential change, vault write) and it is **scoped to
/// the provider**, so a mutation elsewhere in the process cannot cancel this
/// provider's fetch. Only the identity of the scope is compared here — never
/// the credential itself.
#[derive(Clone, Copy)]
struct ScopeEpoch {
    epoch: u64,
    /// Whether the provider's credential currently comes from the vault. Only
    /// vault-backed providers are affected by a vault write.
    vault_backed: bool,
}

static SCOPE_EPOCHS: Lazy<Mutex<HashMap<String, ScopeEpoch>>> = Lazy::new(Default::default);

/// The provider's current scope epoch, registering the provider on first use.
fn scope_epoch(provider: &str) -> u64 {
    let mut epochs = SCOPE_EPOCHS.lock().unwrap_or_else(|e| e.into_inner());
    epochs
        .entry(provider.to_owned())
        .or_insert(ScopeEpoch {
            epoch: 0,
            vault_backed: false,
        })
        .epoch
}

/// Record whether the provider's credential is vault-backed. Called before the
/// epoch is sampled, so a vault write that lands while the request is in
/// flight still finds the provider registered.
fn mark_vault_backed(provider: &str, vault_backed: bool) {
    let mut epochs = SCOPE_EPOCHS.lock().unwrap_or_else(|e| e.into_inner());
    epochs
        .entry(provider.to_owned())
        .or_insert(ScopeEpoch {
            epoch: 0,
            vault_backed: false,
        })
        .vault_backed = vault_backed;
}

/// Capture the credential-backed route epoch under the live config read guard.
pub(crate) fn oauth_route_epoch(
    provider: &str,
    _config: &tokio::sync::RwLockReadGuard<'_, Config>,
) -> u64 {
    mark_vault_backed(provider, true);
    scope_epoch(provider)
}

/// Bump the epoch of every provider the mutation can have changed.
fn bump_scope_epochs(affected: impl Fn(&str) -> bool, vault_only: bool) {
    let mut epochs = SCOPE_EPOCHS.lock().unwrap_or_else(|e| e.into_inner());
    for (name, entry) in epochs.iter_mut() {
        if vault_only && !entry.vault_backed {
            continue;
        }
        if affected(name) {
            entry.epoch = entry
                .epoch
                .checked_add(1)
                .expect("discovery scope epoch exhausted");
        }
    }
}

/// Commit only while the provider's scope epoch still matches the one sampled
/// before the request started.
///
/// The caller holds the config read guard and the coarse revision fence, so a
/// config mutation cannot interleave with the commit: the mutation path bumps
/// the epoch under the config write lock. A vault write does not take the
/// config lock, but it does take the revision fence, so the fence keeps the
/// epoch check and the publication atomic against it as well.
pub(crate) fn scope_epoch_current(provider: &str, expected: u64) -> bool {
    let epochs = SCOPE_EPOCHS.lock().unwrap_or_else(|e| e.into_inner());
    epochs.get(provider).map_or(0, |entry| entry.epoch) == expected
}

/// Called under the mutation epoch; only matching providers lose inventory.
pub(super) fn revoke_inventory(affected: impl Fn(&str) -> bool) {
    withdraw_config_inventory(&affected);
    crate::providers::session_model_handle::revoke_config_selection(affected);
}

pub(super) fn withdraw_config_inventory(affected: &impl Fn(&str) -> bool) {
    bump_scope_epochs(affected, false);
    INVENTORIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|name, _| !affected(name));
}

pub(super) fn revoke_vault_inventory(affected: impl Fn(&str) -> bool) {
    bump_scope_epochs(&affected, true);
    INVENTORIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|name, (_, scope, _)| !scope.vault_backed || !affected(name));
    crate::providers::session_model_handle::revoke_vault_discovery(affected);
}

pub(crate) fn listing_address(
    config: &Config,
    provider: &str,
) -> Result<(String, String), DiscoveryError> {
    let pc = config
        .providers
        .get(provider)
        .ok_or_else(|| DiscoveryError::NotConfigured(provider.into()))?;
    if !crate::providers::probe::supports_model_listing(provider) {
        return Err(DiscoveryError::Unsupported(provider.into()));
    }
    let preset = presets::get_merged_preset(provider, &config.presets_override);
    let base = pc
        .base_url
        .as_deref()
        .or_else(|| preset.as_ref().map(|p| p.base_url.as_str()))
        .filter(|base| !base.trim().is_empty())
        .ok_or_else(|| DiscoveryError::Unsupported(provider.into()))?;
    let endpoint = listing_endpoint(provider, base)
        .ok_or_else(|| DiscoveryError::Unsupported(provider.into()))?;
    let protocol = pc
        .protocol
        .clone()
        .or_else(|| preset.as_ref().map(|p| p.protocol.to_string()))
        .unwrap_or_else(|| "openai".into());
    Ok((endpoint, protocol))
}

fn listing_endpoint(provider: &str, base: &str) -> Option<String> {
    if let Some(preset) = presets::get_preset(provider).filter(|preset| preset.base_url == base) {
        return Some(preset.resolve_models_url());
    }
    let mut url = reqwest::Url::parse(base).ok()?;
    url.path_segments_mut().ok()?.pop_if_empty().push("models");
    Some(url.into())
}

/// Check the transport actually mounted by the run builder, not a second
/// transport synthesized from current configuration.
pub fn transport_matches_listing(
    config: &Config,
    provider: &str,
    transport: &dyn crate::providers::AiProvider,
    credential: impl Fn(&str, &ProviderConfig) -> Option<String>,
) -> bool {
    let Some(http) = transport.as_http_provider() else {
        return false;
    };
    let Ok((scope, endpoint, key)) = resolve_scope(config, 0, provider, &credential) else {
        return false;
    };
    http.matches_configured_transport(provider, &scope.protocol, &key, |base| {
        listing_endpoint(provider, base).as_deref() == Some(endpoint.as_str())
    })
}

pub(crate) fn configured_inventory(
    config: &Arc<RwLock<Config>>,
    guard: &tokio::sync::RwLockReadGuard<'_, Config>,
    provider: &str,
    resolve: impl Fn(&str, &ProviderConfig) -> Option<String>,
) -> Option<DiscoveredModels> {
    let identity = Arc::as_ptr(config) as usize;
    mark_vault_backed(
        provider,
        guard
            .providers
            .get(provider)
            .is_some_and(|pc| pc.api_key.as_ref().is_none_or(String::is_empty)),
    );
    let epoch = scope_epoch(provider);
    // Sample the coarse revision before any credential resolution: the fence
    // must never be taken while the vault is read.
    let revision = discovery_revision(guard);
    let (scope, _, _) = resolve_scope(guard, identity, provider, &resolve).ok()?;
    commit_if_current(guard, revision, || {
        if !scope_epoch_current(provider, epoch) {
            return None;
        }
        inventory(config, provider, &scope)
    })
    .flatten()
}

fn resolve_scope(
    config: &Config,
    identity: usize,
    provider: &str,
    resolve: &impl Fn(&str, &ProviderConfig) -> Option<String>,
) -> Result<(Scope, String, String), DiscoveryError> {
    use sha2::{Digest, Sha256};
    static SALT: Lazy<uuid::Uuid> = Lazy::new(uuid::Uuid::new_v4);
    let digest = |domain: &[u8], value: &str| -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(SALT.as_bytes());
        digest.update(domain);
        digest.update(value.as_bytes());
        digest.finalize().into()
    };
    let (endpoint, protocol) = listing_address(config, provider)?;
    let pc = &config.providers[provider];
    let vault_backed = pc.api_key.as_ref().is_none_or(String::is_empty);
    let key = pc
        .api_key
        .as_ref()
        .filter(|key| !key.is_empty())
        .cloned()
        .or_else(|| resolve(provider, pc))
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| DiscoveryError::MissingCredential(provider.into()))?;
    Ok((
        Scope {
            config: identity,
            endpoint_digest: digest(b"endpoint", &endpoint),
            protocol,
            credential: digest(b"credential", &key),
            vault_backed,
            enabled: pc.enabled,
        },
        endpoint,
        key,
    ))
}

fn inventory(
    config: &Arc<RwLock<Config>>,
    provider: &str,
    scope: &Scope,
) -> Option<DiscoveredModels> {
    let inventories = INVENTORIES.lock().unwrap_or_else(|e| e.into_inner());
    let (owner, accepted, listing) = inventories.get(provider)?;
    (owner.ptr_eq(&Arc::downgrade(config)) && accepted == scope).then(|| listing.clone())
}

pub async fn refresh_configured_models(
    config: &Arc<RwLock<Config>>,
    provider: &str,
    mode: RefreshMode,
    resolve_credential: impl Fn(&str, &ProviderConfig) -> Option<String>,
) -> Outcome {
    let obsolete = || DiscoveryError::Superseded(provider.into());
    let identity = Arc::as_ptr(config) as usize;
    let guard = config.read().await;
    mark_vault_backed(
        provider,
        guard
            .providers
            .get(provider)
            .is_some_and(|pc| pc.api_key.as_ref().is_none_or(String::is_empty)),
    );
    let epoch = scope_epoch(provider);
    // Sample the coarse revision before the credential is resolved, so the
    // fence is never held while the vault is read.
    let revision = discovery_revision(&guard);
    let (scope, _, _) = resolve_scope(&guard, identity, provider, &resolve_credential)?;
    if !scope.enabled && !matches!(mode, RefreshMode::ForceNamed) {
        return Err(DiscoveryError::Unsupported(provider.into()));
    }
    // A cache hit is a read: it republishes nothing. Only a freshly fetched
    // listing may change the published rows.
    let cached = commit_if_current(&guard, revision, || {
        scope_epoch_current(provider, epoch).then(|| {
            if matches!(mode, RefreshMode::PreferFreshCache) {
                inventory(config, provider, &scope).filter(|listing| listing.is_fresh(CACHE_TTL))
            } else {
                None
            }
        })
    })
    .flatten()
    .ok_or_else(obsolete)?;
    if let Some(listing) = cached {
        return Ok(RefreshOutcome::CacheHit(listing));
    }
    drop(guard);

    let flight_key = (provider.to_owned(), scope.clone(), epoch);
    let flight = {
        let mut flights = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
        flights.retain(|_, flight| flight.strong_count() != 0);
        if let Some(existing) = flights.get(&flight_key).and_then(Weak::upgrade) {
            existing
        } else {
            let flight = Arc::new(Flight::new(None));
            flights.insert(flight_key.clone(), Arc::downgrade(&flight));
            flight
        }
    };
    let mut shared = flight.lock().await;
    // A waiter can become the leader if the previous leader was cancelled.
    // Resolve a current credential after waiting, never carry one into the queue.
    let guard = config.read().await;
    let revision = discovery_revision(&guard);
    let request = resolve_scope(&guard, identity, provider, &resolve_credential)
        .ok()
        .and_then(|(current, endpoint, key)| (current == scope).then_some((endpoint, key)))
        .filter(|_| {
            commit_if_current(&guard, revision, || scope_epoch_current(provider, epoch))
                .unwrap_or(false)
        });
    let outcome = if let Some(outcome) = &*shared {
        return if request.is_some() {
            outcome.clone()
        } else {
            Err(obsolete())
        };
    } else if let Some((endpoint, key)) = request {
        drop(guard);
        let fetched = fetch_listing(provider, &endpoint, &scope.protocol, &key).await;
        drop((endpoint, key));
        let guard = config.read().await;
        let revision = discovery_revision(&guard);
        let current = resolve_scope(&guard, identity, provider, &resolve_credential);
        if current.is_ok_and(|(current, _, _)| current == scope) {
            commit_if_current(&guard, revision, || {
                if !scope_epoch_current(provider, epoch) {
                    return Err(obsolete());
                }
                match fetched {
                    Ok(mut listing) => {
                        use std::sync::atomic::{AtomicU64, Ordering};
                        static NEXT_ACCEPTANCE: AtomicU64 = AtomicU64::new(0);
                        listing.acceptance =
                            Some((epoch, NEXT_ACCEPTANCE.fetch_add(1, Ordering::Relaxed)));
                        INVENTORIES
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(
                                provider.into(),
                                (Arc::downgrade(config), scope.clone(), listing.clone()),
                            );
                        if scope.enabled {
                            crate::providers::session_model_handle::publish_discovered(
                                provider,
                                &listing.models,
                            );
                        }
                        Ok(RefreshOutcome::Fresh(listing))
                    }
                    Err(cause) => match inventory(config, provider, &scope) {
                        // The last good listing stays published; the failure
                        // does not publish anything new.
                        Some(listing) => Ok(RefreshOutcome::Stale { listing, cause }),
                        None => Err(cause),
                    },
                }
            })
            .unwrap_or_else(|| Err(obsolete()))
        } else {
            Err(obsolete())
        }
    } else {
        Err(obsolete())
    };
    *shared = Some(outcome.clone());
    FLIGHTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&flight_key);
    outcome
}

async fn fetch_listing(
    provider: &str,
    endpoint: &str,
    protocol: &str,
    api_key: &str,
) -> Result<DiscoveredModels, DiscoveryError> {
    // The protocol headers include secrets that reqwest does not strip from
    // every redirect. Listing uses only the configured endpoint.
    static CLIENT: Lazy<Result<reqwest::Client, reqwest::Error>> = Lazy::new(|| {
        crate::providers::protocols::http_client::provider_http_client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
    });
    let transport = || DiscoveryError::Transport {
        provider: provider.into(),
    };
    let timeout = || DiscoveryError::Timeout {
        provider: provider.into(),
    };
    let shape = || DiscoveryError::Shape {
        provider: provider.into(),
    };
    // Do not expose transport error strings: invalid addresses and the error
    // source chain may contain credentials from URL userinfo or query strings.
    let client = CLIENT.as_ref().map_err(|_| transport())?;
    let request = auth_headers(client.get(endpoint), protocol, api_key);
    let response = tokio::time::timeout(REQUEST_TIMEOUT, request.send())
        .await
        .map_err(|_| timeout())?
        .map_err(|_| transport())?;
    let status = response.status();
    if !status.is_success() {
        return Err(DiscoveryError::Status {
            provider: provider.into(),
            status: status.as_u16(),
        });
    }
    let body = tokio::time::timeout(REQUEST_TIMEOUT, read_bounded(response))
        .await
        .map_err(|_| timeout())?
        .map_err(|error| match error {
            BodyError::TooLarge => shape(),
            BodyError::Transport => transport(),
        })?;
    let models = parse_listing(&body).ok_or_else(shape)?;
    Ok(DiscoveredModels {
        acceptance: None,
        provider: provider.into(),
        fetched_at: now_secs(),
        models,
    })
}

/// Why a bounded body read stopped.
enum BodyError {
    /// The response exceeded [`MAX_BODY_BYTES`] — reported as a shape failure,
    /// which is what it is: a `/models` listing is a few KB.
    TooLarge,
    /// The stream broke mid-body.
    Transport,
}

/// Read a response body, refusing to buffer more than [`MAX_BODY_BYTES`].
///
/// `response.text()` buffers the whole body first and only then can its length
/// be compared — which makes the cap a description of what was already in
/// memory rather than a limit on it. A hostile or merely misconfigured endpoint
/// (a captive portal serving a video, a relay streaming an error loop) is
/// reachable from the tool path, so the bound has to be enforced *while*
/// reading.
///
/// Chunk boundaries do not matter here: the whole body must be under the cap,
/// so a running total that trips mid-chunk is exact, not approximate.
async fn read_bounded(response: reqwest::Response) -> Result<String, BodyError> {
    let mut body: Vec<u8> = Vec::new();
    let mut response = response;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > MAX_BODY_BYTES {
                    return Err(BodyError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return Err(BodyError::Transport),
        }
    }
    // A listing that is not UTF-8 is not a listing. Reported as transport
    // rather than shape only because the bytes never became a document.
    String::from_utf8(body).map_err(|_| BodyError::Transport)
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
            // Ollama `/api/tags` does not currently emit `context_length` on
            // `details` (it ships `parameter_size` and `family` instead), so
            // this entry is a forward-compatibility slot, not a known shape:
            // the day Ollama starts advertising the figure it lands here
            // without a parser change. Listing it now keeps the table the
            // single place to consult when a new vendor shape appears.
            "details.context_length",
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
    if !rows.is_empty() && out.is_empty() {
        return None;
    }
    Some(out)
}

#[cfg(test)]
tokio::task_local! { static TEST_NOW_SECONDS: u64; }

fn now_secs() -> u64 {
    #[cfg(test)]
    if let Ok(seconds) = TEST_NOW_SECONDS.try_with(|seconds| *seconds) {
        return seconds;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tokio::sync::{mpsc, oneshot, RwLock};

    const LISTING: &str = r#"{"data":[{"id":"vendor-model"}]}"#;
    const EMPTY_LISTING: &str = r#"{"data":[]}"#;
    const NOT_A_LISTING: &str = "not a listing";

    /// The discovery fence is deliberately coarse: any configuration or vault
    /// mutation anywhere in the process may cancel an in-flight fetch, and the
    /// ABA scenario deliberately mutates the configuration while a fetch is in
    /// flight. The scenarios below are therefore serialised against each
    /// other, and a scenario cancelled by a mutation that had nothing to do
    /// with it is retried.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A loopback `/models` endpoint whose responses the test controls.
    #[derive(Clone)]
    struct Endpoint {
        url: String,
        body: Arc<tokio::sync::Mutex<Option<String>>>,
        arrivals: Arc<tokio::sync::Mutex<mpsc::Receiver<oneshot::Sender<()>>>>,
    }

    impl Endpoint {
        async fn start() -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/v1", listener.local_addr().unwrap());
            let body = Arc::new(tokio::sync::Mutex::new(None));
            let (tx, rx) = mpsc::channel(1);
            let server_body = Arc::clone(&body);
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let mut header = Vec::new();
                    while !header.ends_with(b"\r\n\r\n") {
                        let Ok(byte) = socket.read_u8().await else {
                            break;
                        };
                        header.push(byte);
                    }
                    assert!(header.starts_with(b"GET /v1/models "));
                    let (release, released) = oneshot::channel::<()>();
                    if tx.send(release).await.is_err() {
                        break;
                    }
                    if released.await.is_err() {
                        break;
                    }
                    let body: String = server_body.lock().await.take().unwrap_or_default();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    if socket.write_all(response.as_bytes()).await.is_err() {
                        break;
                    }
                    let _ = socket.shutdown().await;
                }
            });
            Self {
                url,
                body,
                arrivals: Arc::new(tokio::sync::Mutex::new(rx)),
            }
        }

        /// Wait for the next request; the returned sender releases it.
        async fn next_request(&self) -> oneshot::Sender<()> {
            tokio::time::timeout(Duration::from_secs(5), async {
                self.arrivals.lock().await.recv().await
            })
            .await
            .expect("the expected listing request never reached the endpoint")
            .expect("the endpoint stopped accepting requests")
        }

        /// The body the pending request is answered with.
        async fn answer(&self, body: &str) {
            *self.body.lock().await = Some(body.to_owned());
        }

        /// Assert that no request arrives within a short window.
        async fn assert_idle(&self) {
            let mut arrivals = self.arrivals.lock().await;
            let arrived =
                tokio::time::timeout(std::time::Duration::from_millis(250), arrivals.recv()).await;
            assert!(
                arrived.is_err(),
                "the endpoint received a request that should not have happened"
            );
        }
    }

    /// The provider the scenario under test owns. Every scenario uses its own
    /// name: the retained inventory is keyed by provider name.
    fn other_provider(provider: &str) -> String {
        format!("{provider}-other")
    }

    fn fixture_with(
        url: &str,
        provider: &str,
        ready: bool,
        authenticated: bool,
        vault_backed: bool,
    ) -> (Arc<RwLock<Config>>, SelectableSnapshot) {
        let other = other_provider(provider);
        let mut config = Config::default();
        let mut pc = ProviderConfig::test_config("operator-model");
        pc.base_url = Some(url.into());
        pc.api_key = (!vault_backed).then(|| "fake-key-a".into());
        config.providers.insert(provider.to_owned(), pc);
        config.providers.insert(
            other.clone(),
            ProviderConfig::test_config("unrelated-model"),
        );
        let routable: BTreeSet<String> = [provider.to_owned(), other.clone()].into_iter().collect();
        let available: BTreeSet<String> = if ready {
            routable.clone()
        } else {
            [other.clone()].into_iter().collect()
        };
        let snapshot = SelectableSnapshot::from_config(&config, routable, &available, |name| {
            authenticated || name == other
        })
        .certify_discovery_transports(|_| true);
        (Arc::new(RwLock::new(config)), snapshot)
    }

    type SelectableSnapshot = crate::providers::session_model_handle::SelectableModelsSnapshot;

    async fn call(config: &Arc<RwLock<Config>>, provider: &str, mode: RefreshMode) -> Outcome {
        refresh_configured_models(config, provider, mode, |_, _| None).await
    }

    async fn call_with(
        config: &Arc<RwLock<Config>>,
        provider: &str,
        mode: RefreshMode,
        resolve: impl Fn(&str, &ProviderConfig) -> Option<String>,
    ) -> Outcome {
        refresh_configured_models(config, provider, mode, resolve).await
    }

    async fn test_revision(
        config: &RwLock<Config>,
    ) -> super::super::discovery_authority::DiscoveryRevision {
        let guard = config.read().await;
        discovery_revision(&guard)
    }

    async fn expect_observed_supersession(
        config: &RwLock<Config>,
        before: super::super::discovery_authority::DiscoveryRevision,
        outcome: &Outcome,
    ) {
        if matches!(outcome, Err(DiscoveryError::Superseded(_))) {
            assert!(
                test_revision(config).await != before,
                "setup refresh rejected at a stable revision: {outcome:?}"
            );
        }
    }

    async fn setup_refresh(
        config: &RwLock<Config>,
        refresh: impl std::future::Future<Output = Outcome>,
    ) -> Outcome {
        let before = test_revision(config).await;
        let outcome = refresh.await;
        expect_observed_supersession(config, before, &outcome).await;
        outcome
    }

    fn selectable(provider: &str, model: &str) -> bool {
        crate::providers::session_model_handle::selectable_models()
            .unwrap()
            .resolve(Some(provider), model)
            .is_ok()
    }

    /// Run `run` against a fresh endpoint and configuration.
    ///
    /// Retry `Superseded` only when the attempt observed a revision change.
    /// Setup steps after intentional mutations also check their own revision
    /// window so that earlier mutations cannot mask a false rejection.
    async fn scenario<F, Fut>(provider: &str, run: F)
    where
        F: FnMut(Arc<RwLock<Config>>, Endpoint) -> Fut,
        Fut: std::future::Future<Output = Result<(), DiscoveryError>>,
    {
        scenario_with_credentials(provider, false, run).await;
    }

    async fn scenario_with_credentials<F, Fut>(provider: &str, vault_backed: bool, mut run: F)
    where
        F: FnMut(Arc<RwLock<Config>>, Endpoint) -> Fut,
        Fut: std::future::Future<Output = Result<(), DiscoveryError>>,
    {
        let _serial = SERIAL.lock().await;
        for _ in 0..8 {
            let endpoint = Endpoint::start().await;
            let (config, snapshot) =
                fixture_with(&endpoint.url, provider, true, true, vault_backed);
            let before = test_revision(&config).await;
            let outcome = crate::providers::session_model_handle::with_test_selectable_models(
                Some(snapshot),
                TEST_NOW_SECONDS.scope(1000, run(Arc::clone(&config), endpoint.clone())),
            )
            .await;
            match outcome {
                Ok(()) => return,
                Err(DiscoveryError::Superseded(_)) if test_revision(&config).await != before => {
                    continue
                }
                Err(other) => panic!("unexpected discovery failure: {other}"),
            }
        }
        panic!("the scenario kept being cancelled by unrelated mutations");
    }

    #[test]
    fn discovery_requires_the_installed_transport_scope() {
        let provider = "openai";
        let mut cfg = Config::default();
        let mut pc = ProviderConfig::test_config("operator-model");
        pc.api_key = Some("scope-test-key".into());
        cfg.providers.insert(provider.into(), pc.clone());
        let installed = crate::providers::create_provider(provider, pc.clone()).unwrap();
        assert!(transport_matches_listing(
            &cfg,
            provider,
            installed.as_ref(),
            |_, _| None
        ));
        cfg.presets_override.providers.insert(
            provider.into(),
            crate::config::presets_override::PartialProviderPreset {
                base_url: Some("https://listing-relay.invalid/v1".into()),
                enabled: true,
                ..Default::default()
            },
        );
        assert!(!transport_matches_listing(
            &cfg,
            provider,
            installed.as_ref(),
            |_, _| None
        ));
        pc.base_url = Some("https://listing-relay.invalid/v1".into());
        let matching = crate::providers::create_provider(provider, pc.clone()).unwrap();
        assert!(transport_matches_listing(
            &cfg,
            provider,
            matching.as_ref(),
            |_, _| None
        ));
        cfg.providers.get_mut(provider).unwrap().api_key = Some("rotated".into());
        assert!(!transport_matches_listing(
            &cfg,
            provider,
            matching.as_ref(),
            |_, _| None
        ));
    }

    #[tokio::test]
    async fn identical_fresh_listings_have_distinct_acceptances() {
        const PROVIDER: &str = "task-1b-acceptance-id";
        scenario(PROVIDER, |config, endpoint| async move {
            let refresh = || async {
                let (result, ()) =
                    tokio::join!(call(&config, PROVIDER, RefreshMode::Force), async {
                        let release = endpoint.next_request().await;
                        endpoint.answer(LISTING).await;
                        release.send(()).unwrap();
                    });
                match result? {
                    RefreshOutcome::Fresh(listing) => Ok(listing),
                    other => panic!("expected a fresh listing, got {other:?}"),
                }
            };
            let first = refresh().await?;
            let second = refresh().await?;
            assert_eq!(first.fetched_at, second.fetched_at);
            assert_eq!(first.models, second.models);
            assert!(!first.same_acceptance(&second));
            let guard = config.read().await;
            let current = configured_inventory(&config, &guard, PROVIDER, |_, _| None).unwrap();
            assert!(current.same_acceptance(&second));
            Ok(())
        })
        .await;
    }

    #[test]
    fn empty_listing_is_authoritative_but_nonempty_invalid_is_shape_error() {
        for envelope in ["data", "models"] {
            let decode = |rows| {
                parse_listing(&format!(r#"{{"{envelope}":{rows}}}"#)).ok_or_else(|| {
                    DiscoveryError::Shape {
                        provider: "test".into(),
                    }
                })
            };
            assert!(decode("[]").unwrap().is_empty());
            assert!(matches!(
                decode(r#"[{"id":""},{"name":9}]"#),
                Err(DiscoveryError::Shape { .. })
            ));
            let models = decode(r#"[{"name":9},{"id":"usable"}]"#).unwrap();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].id, "usable");
        }
    }

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
        assert_eq!(models.len(), 1);
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
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "models/gemini-3.1-pro-preview");
        assert_eq!(models[0].context_window, Some(1_048_576));
    }

    #[test]
    fn parses_ollama_shape_with_details() {
        // Ollama `/api/tags` uses the `models[]` array (same outer shape as
        // Gemini) with two material differences: the id lives on `name` and
        // the only metadata shipped today is `details.parameter_size` /
        // `details.family` — there is **no** context-length field. The parser
        // must still resolve both ids; window stays `None` because Ollama
        // simply does not send it. The `details.context_length` slot in the
        // context-window array is a forward-compatibility placeholder for the
        // day Ollama starts emitting the figure.
        let body = r#"{"models":[
            {"name":"llama3.3:70b","details":{"parameter_size":"70B","family":"llama"}},
            {"name":"codellama:13b","details":{"parameter_size":"13B"}}
        ]}"#;
        let models = parse_listing(body).unwrap();
        assert_eq!(models.len(), 2, "both Ollama rows must surface");
        assert_eq!(models[0].id, "llama3.3:70b");
        assert!(
            models[0].context_window.is_none(),
            "Ollama does not emit context_length; the lookup table must not \
             invent one. Window stays None and the picker falls back to the \
             conservative default."
        );
        assert_eq!(models[1].id, "codellama:13b");
        assert!(models[1].context_window.is_none());
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

    #[tokio::test]
    async fn refresh_publishes_only_current_scope() {
        const PROVIDER: &str = "task-1b-scope";
        scenario(PROVIDER, |config, endpoint| async move {
            let (first, ()) = tokio::join!(call(&config, PROVIDER, RefreshMode::Force), async {
                let release = endpoint.next_request().await;
                endpoint.answer(LISTING).await;
                release.send(()).unwrap();
            });
            assert!(
                matches!(first, Ok(RefreshOutcome::Fresh(_))),
                "the first listing must be fresh: {first:?}"
            );
            assert!(selectable(PROVIDER, "vendor-model"));
            // The scope leaves and comes back (A -> B -> A) while the request
            // is in flight. The result belongs to a scope that no longer
            // exists, and the published rows must not survive the change.
            for response in [LISTING, NOT_A_LISTING] {
                let live_config = Arc::clone(&config);
                let endpoint2 = endpoint.clone();
                let (obsolete, ()) =
                    tokio::join!(call(&config, PROVIDER, RefreshMode::Force), async move {
                        let release = endpoint2.next_request().await;
                        let mut live = live_config.write().await;
                        let original = live.clone();
                        let mut changed = original.clone();
                        changed.providers.get_mut(PROVIDER).unwrap().api_key =
                            Some("fake-key-b".into());
                        super::super::discovery_authority::replace_config(&mut live, changed);
                        assert!(!selectable(PROVIDER, "vendor-model"));
                        super::super::discovery_authority::replace_config(&mut live, original);
                        drop(live);
                        endpoint2.answer(response).await;
                        release.send(()).unwrap();
                    });
                assert!(
                    matches!(obsolete, Err(DiscoveryError::Superseded(_))),
                    "{obsolete:?}"
                );
                assert!(!selectable(PROVIDER, "vendor-model"));
                assert!(!selectable(PROVIDER, "operator-model"));
                assert!(selectable(&other_provider(PROVIDER), "unrelated-model"));
            }
            let guard = config.read().await;
            assert!(configured_inventory(&config, &guard, PROVIDER, |_, _| None).is_none());
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn same_scope_failure_keeps_last_good_inventory() {
        const PROVIDER: &str = "task-1b-stale";
        scenario(PROVIDER, |config, endpoint| async move {
            // Two overlapping callers share one round trip.
            let mut leader = Box::pin(call(&config, PROVIDER, RefreshMode::Force));
            let mut joiner = Box::pin(call(&config, PROVIDER, RefreshMode::Force));
            assert!(futures::poll!(&mut leader).is_pending());
            assert!(futures::poll!(&mut joiner).is_pending());
            let (a, b, ()) = tokio::join!(leader, joiner, async {
                let release = endpoint.next_request().await;
                endpoint.answer(LISTING).await;
                release.send(()).unwrap();
            });
            assert!(matches!(a, Ok(RefreshOutcome::Fresh(_))), "{a:?}");
            assert!(matches!(b, Ok(RefreshOutcome::Fresh(_))), "{b:?}");
            assert!(selectable(PROVIDER, "vendor-model"));
            // The cached listing is a read: it answers without a request.
            let cached = call(&config, PROVIDER, RefreshMode::PreferFreshCache)
                .await
                .map_err(|e| e)?;
            assert!(
                matches!(cached, RefreshOutcome::CacheHit(_)),
                "{cached:?}"
            );
            endpoint.assert_idle().await;
            // A failure in the same scope keeps the last good listing.
            let (stale, ()) = tokio::join!(
                call(&config, PROVIDER, RefreshMode::Force),
                async {
                    let release = endpoint.next_request().await;
                    endpoint.answer(r#"{"data":[{"id":""}]}"#).await;
                    release.send(()).unwrap();
                }
            );
            let stale = stale.map_err(|e| e)?;
            assert!(
                matches!(stale, RefreshOutcome::Stale { cause: DiscoveryError::Shape { .. }, .. }),
                "{stale:?}"
            );
            assert!(selectable(PROVIDER, "vendor-model"));
            // An actual empty listing is authoritative: the discovered row is
            // removed, the configured roster is not.
            let (empty, ()) = tokio::join!(
                call(&config, PROVIDER, RefreshMode::Force),
                async {
                    let release = endpoint.next_request().await;
                    endpoint.answer(EMPTY_LISTING).await;
                    release.send(()).unwrap();
                }
            );
            let empty = empty.map_err(|e| e)?;
            assert!(
                matches!(empty, RefreshOutcome::Fresh(ref listing) if listing.models.is_empty()),
                "{empty:?}"
            );
            let cached = call(&config, PROVIDER, RefreshMode::PreferFreshCache)
                .await
                .map_err(|e| e)?;
            assert!(
                matches!(cached, RefreshOutcome::CacheHit(ref listing) if listing.models.is_empty()),
                "{cached:?}"
            );
            assert!(!selectable(PROVIDER, "vendor-model"));
            assert!(selectable(PROVIDER, "operator-model"));
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn same_scope_failure_without_a_prior_inventory_fails_closed() {
        const PROVIDER: &str = "task-1b-failclosed";
        scenario(PROVIDER, |config, endpoint| async move {
            let (result, ()) = tokio::join!(call(&config, PROVIDER, RefreshMode::Force), async {
                let release = endpoint.next_request().await;
                endpoint.answer(NOT_A_LISTING).await;
                release.send(()).unwrap();
            });
            match result {
                Err(DiscoveryError::Shape { .. }) => {}
                other => {
                    panic!("a failure with nothing to fall back on must fail closed: {other:?}")
                }
            }
            // Nothing was published: the configured roster survives, and no
            // discovered row exists.
            assert!(!selectable(PROVIDER, "vendor-model"));
            assert!(selectable(PROVIDER, "operator-model"));
            assert!(selectable(&other_provider(PROVIDER), "unrelated-model"));
            let guard = config.read().await;
            assert!(configured_inventory(&config, &guard, PROVIDER, |_, _| None).is_none());
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn sequential_forced_refreshes_each_reach_the_provider() {
        const PROVIDER: &str = "task-1b-force";
        scenario(PROVIDER, |config, endpoint| async move {
            // Two admin-forced refreshes back to back, well inside one clock
            // second: each one has to reach the provider. A second-resolution
            // timestamp must not turn the second call into a cache hit.
            let (first, ()) = tokio::try_join!(
                setup_refresh(&config, call(&config, PROVIDER, RefreshMode::Force)),
                async {
                    let release = endpoint.next_request().await;
                    endpoint.answer(LISTING).await;
                    release.send(()).unwrap();
                    Ok(())
                }
            )?;
            assert!(matches!(first, RefreshOutcome::Fresh(_)), "{first:?}");
            let (second, ()) = tokio::try_join!(
                setup_refresh(&config, call(&config, PROVIDER, RefreshMode::Force)),
                async {
                    let release = endpoint.next_request().await;
                    endpoint
                        .answer(r#"{"data":[{"id":"vendor-model-2"}]}"#)
                        .await;
                    release.send(()).unwrap();
                    Ok(())
                }
            )?;
            assert!(
                matches!(second, RefreshOutcome::Fresh(ref listing)
                    if listing.fetched_at == 1000
                        && listing.models.first().map(|m| m.id.as_str()) == Some("vendor-model-2")),
                "{second:?}"
            );
            // The fresh cache answers without a request.
            let cached = setup_refresh(
                &config,
                call(&config, PROVIDER, RefreshMode::PreferFreshCache),
            )
            .await?;
            assert!(matches!(cached, RefreshOutcome::CacheHit(_)), "{cached:?}");
            endpoint.assert_idle().await;
            // A forced empty listing inside the same second is a fetch, not a
            // cache hit, and it removes the discovered rows.
            let (empty, ()) = tokio::try_join!(
                setup_refresh(&config, call(&config, PROVIDER, RefreshMode::Force)),
                async {
                    let release = endpoint.next_request().await;
                    endpoint.answer(EMPTY_LISTING).await;
                    release.send(()).unwrap();
                    Ok(())
                }
            )?;
            assert!(
                matches!(empty, RefreshOutcome::Fresh(ref listing) if listing.models.is_empty()),
                "{empty:?}"
            );
            assert!(!selectable(PROVIDER, "vendor-model"));
            assert!(!selectable(PROVIDER, "vendor-model-2"));
            assert!(selectable(PROVIDER, "operator-model"));
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn a_scope_change_invalidates_the_retained_inventory_and_the_published_rows() {
        const PROVIDER: &str = "task-1b-retain";
        scenario(PROVIDER, |config, endpoint| async move {
            let (first, ()) = tokio::try_join!(
                setup_refresh(&config, call(&config, PROVIDER, RefreshMode::Force)),
                async {
                    let release = endpoint.next_request().await;
                    endpoint.answer(LISTING).await;
                    release.send(()).unwrap();
                    Ok(())
                }
            )?;
            assert!(matches!(first, RefreshOutcome::Fresh(_)), "{first:?}");
            assert!(selectable(PROVIDER, "vendor-model"));
            {
                let guard = config.read().await;
                assert!(configured_inventory(&config, &guard, PROVIDER, |_, _| None).is_some());
            }
            // The provider's effective endpoint changes. The retained listing
            // and the rows it published must not survive the change.
            {
                let mut live = config.write().await;
                let original = live.clone();
                let mut changed = original.clone();
                changed.providers.get_mut(PROVIDER).unwrap().base_url =
                    Some("http://127.0.0.1:1/v1".into());
                super::super::discovery_authority::replace_config(&mut live, changed);
            }
            assert!(!selectable(PROVIDER, "vendor-model"));
            assert!(!selectable(PROVIDER, "operator-model"));
            assert!(selectable(&other_provider(PROVIDER), "unrelated-model"));
            {
                let guard = config.read().await;
                assert!(configured_inventory(&config, &guard, PROVIDER, |_, _| None).is_none());
            }
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn new_endpoint_cannot_publish_against_the_old_runtime_transport() {
        use crate::gateway::handlers::providers::handle_models_refresh;
        use crate::gateway::protocol::JsonRpcRequest;
        use crate::gateway::security::{store::SecurityStore, SharedTokenManager};
        use serde_json::json;

        const PROVIDER: &str = "task-1b-runtime-scope";
        scenario(PROVIDER, |config, old_endpoint| async move {
            use crate::gateway::handlers::agent::{build_run_request, BuildRunError};
            use crate::gateway::model_override::ModelOverride;

            let key = crate::gateway::router::AgentRouter::new().route(None, None, None, None).await;
            let agent = crate::gateway::agent_instance::AgentInstanceConfig::default();
            let build = |model: &'static str| build_run_request(
                "endpoint-fixture".into(), &key,
                serde_json::from_value(json!({"input": "hi", "model_override": ModelOverride::Qualified { provider: PROVIDER.into(), model: model.into() }})).unwrap(),
                Some(&config), None, &agent,
            );
            let queued = build("operator-model").await.unwrap();
            queued.validate_dispatch_model().unwrap();
            let replacement = Endpoint::start().await;
            {
                let mut live = config.write().await;
                let mut changed = live.clone();
                changed.providers.get_mut(PROVIDER).unwrap().base_url =
                    Some(replacement.url.clone());
                super::super::discovery_authority::replace_config(&mut live, changed);
            }
            let dir = tempfile::tempdir().unwrap();
            let vault = Arc::new(SharedTokenManager::new(
                Arc::new(SecurityStore::in_memory().unwrap()),
                dir.path().join("test.vault"),
            ));
            let request = JsonRpcRequest::with_id(
                "providers.modelsRefresh",
                Some(json!({"provider": PROVIDER})),
                json!(1),
            );
            let before_refresh = test_revision(&config).await;
            let refresh = handle_models_refresh(request, Arc::clone(&config), vault);
            tokio::pin!(refresh);
            let response = tokio::select! {
                response = &mut refresh => response,
                release = replacement.next_request() => {
                    replacement.answer(LISTING).await;
                    release.send(()).unwrap();
                    refresh.await
                }
            };
            let result = response.result.unwrap();
            let row = &result["providers"][0];
            propagated_rpc_supersession(&config, before_refresh, PROVIDER, row).await?;
            assert_eq!(row["models"][0]["id"], "vendor-model");
            assert!(matches!(build("vendor-model").await, Err(BuildRunError::InvalidModel(_))));
            assert!(queued.validate_dispatch_model().is_err());
            old_endpoint.assert_idle().await;
            assert!(!selectable(PROVIDER, "vendor-model"));
            assert!(!selectable(PROVIDER, "operator-model"));
            assert!(selectable(&other_provider(PROVIDER), "unrelated-model"));
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn admin_refresh_reaches_member_projection_and_exact_wire_binding() {
        let _serial = SERIAL.lock().await;
        for _ in 0..8 {
            match refresh_to_wire().await {
                Ok(()) => return,
                Err(DiscoveryError::Superseded(_)) => continue,
                Err(error) => panic!("unexpected discovery failure: {error}"),
            }
        }
        panic!("the scenario kept being cancelled by unrelated mutations");
    }

    async fn propagated_rpc_supersession(
        config: &RwLock<Config>,
        before: super::super::discovery_authority::DiscoveryRevision,
        provider: &str,
        row: &serde_json::Value,
    ) -> Result<(), DiscoveryError> {
        let superseded = DiscoveryError::Superseded(provider.into());
        if row["error"].as_str() == Some(superseded.to_string().as_str()) {
            expect_observed_supersession(config, before, &Err(superseded.clone())).await;
            return Err(superseded);
        }
        Ok(())
    }

    async fn refresh_to_wire() -> Result<(), DiscoveryError> {
        use crate::gateway::handlers::{agent, chat, providers};
        use crate::gateway::model_override::ModelOverride;
        use crate::gateway::protocol::JsonRpcRequest;
        use crate::gateway::security::{store::SecurityStore, SharedTokenManager};
        use crate::orchestrator::harness_bridge::qualified_model_provider;
        use crate::providers::adapter::RequestPayload;
        use crate::providers::{create_provider, message::UnifiedMessage};
        use aleph_protocol::providers::ChatModelsListResult;
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        const PROVIDER: &str = "catalog-path-fixture";
        const MODEL: &str = "org/new-model";
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":MODEL}]})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: [DONE]\n\n",
                ),
                "text/event-stream",
            ))
            .mount(&server)
            .await;
        let (config, _) =
            fixture_with(&format!("{}/v1", server.uri()), PROVIDER, true, true, false);
        let (snapshot, named) = {
            let config = config.read().await;
            let transport = create_provider(PROVIDER, config.providers[PROVIDER].clone()).unwrap();
            let ready = BTreeSet::from([PROVIDER.to_string()]);
            let snapshot =
                SelectableSnapshot::from_config(&config, ready.clone(), &ready, |_| true)
                    .certify_discovery_transports(|name| {
                        transport_matches_listing(&config, name, transport.as_ref(), |_, _| None)
                    });
            (
                snapshot,
                std::collections::HashMap::from([(PROVIDER.into(), transport)]),
            )
        };
        crate::providers::session_model_handle::with_test_selectable_models(Some(snapshot), async {
            let dir = tempfile::tempdir().unwrap();
            let vault = Arc::new(SharedTokenManager::new(
                Arc::new(SecurityStore::in_memory().unwrap()),
                dir.path().join("test.vault"),
            ));
            let request = JsonRpcRequest::with_id(
                "providers.modelsRefresh",
                Some(json!({"provider": PROVIDER})),
                json!(1),
            );
            let before_refresh = test_revision(&config).await;
            let response =
                providers::handle_models_refresh(request, Arc::clone(&config), vault).await;
            let refresh_result = response.result.unwrap();
            let row = &refresh_result["providers"][0];
            propagated_rpc_supersession(&config, before_refresh, PROVIDER, row).await?;
            assert_eq!(row["models"][0]["id"], MODEL);
            let member_models = || {
                let response = chat::handle_models_list(JsonRpcRequest::with_id(
                    "chat.models.list",
                    None,
                    json!(2),
                ));
                serde_json::from_value::<ChatModelsListResult>(response.result.unwrap())
                    .unwrap()
                    .items
            };
            let choice = member_models()
                .into_iter()
                .find(|item| item.provider == PROVIDER && item.model == MODEL)
                .unwrap();
            let model = ModelOverride::Qualified {
                provider: choice.provider,
                model: choice.model,
            };
            let key = crate::gateway::router::AgentRouter::new()
                .route(None, None, None, None)
                .await;
            let agent = crate::gateway::agent_instance::AgentInstanceConfig::default();
            let params =
                || serde_json::from_value(json!({"input": "hi", "model_override": model})).unwrap();
            let admitted = agent::build_run_request(
                "first".into(),
                &key,
                params(),
                Some(&config),
                None,
                &agent,
            )
            .await
            .unwrap();
            assert_eq!(admitted.model_override, Some(model.clone()));
            admitted.validate_dispatch_model().unwrap();
            let Some(ModelOverride::Qualified { provider, model }) =
                admitted.model_override.clone()
            else {
                panic!("expected a qualified override");
            };
            let transport = qualified_model_provider(&named, provider, model).unwrap();
            let messages = [UnifiedMessage::user("hi")];
            transport
                .process(RequestPayload::new(&messages))
                .await
                .unwrap();

            let mut live = config.write().await;
            let mut disabled = live.clone();
            disabled.providers.get_mut(PROVIDER).unwrap().enabled = false;
            super::super::discovery_authority::replace_config(&mut live, disabled);
            assert!(admitted.validate_dispatch_model().is_err());
            let mut removed = live.clone();
            removed.providers.remove(PROVIDER);
            super::super::discovery_authority::replace_config(&mut live, removed);
            drop(live);
            assert!(admitted.validate_dispatch_model().is_err());
            assert!(!member_models().iter().any(|item| item.provider == PROVIDER));
            assert!(!selectable(PROVIDER, "operator-model"));
            let refused = agent::build_run_request(
                "second".into(),
                &key,
                params(),
                Some(&config),
                None,
                &agent,
            )
            .await;
            assert!(matches!(
                refused,
                Err(agent::BuildRunError::InvalidModel(_))
            ));
            let requests = server.received_requests().await.unwrap();
            assert_eq!(
                requests.len(),
                2,
                "expected only one listing and one inference"
            );
            assert_eq!(requests[0].url.path(), "/v1/models");
            assert_eq!(requests[1].url.path(), "/v1/chat/completions");
            let payload: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
            assert_eq!(payload["model"], MODEL);
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn list_models_refresh_reaches_the_member_resolver() {
        use crate::builtin_tools::list_models::{ListModelsArgs, ListModelsTool};
        use crate::tools::AlephTool;

        const PROVIDER: &str = "task-1b-tool-publication";
        scenario(PROVIDER, |config, endpoint| async move {
            let tool = ListModelsTool::new().with_config(config);
            let args = ListModelsArgs {
                all: false,
                refresh: true,
            };
            let (response, ()) = tokio::join!(tool.call(args), async {
                let release = endpoint.next_request().await;
                endpoint.answer(LISTING).await;
                release.send(()).unwrap();
            });
            let response = response.unwrap();
            assert!(selectable(PROVIDER, "vendor-model"));
            assert!(response
                .models
                .iter()
                .any(|choice| choice.model == "vendor-model"));
            assert!(response
                .message
                .contains("1/1 provider(s) returned fresh lists"));
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn roster_only_update_rechecks_queued_and_unqualified_choices() {
        use crate::gateway::handlers::agent::build_run_request;
        use crate::providers::session_model_handle::{selectable_models, ModelSelectionError};
        use serde_json::json;

        const PROVIDER: &str = "roster-only-fixture";
        scenario(PROVIDER, |config, _| async move {
            let key = crate::gateway::router::AgentRouter::new().route(None, None, None, None).await;
            let request = build_run_request("roster-fixture".into(), &key,
                serde_json::from_value(json!({"input": "hi", "model_override": { "kind": "qualified", "provider": PROVIDER, "model": "operator-model" }})).unwrap(),
                Some(&config), None, &crate::gateway::agent_instance::AgentInstanceConfig::default(),
            ).await.unwrap();
            request.validate_dispatch_model().unwrap();
            let mut live = config.write().await;
            let mut changed = live.clone();
            let provider = changed.providers.get_mut(PROVIDER).unwrap();
            provider.models = vec!["unrelated-model".into()];
            super::super::discovery_authority::replace_config(&mut live, changed);
            assert!(request.validate_dispatch_model().is_err());
            let snapshot = selectable_models().unwrap();
            assert!(snapshot.resolve(Some(PROVIDER), "unrelated-model").is_ok());
            assert!(matches!(snapshot.resolve_raw("unrelated-model"), Err(ModelSelectionError::Ambiguous { .. })));
            Ok(())
        }).await;
    }

    #[tokio::test]
    async fn credential_revocation_refuses_a_previously_admitted_configured_model() {
        use crate::gateway::handlers::agent::build_run_request;
        use crate::gateway::security::{store::SecurityStore, SharedTokenManager};
        use crate::providers::session_model_handle::{
            selectable_models, with_test_selectable_models,
        };
        use serde_json::json;

        const PROVIDER: &str = "queued-vault-fixture";
        scenario_with_credentials(PROVIDER, true, |config, endpoint| async move {
            let dir = tempfile::tempdir().unwrap();
            let store_path = dir.path().join("security.db");
            let vault = SharedTokenManager::new(Arc::new(SecurityStore::open(&store_path).unwrap()), dir.path().join("test.vault"));
            vault.generate_token().unwrap();
            let vault_key = crate::providers::probe::provider_vault_key(PROVIDER);
            vault.store_secret(&vault_key, "fixture-token").unwrap();
            let mut boot = config.read().await.clone();
            boot.providers.get_mut(PROVIDER).unwrap().api_key = Some(String::new());
            *config.write().await = boot.clone();
            let available = BTreeSet::from([PROVIDER.to_string()]);
            let mounted = SelectableSnapshot::from_config(&boot, available.clone(), &available, |_| true);
            with_test_selectable_models(Some(mounted), async {
                let key = crate::gateway::router::AgentRouter::new().route(None, None, None, None).await;
                let request = build_run_request("queued-fixture".into(), &key,
                    serde_json::from_value(json!({"input": "hi", "model_override": { "kind": "qualified", "provider": PROVIDER, "model": "operator-model" }})).unwrap(),
                    Some(&config), None, &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                ).await.unwrap();
                request.validate_dispatch_model().unwrap();
                vault.reset_token().unwrap();
                request.validate_dispatch_model().unwrap();
                assert_eq!(vault.get_secret(&vault_key).unwrap().unwrap().expose(), "fixture-token");
                let connection = rusqlite::Connection::open(&store_path).unwrap();
                connection.execute_batch("CREATE TRIGGER refuse_fixture_rotation BEFORE INSERT ON shared_token BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
                assert!(vault.reset_token().is_err());
                request.validate_dispatch_model().unwrap();
                assert_eq!(vault.get_secret(&vault_key).unwrap().unwrap().expose(), "fixture-token");
                // OAuth logout deletes this key before saving config. The vault
                // operation must revoke queued bindings even before that save.
                vault.delete_secret(&vault_key).unwrap();
                assert!(request.validate_dispatch_model().is_err());
                assert!(!selectable_models().unwrap().choices().iter().any(|item| item.provider == PROVIDER));
                endpoint.assert_idle().await;
                Ok(())
            }).await
        }).await;
    }

    #[tokio::test]
    async fn a_vault_credential_change_revokes_only_the_vault_backed_inventory() {
        const PROVIDER: &str = "task-1b-vault";
        scenario(PROVIDER, |config, endpoint| async move {
            let other = other_provider(PROVIDER);
            let other_endpoint = Endpoint::start().await;
            {
                let mut live = config.write().await;
                // The provider's credential comes from the vault, not the config.
                live.providers.get_mut(PROVIDER).unwrap().api_key = None;
                // The unrelated provider keeps its credential in the config.
                let unrelated = live.providers.get_mut(&other).unwrap();
                unrelated.base_url = Some(other_endpoint.url.clone());
                unrelated.api_key = Some("fake-key-other".into());
            }
            let (vault_listing, ()) = tokio::join!(
                call_with(&config, PROVIDER, RefreshMode::Force, |_, _| {
                    Some("vault-secret-key".into())
                }),
                async {
                    let release = endpoint.next_request().await;
                    endpoint.answer(LISTING).await;
                    release.send(()).unwrap();
                }
            );
            assert!(matches!(vault_listing?, RefreshOutcome::Fresh(_)));
            let (other_listing, ()) =
                tokio::join!(call(&config, &other, RefreshMode::Force), async {
                    let release = other_endpoint.next_request().await;
                    other_endpoint.answer(LISTING).await;
                    release.send(()).unwrap();
                });
            assert!(
                matches!(other_listing, Ok(RefreshOutcome::Fresh(_))),
                "{other_listing:?}"
            );
            // A vault write for the provider's credential revokes its
            // inventory, and only its own: the unrelated provider's listing
            // is not vault-backed and must survive.
            {
                let mut fence = super::super::discovery_authority::mutation_fence();
                fence.vault_changed(|provider| provider == PROVIDER);
            }
            let guard = config.read().await;
            assert!(configured_inventory(&config, &guard, PROVIDER, |_, _| {
                Some("vault-secret-key".into())
            })
            .is_none());
            assert!(configured_inventory(&config, &guard, &other, |_, _| None).is_some());
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn a_vault_mutation_during_a_fetch_cannot_be_raced_by_a_stale_publication() {
        const PROVIDER: &str = "task-1b-vaultrace";
        scenario_with_credentials(PROVIDER, true, |config, endpoint| async move {
            use crate::gateway::security::{store::SecurityStore, SharedTokenManager};
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(SecurityStore::in_memory().unwrap());
            let vault = SharedTokenManager::new(store, dir.path().join("test.vault"));
            vault.generate_token().unwrap();
            let vault_key = crate::providers::probe::provider_vault_key(PROVIDER);
            vault.store_secret(&vault_key, "vault-test-a").unwrap();
            let (_, snapshot) = fixture_with(&endpoint.url, PROVIDER, true, true, true);
            {
                let guard = config.read().await;
                assert!(guard.providers[PROVIDER].api_key.is_none());
                let mut transport_config = guard.providers[PROVIDER].clone();
                transport_config.api_key = Some("vault-test-a".into());
                let installed =
                    crate::providers::create_provider(PROVIDER, transport_config).unwrap();
                assert!(transport_matches_listing(
                    &guard,
                    PROVIDER,
                    installed.as_ref(),
                    |_, _| {
                        vault
                            .get_secret(&vault_key)
                            .unwrap()
                            .map(|secret| secret.expose().to_owned())
                    }
                ));
            }
            crate::providers::session_model_handle::with_test_selectable_models(
                Some(snapshot),
                async {
                    let credential = |_: &str, _: &ProviderConfig| {
                        vault
                            .get_secret(&vault_key)
                            .unwrap()
                            .map(|secret| secret.expose().to_owned())
                    };
                    let refresh = || call_with(&config, PROVIDER, RefreshMode::Force, credential);
                    // The boot snapshot was constructed with a vault-backed provider.
                    // A first, successful listing: its rows are published.
                    let (first, ()) = tokio::try_join!(setup_refresh(&config, refresh()), async {
                        let release = endpoint.next_request().await;
                        endpoint.answer(LISTING).await;
                        release.send(()).unwrap();
                        Ok(())
                    })?;
                    assert!(matches!(first, RefreshOutcome::Fresh(_)), "{first:?}");
                    assert!(selectable(PROVIDER, "vendor-model"));
                    // A second fetch is in flight when the credential is replaced in
                    // the vault. The vault write takes the revision fence, so it lands
                    // either before the commit (the fetch is superseded) or after it
                    // (the rows are revoked). It can never be interleaved between the
                    // scope check and the publication.
                    let before_fetch = {
                        let guard = config.read().await;
                        discovery_revision(&guard)
                    };
                    let vault_changed = std::cell::Cell::new(false);
                    let refresh_until_mutation = || async {
                        let result = refresh().await;
                        // An unrelated concurrent revision may prevent the HTTP
                        // request from starting at all. Retry the fixture rather
                        // than keep waiting for a request that cannot arrive.
                        if !vault_changed.get()
                            && matches!(result, Err(DiscoveryError::Superseded(_)))
                        {
                            expect_observed_supersession(&config, before_fetch, &result).await;
                            return result.map(Ok);
                        }
                        Ok(result)
                    };
                    let (leader, waiter, ()) = tokio::try_join!(
                        refresh_until_mutation(),
                        refresh_until_mutation(),
                        async {
                            let release = endpoint.next_request().await;
                            tokio::time::timeout(Duration::from_secs(5), async {
                                loop {
                                    let queued = FLIGHTS.lock().unwrap().iter().any(
                                        |((name, _, _), flight)| {
                                            name == PROVIDER && flight.strong_count() == 2
                                        },
                                    );
                                    if queued {
                                        return Ok(());
                                    }
                                    {
                                        let guard = config.read().await;
                                        if discovery_revision(&guard) != before_fetch {
                                            return Err(DiscoveryError::Superseded(
                                                PROVIDER.into(),
                                            ));
                                        }
                                    }
                                    tokio::task::yield_now().await;
                                }
                            })
                            .await
                            .expect("the second refresh did not join the first")?;
                            assert!(vault.delete_secret(&vault_key).unwrap());
                            vault_changed.set(true);
                            assert!(!selectable(PROVIDER, "vendor-model"));
                            vault.store_secret(&vault_key, "vault-test-b").unwrap();
                            vault.store_secret(&vault_key, "vault-test-a").unwrap();
                            endpoint.answer(LISTING).await;
                            release.send(()).unwrap();
                            Ok(())
                        }
                    )?;
                    for result in [leader, waiter] {
                        assert!(
                            matches!(result, Err(DiscoveryError::Superseded(_))),
                            "{result:?}"
                        );
                    }
                    endpoint.assert_idle().await;
                    // The revoked route is not restored by the stale result.
                    assert!(!selectable(PROVIDER, "vendor-model"));
                    assert!(!selectable(PROVIDER, "operator-model"));
                    let guard = config.read().await;
                    assert!(configured_inventory(&config, &guard, PROVIDER, credential).is_none());
                    Ok(())
                },
            )
            .await
        })
        .await;
    }

    #[tokio::test]
    async fn named_admin_refresh_of_disabled_provider_does_not_publish() {
        const PROVIDER: &str = "task-1b-disabled";
        scenario(PROVIDER, |config, endpoint| async move {
            let original = config.read().await.clone();
            let mut disabled = original.clone();
            disabled.providers.get_mut(PROVIDER).unwrap().enabled = false;
            super::super::discovery_authority::replace_config(&mut *config.write().await, disabled);

            for mode in [RefreshMode::Force, RefreshMode::PreferFreshCache] {
                assert!(matches!(
                    call(&config, PROVIDER, mode).await,
                    Err(DiscoveryError::Unsupported(_))
                ));
            }
            let (diagnostic, ()) =
                tokio::join!(call(&config, PROVIDER, RefreshMode::ForceNamed), async {
                    let release = endpoint.next_request().await;
                    endpoint.answer(LISTING).await;
                    release.send(()).unwrap();
                });
            assert!(matches!(diagnostic?, RefreshOutcome::Fresh(_)));
            assert!(!selectable(PROVIDER, "vendor-model"));

            super::super::discovery_authority::replace_config(&mut *config.write().await, original);
            assert!(!selectable(PROVIDER, "vendor-model"));
            let guard = config.read().await;
            assert!(configured_inventory(&config, &guard, PROVIDER, |_, _| None).is_none());
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn discovery_transport_errors_do_not_retain_endpoint_or_key() {
        let endpoint = "http://test-user:test-password@[invalid?key=test-query";
        let error = fetch_listing("test-provider", endpoint, "anthropic", "test-header")
            .await
            .unwrap_err();
        assert!(matches!(error, DiscoveryError::Transport { .. }));
        for text in [error.to_string(), format!("{error:?}")] {
            for secret in ["test-user", "test-password", "test-query", "test-header"] {
                assert!(!text.contains(secret));
            }
        }
    }

    #[tokio::test]
    async fn discovery_does_not_grant_provider_readiness() {
        const PROVIDER: &str = "task-1b-readiness";
        scenario(PROVIDER, |_config, endpoint| async move {
            for (ready, authenticated) in [(true, false), (false, true)] {
                let (config, snapshot) =
                    fixture_with(&endpoint.url, PROVIDER, ready, authenticated, false);
                crate::providers::session_model_handle::with_test_selectable_models(
                    Some(snapshot),
                    async {
                        let (result, ()) =
                            tokio::join!(call(&config, PROVIDER, RefreshMode::Force), async {
                                let release = endpoint.next_request().await;
                                endpoint.answer(LISTING).await;
                                release.send(()).unwrap();
                            });
                        assert!(matches!(result, Ok(RefreshOutcome::Fresh(_))), "{result:?}");
                        // A listing never makes an unpublished provider
                        // selectable, and it never grants readiness.
                        assert!(!selectable(PROVIDER, "vendor-model"));
                        assert!(selectable(&other_provider(PROVIDER), "unrelated-model"));
                    },
                )
                .await;
            }
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn no_credential_material_is_retained_in_the_listing_or_the_snapshot() {
        const PROVIDER: &str = "task-1b-secret";
        scenario(PROVIDER, |config, endpoint| async move {
            let (listing, ()) = tokio::join!(call(&config, PROVIDER, RefreshMode::Force), async {
                let release = endpoint.next_request().await;
                endpoint.answer(LISTING).await;
                release.send(()).unwrap();
            });
            let listing = listing.map_err(|e| e)?;
            assert!(!format!("{listing:?}").contains("fake-key-a"));
            let snapshot = crate::providers::session_model_handle::selectable_models().unwrap();
            assert!(!format!("{snapshot:?}").contains("fake-key-a"));
            // The listing itself is published.
            assert!(snapshot.resolve(Some(PROVIDER), "vendor-model").is_ok());
            Ok(())
        })
        .await;
    }
}
