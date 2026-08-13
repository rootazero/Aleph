//! WebFetch-based search SERP fallback (Round-2).
//!
//! When **every** configured search provider in [`SearchRegistry`] has
//! failed (rate-limit, auth, network, dead engine) the registry consults
//! this module as the last resort: it scrapes well-known no-credential
//! SERP mirrors (DDG Lite, DDG HTML) over raw HTTP and reuses the
//! per-provider parsers from [`crate::search::providers::duckduckgo`].
//!
//! ## Why this layer exists
//!
//! Operators routinely hit "all paid APIs are rate-limited at the same
//! time" because they share an IP pool. Even the zero-config DDG
//! provider can hit a CAPTCHA challenge. A fallback layer that
//! - hits a **different endpoint** (DDG Lite vs DDG HTML),
//! - uses a **different User-Agent** than the primary DDG provider, and
//! - **cools down** a mirror that returns 0 results
//!
//! keeps the search tool functional during transient outages without
//! requiring users to predict every failure mode in advance.
//!
//! ## Why not piped through `WebFetchTool`?
//!
//! [`crate::builtin_tools::web_fetch::WebFetchTool`] extracts the page
//! to Markdown via the Readability algorithm, which destroys the
//! result-list structure we need to parse. The fallback uses raw HTTP
//! and feeds the body straight into the DDG selector parsers — same
//! discipline as [`crate::search::providers::duckduckgo::DuckDuckGoProvider`].
//!
//! ## Architectural conformance
//!
//! - **R3 (Core Minimalism)**: zero new dependencies — reuses `reqwest`,
//!   `scraper`, the existing DDG parsers, and `SearchOptions` mapping.
//! - **R7 (LLM Sovereignty)**: fallback ORDER is deterministic; the LLM
//!   does not pick mirrors, only consumes the unified results.
//! - **R10 (Thin Harness)**: this is search-domain code, not harness;
//!   the search/Think loop is unchanged. Failure handling is pure
//!   mechanical iteration over a fixed list.

use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status};
use crate::search::providers::duckduckgo::{parse_ddg_html, parse_ddg_lite_html};
use crate::search::{SearchOptions, SearchResult};
use crate::sync_primitives::Mutex;
use reqwest::Client;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Time a mirror is silenced after returning an error or zero results.
/// 5 min matches the `SearchRegistry` provider-test cache TTL — a value
/// long enough that the next agent iteration won't re-probe a dead
/// mirror, short enough that a 15-min outage recovers automatically
/// without process restart.
const MIRROR_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// How the fallback parses the HTTP body from each mirror.
#[derive(Debug, Clone, Copy)]
enum MirrorKind {
    /// `lite.duckduckgo.com/lite/` — table layout, `a.result-link` + `td.result-snippet`.
    DdgLite,
    /// `html.duckduckgo.com/html/` — div layout, `div.result > a.result__a`.
    DdgHtml,
}

/// One entry in the fallback chain. Fixed at compile time — runtime
/// configuration would invite the same "operator picks wrong mirror"
/// failure mode we're trying to escape.
#[derive(Debug, Clone)]
struct Mirror {
    /// Stable identifier used in logs, error messages, and the
    /// `provider` attribution on returned `SearchResult`s.
    name: &'static str,
    /// Fully-qualified HTTPS endpoint. Query is appended via
    /// `reqwest::RequestBuilder::query`.
    endpoint: &'static str,
    kind: MirrorKind,
    /// Per-mirror UA. Distinct from the primary `DuckDuckGoProvider` UA
    /// so simultaneous fire-and-fallback paths don't share a rate-limit
    /// bucket on DDG's side.
    user_agent: &'static str,
}

/// The compile-time fallback chain. Order matters: cheapest / least
/// blocked endpoint first.
///
/// **Editing this list?** Each new mirror needs (a) a `MirrorKind` arm
/// in [`WebFetchSerpFallback::try_mirror`], (b) a parser in
/// `providers::duckduckgo` or sibling module exposed as `pub(crate)`.
/// Resist the urge to make this user-configurable — that re-creates
/// the very class of failure (operator picks a dead mirror) the
/// fallback exists to insulate against.
const FALLBACK_MIRRORS: &[Mirror] = &[
    Mirror {
        name: "ddg-lite",
        endpoint: "https://lite.duckduckgo.com/lite/",
        kind: MirrorKind::DdgLite,
        // Mobile UA → Lite endpoint serves the smaller payload reliably
        // and is less prone to challenge pages than the desktop HTML
        // endpoint.
        user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    },
    Mirror {
        name: "ddg-html",
        endpoint: "https://html.duckduckgo.com/html/",
        kind: MirrorKind::DdgHtml,
        // Linux Chrome — deliberately different from
        // DuckDuckGoProvider::USER_AGENT (macOS Chrome) so the two
        // paths don't share a rate-limit bucket.
        user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36",
    },
];

