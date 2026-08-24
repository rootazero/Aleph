//! Runtime self-heal for the Codex/ChatGPT OAuth access token.
//!
//! ## Why this exists
//!
//! The OAuth access token is captured into the live provider at build time
//! (`HttpProvider` owns a frozen `ProviderConfig.api_key`). OAuth access tokens
//! expire after ~1 hour, so a long-running task that crosses that boundary
//! starts getting `401 token_expired`. Nothing on the request path refreshed
//! the token — the only refresh caller was the Panel's `providers.oauthStatus`
//! poll, which updates config/vault but never reaches the already-built
//! provider. The 401 was then classified transient and retried against the
//! *same* stale token until the whole task hard-failed.
//!
//! This service closes that gap with two triggers, both reusing the exact same
//! self-heal primitive [`CodexTokenRefresher::refresh_and_swap`]:
//!
//! * **Proactive** — a background loop refreshes a few minutes before expiry
//!   ([`CodexTokenRefresher::spawn_background`]).
//! * **Reactive** — the dispatch retry path calls `refresh_and_swap` when it
//!   sees a `token_expired` error before retrying.
//!
//! ## Layering note (R1 / R10)
//!
//! Token refresh is *infrastructure* (a limb refreshing its own credential),
//! not harness cognition. It lives in the Gateway, not `src/harness/`, and the
//! dumb loop only calls into it on a specific auth error — it makes no recovery
//! *strategy* decision. The provider hot-swap reuses the already-proven
//! `providers.setDefault` mechanism: the failover chain's primary slot is the
//! live `MultiProviderRegistry`, so re-registering `chatgpt` with a fresh-token
//! instance is visible on the next dispatch turn with no restart.

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::config::Config;
use crate::gateway::handlers::oauth::{try_refresh, SharedOAuthState};
use crate::gateway::security::SharedTokenManager;
use crate::sync_primitives::Arc;
use crate::thinker::MultiProviderRegistry;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Process-wide handle so the dispatch retry path (which lives in the generic
/// `ExecutionEngine`, built before `oauth_state` exists at boot) can reach the
/// refresher without threading it through every constructor. Mirrors the
/// existing global-registry pattern (`shared_skill_system`, scratchpad/process
/// registries). `None` in tests and when OAuth is not configured.
///
/// `FailsClosed`: `run_loop/inner.rs` skips the refresh-and-swap block, and the
/// retry it was about to guard happens anyway — against the same stale token,
/// until the run hard-fails. That is the exact failure the module doc above
/// says this service exists to remove, restored in silence. Safe (no credential
/// is used that should not be) and mute (the run's error blames the provider).
static GLOBAL: CapabilitySlot<Arc<CodexTokenRefresher>> = CapabilitySlot::new(
    "gateway/codex-token-refresher",
    MissingSemantics::FailsClosed,
);

/// Install the process-wide refresher. First writer wins; later calls are
/// ignored (boot installs exactly once).
pub fn set_global(refresher: Arc<CodexTokenRefresher>) {
    let _ = GLOBAL.install(refresher);
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn global_slot() -> &'static dyn SlotStatus {
    &GLOBAL
}

/// The process-wide refresher, if one was installed at boot.
pub fn global() -> Option<Arc<CodexTokenRefresher>> {
    GLOBAL.get().cloned()
}

/// Canonical provider name for the Codex/ChatGPT OAuth backend.
const PROVIDER_NAME: &str = "chatgpt";

/// Refresh this many seconds before the token actually expires, so a request
/// never races a just-expired token (covers normal clock skew).
const REFRESH_MARGIN_SECS: u64 = 300;

/// Background poll interval.
const BACKGROUND_TICK: Duration = Duration::from_secs(60);

/// Whether a provider error message indicates an expired OAuth access token.
///
/// Intentionally narrow: a generic 401 from a mistyped API key must NOT trigger
/// a refresh (it would burn the refresh grant for nothing). We match the
/// distinctive expiry wording the `OpenAI` Responses API returns
/// ("`token_expired`" / "authentication token is expired"). Case-insensitive.
#[must_use]
pub fn is_oauth_token_expired_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("token_expired")
        || lower.contains("token is expired")
        || lower.contains("token has expired")
        || lower.contains("authentication token is expired")
}

/// Self-heal coordinator for the Codex/ChatGPT OAuth token.
pub struct CodexTokenRefresher {
    oauth_state: SharedOAuthState,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
    registry: Arc<MultiProviderRegistry>,
}

impl CodexTokenRefresher {
    pub const fn new(
        oauth_state: SharedOAuthState,
        config: Arc<RwLock<Config>>,
        vault: Arc<SharedTokenManager>,
        registry: Arc<MultiProviderRegistry>,
    ) -> Self {
        Self {
            oauth_state,
            config,
            vault,
            registry,
        }
    }

