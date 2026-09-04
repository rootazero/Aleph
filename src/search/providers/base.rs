//! The one place a search provider's HTTP call turns into an `AlephError`.
//!
//! # Why one funnel
//!
//! Two cross-cutting rules apply to every provider's error path, and both
//! used to be one provider's private answer:
//!
//! * **Credentials must not survive into an error string.** `google.rs` had
//!   `sanitize_api_key` + a forked `check_status_google`, because Google CSE
//!   is the one backend that puts its key in the query string and `reqwest`
//!   errors quote the URL. Nothing told the next provider that the rule
//!   existed, and the eight that used the shared helpers got no redaction at
//!   all.
//! * **An error has to name a lever.** The old `check_status` reported the
//!   status code and dropped the body, so a Tavily 400 reached the model —
//!   through the registry's `name [kind] message` report — as
//!   `tavily [provider] tavily API error: 400 Bad Request`. The vendor had
//!   said *which* parameter it disliked, in the body, and we threw it away.
//!
//! Both are enforced structurally rather than by convention: [`send`] is the
//! only public way to dispatch, [`check_status`] is private to this module,
//! and `error_funnel_census.rs` asserts that no provider mints an
//! `AlephError` from a `reqwest` value on its own.

use crate::error::{AlephError, Result};
use crate::search::SearchResult;
use reqwest::{RequestBuilder, Response, StatusCode};
use std::time::Duration;

/// How much of a failing response's body reaches the error message.
///
/// Enough for a vendor's `{"error":"..."}` envelope, short enough that a
/// backend answering with an HTML error page cannot spend a caller's context
/// window. The excerpt is the *only* part of the body that is ever kept — a
/// success body goes to `parse_json` and never here.
const ERROR_BODY_MAX_CHARS: usize = 300;

/// Build a default HTTP client with 30-second request timeout.
pub(crate) fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| AlephError::network(e.to_string()))
}

/// Replace every occurrence of `secret` in `msg`.
///
/// `None` means this backend holds no credential that can appear in a
/// message — DuckDuckGo, and a SearXNG whose `base_url` carries no password.
/// It is deliberately not "no redaction needed by default": every caller of
/// [`send`] has to answer the question, and answering it wrong is visible at
/// the call site rather than buried in a helper nobody reads.
fn redact(msg: String, secret: Option<&str>) -> String {
    match secret {
        Some(s) if !s.is_empty() => msg.replace(s, "***REDACTED***"),
        _ => msg,
    }
}

/// Dispatch a prepared request and map every failure to a typed, redacted
/// `AlephError`.
///
/// This is the only entry point providers use. Callers set the URL, headers,
/// body and per-request timeout on `request`; everything from `.send()`
/// onwards — transport errors, status classification, credential redaction,
/// the body excerpt — happens here so that all nine backends fail the same
/// way and a tenth cannot forget one half of it.
pub(crate) async fn send(
    request: RequestBuilder,
    provider_name: &str,
    secret: Option<&str>,
) -> Result<Response> {
    let response = request
        .send()
        .await
        .map_err(|e| AlephError::network(redact(e.to_string(), secret)))?;
    check_status(response, provider_name, secret).await
}

/// Check HTTP response status and map to typed errors.
///
/// Returns the response unchanged on success. Maps 401/403 to
/// `AlephError::AuthenticationError`, 429 to `AlephError::RateLimitError`,
/// everything else to `AlephError::ProviderError` — the labels
/// `classify_search_error` turns into the `kind=` an operator greps for.
///
/// Private on purpose: reachable only through [`send`], which is what makes
/// "every provider redacts" a property of the module rather than of nine
/// separate memories.
async fn check_status(
    response: Response,
    provider_name: &str,
    secret: Option<&str>,
) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    // Read the body only on the failure path: on success it belongs to
    // `parse_json`, and consuming it here would leave nothing to parse.
    let detail = body_excerpt(response, secret).await;
    let msg = redact(
        format!("{provider_name} API error: {status}{detail}"),
        secret,
    );
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        Err(AlephError::authentication(provider_name, msg))
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        Err(AlephError::rate_limit(format!("{msg} (rate limited)")))
    } else {
        Err(AlephError::provider(msg))
    }
}

