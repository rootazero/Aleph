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
//! and `error_funnel_census.rs` asserts at the source level that no provider
//! dispatches a request or maps a status code on its own.

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
/// Returns the response unchanged on success. The status → variant mapping
/// is the one place the chain's error kinds (`SearchErrorKind`) are decided:
/// 401/403 → `AlephError::AuthenticationError` (auth), 429 →
/// `AlephError::RateLimitError` (quota), any other 4xx →
/// `AlephError::RequestRejected` (the backend refused the request as
/// shaped), 5xx and anything else → `AlephError::ProviderError`
/// (transient). Each variant names a different lever — fix the key, wait
/// out the window, reshape the request, retry later — which is the entire
/// reason to tell them apart.
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
    } else if status.is_client_error() {
        // The backend understood the request and refused it: resending the
        // identical bytes fails identically, so this is not the transient
        // bucket. The body excerpt above usually carries the vendor's
        // complaint about the offending parameter.
        Err(AlephError::request_rejected(msg))
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
///
/// A failure here is `AlephError::InvalidResponse`, not `ProviderError`:
/// the backend answered 200 but broke its contract, which is a different
/// fact from "the backend errored" and earns a different lever (the backend
/// changed, or served a challenge page — retrying the identical request
/// changes nothing).
pub(crate) async fn parse_json<T: serde::de::DeserializeOwned>(
    response: Response,
    provider_name: &str,
    secret: Option<&str>,
) -> Result<T> {
    response.json::<T>().await.map_err(|e| {
        AlephError::invalid_response(redact(
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

/// The verdict of the construction-time SSRF check on an operator-configured
/// upstream host.
///
/// Three states, not two, because "allowed" splits into "unremarkable" and
/// "allowed only because the operator opted into private networks" — the
/// second must be logged, or the switch silently turns a refusal into a
/// registration and the only record of why lives in a TOML file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostVerdict {
    /// The host is not on any blocklist — registered without comment.
    Allow,
    /// Blocked by the default policy, allowed only under `[ssrf]
    /// allow_private_network = true`. The caller must log at WARN.
    AllowUnderPrivateNetworkOptIn,
    /// Refused. Carries the operator-facing reason.
    Reject(&'static str),
}

/// Classify an operator-configured upstream host against the SSRF blocklist,
/// honouring the operator's `[ssrf] allow_private_network` switch.
///
/// Two floors hold under EVERY policy, including an explicit opt-in:
///
/// * **Cloud metadata endpoints** (`169.254.169.254`, `fd00:ec2::254`, their
///   IPv6 transition forms, and the `metadata.google.internal` /
///   `metadata.internal` hostnames). A metadata service answers ANY path with
///   instance credentials, and `search_config.update` is a model-reachable
///   RPC — if an injected model can point a provider's `base_url` there, every
///   subsequent search query (session context included) is exfiltrated. The
///   operator's switch is about reaching LAN services, not about waiving this.
/// * **Legacy IP-literal encodings** (hex/octal/decimal/short-form IPv4).
///   They are never a legitimate way to write a base URL, and classifying
///   their decoded target cheaply is exactly where historical SSRF bypasses
///   lived — refusing the encoding wholesale costs no real deployment.
///
/// Everything else the default policy blocks (loopback, RFC1918, link-local,
/// `localhost`-family hostnames, `.local`/`.internal` suffixes) becomes
/// [`HostVerdict::AllowUnderPrivateNetworkOptIn`] when the switch is on: a
/// self-hosted SearXNG on the LAN — the most common SearXNG deployment — is
/// precisely what the switch exists for.
pub(crate) fn classify_ssrf_target_host(host: &str, allow_private_network: bool) -> HostVerdict {
    use std::net::IpAddr;
    if let Ok(addr) = host.parse::<IpAddr>() {
        if crate::security::ssrf::ip::is_cloud_metadata(addr) {
            return HostVerdict::Reject("points at a cloud instance-metadata endpoint");
        }
        if crate::security::ssrf::ip::is_blocked_ip(addr) {
            return if allow_private_network {
                HostVerdict::AllowUnderPrivateNetworkOptIn
            } else {
                HostVerdict::Reject("points at a blocked IP")
            };
        }
        return HostVerdict::Allow;
    }
    if crate::security::ssrf::hostname::is_cloud_metadata_hostname(host) {
        return HostVerdict::Reject("names a cloud instance-metadata endpoint");
    }
    // Checked before the blocklist so a legacy literal never benefits from the
    // opt-in: we cannot see its decoded target, so it cannot earn the pass.
    if crate::security::ssrf::hostname::is_legacy_ip_literal(host) {
        return HostVerdict::Reject("is a legacy IP literal encoding");
    }
    if crate::security::ssrf::hostname::is_blocked_hostname(host) {
        return if allow_private_network {
            HostVerdict::AllowUnderPrivateNetworkOptIn
        } else {
            HostVerdict::Reject("is on the SSRF blocklist")
        };
    }
    HostVerdict::Allow
}

/// Reject an operator-configured upstream host that would turn the search
/// registry into an SSRF vector. Called from each provider's `new()` with the
/// URL the operator pasted into config. The check is intentionally narrower
/// than the runtime SSRF guard: a provider's `base_url` is set once at boot
/// (not per-request), so the cost of false positives is high — we only block
/// the unambiguous cases and let hostnames through. A hostname that later
/// resolves to a blocked address is caught by the outbound guard at request
/// time.
///
/// `allow_private_network` is the operator's explicit `[ssrf]` switch
/// ([`crate::security::ssrf::SsrfPolicy::allow_private_network`]): when false
/// (the default) every blocked host is refused; when true, loopback / private
/// / link-local targets are accepted **with a WARN log**, while cloud
/// metadata endpoints and legacy IP literals stay refused under every policy
/// — see [`classify_ssrf_target_host`] for why those two classes never
/// benefit from the opt-in.
pub(crate) fn reject_ssrf_target_host(
    provider_name: &str,
    host: &str,
    allow_private_network: bool,
) -> Result<()> {
    match classify_ssrf_target_host(host, allow_private_network) {
        HostVerdict::Allow => Ok(()),
        HostVerdict::AllowUnderPrivateNetworkOptIn => {
            log::warn!(
                target: "search",
                "provider={provider_name} kind=ssrf-opt-in host={host} \
                 accepted under [ssrf] allow_private_network=true; this base URL \
                 reaches a private/loopback target that the default policy refuses"
            );
            Ok(())
        }
        HostVerdict::Reject(reason) => Err(AlephError::invalid_config(format!(
            "{provider_name} base URL host {host:?} {reason}; refusing to register an \
             SSRF-targeting upstream. A self-hosted instance on a private network requires \
             the operator to set [ssrf] allow_private_network = true (cloud metadata \
             endpoints and legacy IP literals stay blocked regardless)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::error::SearchErrorKind;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The default policy refuses every blocked class; the operator's
    /// `[ssrf] allow_private_network` switch re-admits loopback / private /
    /// `localhost`-family targets as warn-worthy, and cloud metadata plus
    /// legacy IP literals stay refused under BOTH policies. The verdict (not
    /// the log line) is what the constructor acts on, so these quadrants pin
    /// the whole behaviour of the switch.
    #[test]
    fn construction_check_quadrants_of_the_private_network_switch() {
        use super::HostVerdict;
        // Default policy: refuse the blocked classes.
        for host in ["127.0.0.1", "10.0.0.5", "192.168.1.8", "localhost", "box.local"] {
            assert!(
                matches!(
                    super::classify_ssrf_target_host(host, false),
                    HostVerdict::Reject(_)
                ),
                "{host} must be refused by the default policy"
            );
        }
        // Opt-in: the same hosts become warn-worthy allows.
        for host in ["127.0.0.1", "10.0.0.5", "192.168.1.8", "localhost", "box.local"] {
            assert_eq!(
                super::classify_ssrf_target_host(host, true),
                HostVerdict::AllowUnderPrivateNetworkOptIn,
                "{host} must be allowed-with-warn under the operator switch"
            );
        }
        // The floors that never move: cloud metadata (both address families,
        // both spellings of the name) and legacy IP literal encodings.
        for host in [
            "169.254.169.254",
            "fd00:ec2::254",
            "metadata.google.internal",
            "metadata.internal",
            "0x7f000001",
            "2130706433",
            "127.1",
        ] {
            for allow_private in [false, true] {
                assert!(
                    matches!(
                        super::classify_ssrf_target_host(host, allow_private),
                        HostVerdict::Reject(_)
                    ),
                    "{host} must stay refused even with allow_private_network={allow_private}"
                );
            }
        }
        // Public targets pass quietly under both policies.
        for host in ["93.184.216.34", "searx.example.com"] {
            for allow_private in [false, true] {
                assert_eq!(
                    super::classify_ssrf_target_host(host, allow_private),
                    HostVerdict::Allow,
                    "{host} is public and must pass without a warn"
                );
            }
        }
    }

    /// The refusal text must name the lever: an operator whose LAN SearXNG is
    /// refused reads which switch to flip, and is told the floors that no
    /// switch lifts. Vague refusals get worked around; specific ones get
    /// configured.
    #[test]
    fn the_refusal_names_the_switch_and_the_floors() {
        let err = super::reject_ssrf_target_host("SearXNG", "127.0.0.1", false)
            .expect_err("loopback is refused by default");
        let text = err.to_string();
        assert!(text.contains("allow_private_network"), "{text}");
        assert!(text.contains("metadata"), "{text}");
    }

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
        for host in [
            "127.0.0.1",
            "10.0.0.5",
            "localhost",
            "169.254.169.254",
            "0177.0.0.1",
        ] {
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

    /// Run [`send`] against a mock server answering with `status`, and return
    /// the error it produced. The funnel is the one place a status becomes an
    /// `AlephError` variant, so the variant — and through it the
    /// [`SearchErrorKind`] — is what these tests pin.
    async fn send_against(status: u16) -> AlephError {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(status).set_body_string("{\"error\":\"vendor detail\"}"),
            )
            .mount(&server)
            .await;
        let client = build_client().expect("client");
        send(client.get(server.uri()), "mock", None)
            .await
            .expect_err("a non-2xx status must come back as an error")
    }

    /// The status → variant mapping is the chain's error-kind contract: each
    /// status class names a different lever (fix the key, wait out the quota
    /// window, reshape the request, retry later), so a regression here is a
    /// wrong instruction to every reader of the failure report.
    #[tokio::test]
    async fn status_codes_map_to_the_kind_whose_lever_fits() {
        let cases: &[(u16, SearchErrorKind)] = &[
            (401, SearchErrorKind::Auth),
            (403, SearchErrorKind::Auth),
            (429, SearchErrorKind::Quota),
            (400, SearchErrorKind::RequestRejected),
            (404, SearchErrorKind::RequestRejected),
            (422, SearchErrorKind::RequestRejected),
            (500, SearchErrorKind::Transient),
            (503, SearchErrorKind::Transient),
        ];
        for (status, expected) in cases {
            let err = send_against(*status).await;
            assert_eq!(
                SearchErrorKind::of(&err),
                *expected,
                "status {status} produced the wrong kind: {err}",
            );
        }
    }

    /// The vendor's complaint about the request survives into the error —
    /// the excerpt is what makes a 400 actionable — and a credential in the
    /// echoed body does not.
    #[tokio::test]
    async fn the_error_keeps_the_vendor_detail_and_redacts_the_secret() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string("bad parameter days=99, key was sk-secret-123"),
            )
            .mount(&server)
            .await;
        let client = build_client().expect("client");
        let err = send(client.get(server.uri()), "mock", Some("sk-secret-123"))
            .await
            .expect_err("400 is an error");
        let text = err.to_string();
        assert!(text.contains("bad parameter days=99"), "{text}");
        assert!(!text.contains("sk-secret-123"), "{text}");
        assert!(text.contains("***REDACTED***"), "{text}");
    }

    /// A 200 with a body that does not parse is `InvalidResponse`, not a
    /// generic provider error: the backend answered but broke its contract,
    /// and retrying the identical request changes nothing.
    #[tokio::test]
    async fn an_unparseable_success_body_is_an_invalid_response() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>challenge</html>"))
            .mount(&server)
            .await;
        let client = build_client().expect("client");
        let response = send(client.get(server.uri()), "mock", None)
            .await
            .expect("200 passes the status check");
        let err = parse_json::<serde_json::Value>(response, "mock", None)
            .await
            .expect_err("HTML is not JSON");
        assert_eq!(SearchErrorKind::of(&err), SearchErrorKind::InvalidResponse);
        assert!(
            matches!(err, AlephError::InvalidResponse { .. }),
            "the variant carries the kind: {err}",
        );
    }
}