/// WebFetch-based SERP fallback that scrapes no-credential search
/// mirrors when the configured provider chain is exhausted.
///
/// Holds:
/// - A shared `reqwest::Client` (30s connect timeout via [`build_client`]).
/// - A per-mirror cool-down map so a 0-result / error mirror is silenced
///   for [`MIRROR_COOLDOWN`] instead of being hammered on every call.
///
/// Cool-downs are **per-process**; restarting `aleph-server` clears them.
/// That is the intentional behaviour: a stale cool-down should not
/// outlive a process the operator chose to restart.
#[derive(Debug)]
pub struct WebFetchSerpFallback {
    client: Client,
    /// `mirror.name → Instant of last failure`. Reads acquire under
    /// `Mutex` for simplicity — call frequency is tiny (only when the
    /// full provider chain has already failed) so lock contention is a
    /// non-issue.
    cooldowns: Mutex<HashMap<&'static str, Instant>>,
}

impl WebFetchSerpFallback {
    /// Construct with a fresh HTTP client. Returns `Err` only if
    /// `reqwest::Client::builder()` itself fails (effectively
    /// impossible in the default build).
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: build_client()?,
            cooldowns: Mutex::new(HashMap::new()),
        })
    }

    /// Try each mirror in order; return the first non-empty result set.
    ///
    /// Returns `Err(AlephError::ProviderError)` only when **every**
    /// mirror in the chain either errored, returned 0 results, or was
    /// cooling down. The aggregated error string lists the per-mirror
    /// reason so operators can diagnose which mirror is the bottleneck.
    pub async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let mut errors: Vec<String> = Vec::with_capacity(FALLBACK_MIRRORS.len());
        for mirror in FALLBACK_MIRRORS {
            if self.is_cooling_down(mirror.name) {
                errors.push(format!("{} cooling down", mirror.name));
                continue;
            }
            match self.try_mirror(mirror, query, options).await {
                Ok(results) if !results.is_empty() => {
                    log::info!(
                        target: "search",
                        "web-fetch fallback succeeded via mirror={} count={}",
                        mirror.name,
                        results.len(),
                    );
                    return Ok(results);
                }
                Ok(_) => {
                    self.note_failure(mirror.name);
                    errors.push(format!("{} returned 0 results", mirror.name));
                    log::warn!(
                        target: "search",
                        "web-fetch fallback mirror={} returned 0 results — cool-down armed",
                        mirror.name,
                    );
                }
                Err(e) => {
                    self.note_failure(mirror.name);
                    let kind = super::registry::classify_search_error(&e);
                    errors.push(format!("{} [{}] {}", mirror.name, kind, e));
                    log::warn!(
                        target: "search",
                        "web-fetch fallback mirror={} kind={} {}",
                        mirror.name,
                        kind,
                        e,
                    );
                }
            }
        }
        Err(AlephError::provider(format!(
            "WebFetch SERP fallback exhausted: {}",
            errors.join("; ")
        )))
    }

    /// Execute a single mirror fetch + parse.
    async fn try_mirror(
        &self,
        mirror: &Mirror,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        let response = self
            .client
            .get(mirror.endpoint)
            .query(&[("q", query)])
            .header("User-Agent", mirror.user_agent)
            .header("Accept", "text/html")
            .timeout(Duration::from_secs(options.validated_timeout()))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;
        let response = check_status(response, mirror.name)?;
        let body = response.text().await.map_err(|e| {
            AlephError::provider(format!("Failed to read {} body: {}", mirror.name, e))
        })?;

        let parsed = match mirror.kind {
            MirrorKind::DdgHtml => parse_ddg_html(&body, options.validated_max_results()),
            MirrorKind::DdgLite => parse_ddg_lite_html(&body, options.validated_max_results()),
        };
        // Re-tag attribution: "fallback:ddg-lite" distinguishes a
        // result that took the fallback path from one served by the
        // primary `duckduckgo` provider. Operators rely on this in
        // logs and quota dashboards.
        let attributed: Vec<SearchResult> = parsed
            .into_iter()
            .map(|mut r| {
                r.provider = Some(format!("fallback:{}", mirror.name));
                r
            })
            .collect();
        Ok(attributed)
    }

    fn is_cooling_down(&self, name: &'static str) -> bool {
        self.lock_cooldowns()
            .get(name)
            .is_some_and(|t| t.elapsed() < MIRROR_COOLDOWN)
    }

    fn note_failure(&self, name: &'static str) {
        self.lock_cooldowns().insert(name, Instant::now());
    }

    /// Acquire the cooldown map lock, recovering from poisoning.
    ///
    /// Cool-down writes are non-panicking (`HashMap::insert`), so the
    /// only way the mutex can be poisoned is by a panic on a different
    /// thread *while* the lock is held — which the existing code
    /// (`is_cooling_down`, `note_failure`) cannot trigger. Recovering
    /// via `into_inner()` rather than propagating the poison error
    /// keeps the fallback path infallible: a stale poison would
    /// otherwise lock out every mirror until process restart.
    fn lock_cooldowns(&self) -> std::sync::MutexGuard<'_, HashMap<&'static str, Instant>> {
        self.cooldowns.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Test-only: drop all cool-down state. Wiring tests want to
    /// re-attempt the same mirror without waiting 5 min.
    #[cfg(test)]
    pub(crate) fn clear_cooldowns(&self) {
        self.lock_cooldowns().clear();
    }

    /// Test-only: number of mirrors compiled into the fallback chain.
    /// Wiring tests assert this is non-zero so silent removal of the
    /// last mirror trips CI.
    #[cfg(test)]
    pub(crate) fn mirror_count() -> usize {
        FALLBACK_MIRRORS.len()
    }

    /// Test-only: directly poke a mirror into cool-down so wiring
    /// tests can verify the silenced branch.
    #[cfg(test)]
    pub(crate) fn force_cooldown_for(&self, name: &'static str) {
        self.note_failure(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_chain_is_non_empty() {
        // R10 canary: if someone empties FALLBACK_MIRRORS the registry
        // silently degrades to "no fallback". Make sure CI screams.
        assert!(
            WebFetchSerpFallback::mirror_count() >= 2,
            "fallback chain must keep at least 2 mirrors so the cool-down branch \
             has somewhere to fall through to",
        );
    }

    #[test]
    fn fallback_mirror_names_are_unique() {
        // cool-down map keys mirror names — duplicates would silently
        // collapse independent mirrors' state.
        let mut names: Vec<&'static str> = FALLBACK_MIRRORS.iter().map(|m| m.name).collect();
        names.sort();
        let pre = names.len();
        names.dedup();
        assert_eq!(pre, names.len(), "duplicate mirror names: {names:?}");
    }

    #[test]
    fn fallback_constructs() {
        let f = WebFetchSerpFallback::new().expect("construct");
        assert!(!f.is_cooling_down("ddg-lite"));
        assert!(!f.is_cooling_down("ddg-html"));
    }

    #[test]
    fn cooldown_silences_then_clears() {
        let f = WebFetchSerpFallback::new().unwrap();
        f.force_cooldown_for("ddg-lite");
        assert!(f.is_cooling_down("ddg-lite"));
        assert!(!f.is_cooling_down("ddg-html"));
        f.clear_cooldowns();
        assert!(!f.is_cooling_down("ddg-lite"));
    }

    #[tokio::test]
    async fn search_returns_error_when_all_mirrors_cooling_down() {
        // Pre-cool every mirror — `.search()` must short-circuit and
        // return the aggregated error without making any network call.
        let f = WebFetchSerpFallback::new().unwrap();
        for m in FALLBACK_MIRRORS {
            f.force_cooldown_for(m.name);
        }
        let opts = SearchOptions::default();
        let err = f.search("rust async", &opts).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("WebFetch SERP fallback exhausted"),
            "wrong aggregate error: {msg}",
        );
        for m in FALLBACK_MIRRORS {
            assert!(
                msg.contains(m.name),
                "aggregate error missing mirror name '{}': {msg}",
                m.name,
            );
        }
    }
}