/// ` — <first ERROR_BODY_MAX_CHARS chars of the body>`, or an empty string.
///
/// A body that cannot be read is *not* reported as an empty body: the two
/// mean different things to whoever is diagnosing, and the status code is
/// already carrying the part we are sure of.
async fn body_excerpt(response: Response, secret: Option<&str>) -> String {
    let Ok(body) = response.text().await else {
        return String::new();
    };
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let excerpt = crate::utils::text_format::truncate_chars(body, ERROR_BODY_MAX_CHARS);
    format!(" — {}", redact(excerpt.to_string(), secret))
}

/// Parse JSON response body with provider-specific error context.
///
/// Takes the secret for the same reason [`send`] does: serde's error text
/// quotes the input it choked on, and for a backend that echoes its
/// credential back in an error envelope that input contains the key.
pub(crate) async fn parse_json<T: serde::de::DeserializeOwned>(
    response: Response,
    provider_name: &str,
    secret: Option<&str>,
) -> Result<T> {
    response.json::<T>().await.map_err(|e| {
        AlephError::provider(redact(
            format!("Failed to parse {provider_name} response: {e}"),
            secret,
        ))
    })
}

/// Drop results a caller cannot act on, and say how many were dropped.
///
/// The url is a result's identity: a title and a snippet with nothing to
/// open is not a smaller answer, it is not an answer. Everything else is
/// allowed to be missing — a url with no title still tells the caller where
/// to look.
///
/// This exists because the per-result structs used to make `title` a
/// required `String`, and serde does not degrade field by field: **one**
/// item a vendor returned without a title made the whole document fail to
/// deserialize, so the backend reported a parse failure and the chain moved
/// on as if it were down. The structs are now tolerant and this is the one
/// filter that decides what "usable" means, rather than six copies of it.
///
/// The drop is logged rather than returned: `SearchProvider::search` answers
/// with a `Vec`, and widening that to carry a count would ripple through
/// nine providers, the fallback and the registry to deliver a number whose
/// only reader is an operator — who has the log. If a backend ever turns out
/// to drop a material fraction, that is the moment to give it a carrier and
/// a note.
pub(crate) fn retain_usable(provider_name: &str, results: Vec<SearchResult>) -> Vec<SearchResult> {
    let before = results.len();
    let kept: Vec<SearchResult> = results
        .into_iter()
        .filter(|r| {
            let url = r.url.trim();
            if url.is_empty() {
                return false;
            }
            // Drop any non-http(s) scheme. `javascript:`, `data:`, `file:` and
            // `ftp:` would all survive `url::Url::parse` (which the merge layer
            // uses as identity), but should never reach the model or any
            // downstream fetcher. The DDG provider's `normalize_ddg_href`
            // already enforces the same rule for its uddg redirects; this is
            // the entry-point filter that catches the rest before they fan out.
            match url::Url::parse(url) {
                Ok(parsed) => matches!(parsed.scheme(), "http" | "https"),
                Err(_) => false,
            }
        })
        .collect();
    let dropped = before - kept.len();
    if dropped > 0 {
        log::warn!(
            target: "search",
            "provider={provider_name} kind=unusable dropped={dropped} of={before} \
             reason=result-has-no-or-non-http-url"
        );
    }
    kept
}