    /// Seconds until the cached token expires. `None` when there is no token,
    /// `Some(0)` when already expired.
    pub async fn seconds_until_expiry(&self) -> Option<u64> {
        let guard = self.oauth_state.read().await;
        let cache = guard.as_ref()?;
        Some(cache.expires_in_seconds().unwrap_or(0))
    }

    /// Refresh the OAuth token and hot-swap the running provider so the next
    /// dispatch turn uses the fresh token. Returns `true` when the live
    /// provider now carries a renewed token.
    ///
    /// Idempotent and cheap to call spuriously: if there is no token or no
    /// refresh token, [`try_refresh`] returns `false` and nothing is swapped.
    pub async fn refresh_and_swap(&self) -> bool {
        let refreshed =
            try_refresh(&self.oauth_state, &self.config, &self.vault, PROVIDER_NAME).await;

        if !refreshed {
            return false;
        }

        self.swap_provider().await;
        true
    }

    /// Rebuild the `chatgpt` provider with the freshly-stored token and register
    /// it into the live registry. Reuses the `providers.setDefault` swap path:
    /// `try_refresh` already wrote the new access token to the vault, so we
    /// rebuild from config + vault exactly like the Settings UI does.
    async fn swap_provider(&self) {
        // Capture the provider config (api_key is `#[serde(skip)]` → inject the
        // freshly-refreshed token from the vault).
        let provider_config = {
            let cfg = self.config.read().await;
            let Some(pc) = cfg.providers.get(PROVIDER_NAME) else {
                debug!("codex refresh: no chatgpt provider in config; skip swap");
                return;
            };
            let mut pc = pc.clone();
            if pc.api_key.as_ref().is_none_or(|k| k.is_empty()) {
                match self.vault.get_secret(&format!("ai:{PROVIDER_NAME}")) {
                    Ok(Some(secret)) => pc.api_key = Some(secret.expose().to_string()),
                    _ => {
                        warn!("codex refresh: refreshed token missing from vault; skip swap");
                        return;
                    }
                }
            }
            pc
        };

        match crate::providers::create_provider(PROVIDER_NAME, provider_config) {
            Ok(provider) => {
                self.registry.register(PROVIDER_NAME.to_string(), provider);
                // Clear the circuit-breaker state recorded against the stale
                // token so the renewed provider is tried immediately. This is
                // the breaker that decides whether the request is dialed at all
                // — an expired token trips it as a *permanent* failure, i.e. the
                // full 10-minute cooldown, so skipping this reset would strand
                // the freshly-renewed token behind the failure it just fixed.
                // (A second reset against the registry's own health map used to
                // sit here; that map fed only the resolution badge and gated no
                // dial, and is gone.)
                if let Some(obs) = crate::providers::route_observe::global_route_observability() {
                    obs.health.reset(PROVIDER_NAME).await;
                }
                info!("codex refresh: hot-swapped chatgpt provider with renewed token");
            }
            Err(e) => warn!(error = %e, "codex refresh: failed to rebuild chatgpt provider"),
        }
    }

    /// Spawn the proactive background refresh loop. Ticks every minute and
    /// refreshes once the token is within [`REFRESH_MARGIN_SECS`] of expiry.
    pub fn spawn_background(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(BACKGROUND_TICK).await;

                let due = match self.seconds_until_expiry().await {
                    // No token configured — nothing to refresh.
                    None => continue,
                    Some(secs) => secs <= REFRESH_MARGIN_SECS,
                };

                if due {
                    debug!("codex refresh: token near expiry, refreshing proactively");
                    if self.refresh_and_swap().await {
                        info!("codex refresh: proactive token refresh succeeded");
                    } else {
                        // Refresh failed (e.g. refresh token revoked). Back off
                        // a tick; oauthStatus will surface a re-login prompt.
                        warn!("codex refresh: proactive refresh failed; will retry next tick");
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_slot();
        assert_eq!(slot.id(), "gateway/codex-token-refresher");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }

    #[test]
    fn detects_token_expired_errors() {
        assert!(is_oauth_token_expired_error(
            "OpenAI Responses API error (401 Unauthorized): { \"error\": { \"code\": \"token_expired\" } }"
        ));
        assert!(is_oauth_token_expired_error(
            "Provided authentication token is expired. Please try signing in again."
        ));
        assert!(is_oauth_token_expired_error("TOKEN HAS EXPIRED"));
    }

    #[test]
    fn ignores_unrelated_auth_errors() {
        // A mistyped API key 401 must not trigger a refresh.
        assert!(!is_oauth_token_expired_error(
            "OpenAI API error (401 Unauthorized): incorrect api key provided"
        ));
        assert!(!is_oauth_token_expired_error(
            "403 Forbidden: insufficient quota"
        ));
        assert!(!is_oauth_token_expired_error("rate limited (429)"));
    }
}