/// Reject an operator-configured upstream host that would turn the search
/// registry into an SSRF vector. Called from each provider's `new()` with the
/// URL the operator pasted into config. The check is intentionally narrower
/// than the runtime SSRF guard: a provider's `base_url` is set once at boot
/// (not per-request), so the cost of false positives is high — we only block
/// the unambiguous cases (loopback / private IPs / link-local / cloud
/// metadata / `localhost`-family hostnames) and let hostnames through. A
/// hostname that later resolves to a blocked address is caught by the
/// outbound guard at request time.
pub(crate) fn reject_ssrf_target_host(
    provider_name: &str,
    host: &str,
    allow_private_upstream: bool,
) -> Result<()> {
    use std::net::IpAddr;
    if allow_private_upstream {
        // The operator said so, per backend, in their own config. Logged at
        // INFO because "this instance deliberately talks to a private
        // address" is exactly what an operator reading a log after an
        // incident needs to be able to find.
        log::info!(
            target: "search",
            "provider={provider_name} host={host} allow_private_upstream=true — \
             boot-time SSRF host check waived by operator config"
        );
        return Ok(());
    }
    if let Ok(addr) = host.parse::<IpAddr>() {
        if crate::security::ssrf::ip::is_blocked_ip(addr) {
            return Err(AlephError::invalid_config(format!(
                "{provider_name} base URL points at a blocked IP ({host}); \
                 refusing to register an SSRF-targeting upstream"
            )));
        }
    } else {
        if crate::security::ssrf::hostname::is_blocked_hostname(host) {
            return Err(AlephError::invalid_config(format!(
                "{provider_name} base URL hostname {host:?} is on the SSRF \
                 blocklist; refusing to register an SSRF-targeting upstream"
            )));
        }
        // Legacy IP-literal encodings (hex/octal/decimal/short-form IPv4)
        // bypass naive `IpAddr::parse` and would otherwise slip through this
        // check to reach reqwest's resolver, which still parses them.
        if crate::security::ssrf::hostname::is_legacy_ip_literal(host) {
            return Err(AlephError::invalid_config(format!(
                "{provider_name} base URL hostname {host:?} is a legacy IP \
                 literal encoding; refusing to register an SSRF-targeting \
                 upstream"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both directions of the gate, because a gate whose negative half has
    /// no exit is fail-dead, not fail-closed (判据 §14).
    ///
    /// The closed direction is the reason the check exists. The open one is
    /// the reason it needs an opener: SearXNG's own documented deployment is
    /// `docker run -p 8080:8080`, so `http://127.0.0.1:8080` is the common
    /// case. With no opener the backend was dropped at boot behind one WARN
    /// line, `from_config` answered "no provider was constructable", and the
    /// operator read a working refusal as a missing backend (判据 §8).
    #[test]
    fn the_private_upstream_gate_opens_and_closes() {
        for host in ["127.0.0.1", "10.0.0.5", "localhost", "169.254.169.254", "0177.0.0.1"] {
            assert!(
                reject_ssrf_target_host("T", host, false).is_err(),
                "{host} must be refused by default"
            );
            assert!(
                reject_ssrf_target_host("T", host, true).is_ok(),
                "{host} must be reachable once the operator opts in"
            );
        }
        // The opener is not a bypass of anything else: a public host was
        // never refused, so opting in changes nothing for it.
        for host in ["searx.be", "api.firecrawl.dev"] {
            assert!(reject_ssrf_target_host("T", host, false).is_ok());
            assert!(reject_ssrf_target_host("T", host, true).is_ok());
        }
    }

    #[test]
    fn redaction_replaces_every_occurrence_and_leaves_empty_secrets_alone() {
        assert_eq!(
            redact("key=abc123 retry with abc123".to_string(), Some("abc123")),
            "key=***REDACTED*** retry with ***REDACTED***"
        );
        assert_eq!(redact("nothing".to_string(), None), "nothing");
        // An empty string is not a secret; replacing it would splice the
        // marker between every character of the message.
        assert_eq!(redact("nothing".to_string(), Some("")), "nothing");
    }

    #[test]
    fn a_result_without_a_url_is_not_a_smaller_answer() {
        let results = vec![
            SearchResult::new("has url", "https://example.com", "s"),
            SearchResult::new("no url", "", "s"),
            SearchResult::new("blank url", "   ", "s"),
        ];
        let kept = retain_usable("test", results);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].url, "https://example.com");
    }

    /// A title is allowed to be missing — the url is the answer.
    #[test]
    fn a_result_without_a_title_is_kept() {
        let kept = retain_usable("test", vec![SearchResult::new("", "https://x.test", "")]);
        assert_eq!(kept.len(), 1);
    }
}
