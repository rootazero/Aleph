//! Safe HTTP fetch with SSRF protection, DNS pinning, and redirect chain validation.
//!
//! Provides `safe_fetch` which validates every URL (including redirect targets)
//! against the SSRF policy, pins DNS to prevent rebinding, and strips sensitive
//! headers on cross-origin redirects.

use std::collections::HashSet;
use std::time::Duration;

use reqwest::header::{HeaderMap, LOCATION};
use reqwest::{Method, StatusCode};
use url::Url;

use super::dns::resolve_and_validate;
use super::hostname::{
    has_url_credentials, is_allowlisted, is_blocked_hostname, is_blocklisted, is_legacy_ip_literal,
};
use super::policy::SsrfPolicy;
use super::{validate_scheme, SsrfError};

/// Request configuration for `safe_fetch`.
pub struct SafeFetchRequest {
    pub method: Method,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    /// Optional response-body byte cap. `None` reads the body unbounded (legacy
    /// behavior); `Some(n)` streams and aborts once `n` bytes are exceeded so a
    /// hostile/oversized upstream cannot OOM the process before a downstream
    /// size check runs.
    pub max_body_bytes: Option<usize>,
}

impl SafeFetchRequest {
    /// Creates a GET request with the given timeout.
    #[must_use]
    pub fn get(timeout: Duration) -> Self {
        Self {
            method: Method::GET,
            headers: HeaderMap::new(),
            body: None,
            timeout,
            max_body_bytes: None,
        }
    }

    /// Creates a POST request with a body and timeout.
    #[must_use]
    pub fn post(body: Vec<u8>, timeout: Duration) -> Self {
        Self {
            method: Method::POST,
            headers: HeaderMap::new(),
            body: Some(body),
            timeout,
            max_body_bytes: None,
        }
    }

    /// Sets custom headers on the request.
    #[must_use]
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Caps how many response-body bytes are read. A declared `Content-Length`
    /// over the cap fails fast; otherwise the body is streamed and aborted once
    /// the cap is exceeded — bounding memory against a hostile/oversized
    /// upstream. `None` (default) preserves the unbounded read.
    #[must_use]
    pub fn with_max_body_bytes(mut self, max: usize) -> Self {
        self.max_body_bytes = Some(max);
        self
    }

    /// Sets the HTTP method.
    #[must_use]
    pub fn with_method(mut self, method: Method) -> Self {
        self.method = method;
        self
    }
}

/// Response from `safe_fetch`.
#[derive(Debug)]
pub struct SafeFetchResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

// --- Internal helpers ---

/// Returns true if two URLs have different origins (scheme + host + port).
fn is_cross_origin(a: &Url, b: &Url) -> bool {
    a.scheme() != b.scheme()
        || a.host_str() != b.host_str()
        || a.port_or_known_default() != b.port_or_known_default()
}

/// Performs all pre-flight SSRF validation on a URL: scheme, legacy IP, credentials,
/// hostname blocklist/allowlist, user blocklist, and DNS resolution with pinning.
///
/// Returns the parsed URL and a pinned `SocketAddr` for connection.
async fn validate_url_full(
    url_str: &str,
    policy: &SsrfPolicy,
) -> Result<(Url, std::net::SocketAddr), SsrfError> {
    let url = Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

    validate_scheme(&url)?;

    let host = url.host_str().ok_or(SsrfError::NoHost)?;

    // Legacy IP literal detection (hex, octal, decimal, short-form)
    if is_legacy_ip_literal(host) {
        return Err(SsrfError::BlockedAddress(format!(
            "legacy IP literal: {host}"
        )));
    }

    // URL credentials detection
    if has_url_credentials(url_str) {
        return Err(SsrfError::InvalidUrl(
            "URL contains embedded credentials".to_string(),
        ));
    }

    // Allowlist check — bypass the hostname blocklists, but still classify the
    // resolved IP. An allowlist entry trusts the *name*, not "any address that
    // name happens to resolve to": loopback and cloud metadata remain blocked so
    // a rebinding attacker controlling the allowlisted name cannot reach
    // 127.0.0.1 / 169.254.169.254. (Previously this used `SsrfPolicy::disabled()`,
    // which waived all IP checks and was a DNS-rebinding SSRF bypass.)
    if is_allowlisted(host, &policy.allowed_hosts) {
        let port = url.port_or_known_default().unwrap_or(80);
        let pinned = resolve_and_validate(host, port, &SsrfPolicy::for_allowlisted_host()).await?;
        return Ok((url, pinned));
    }

    // Hostname blocklist (hardcoded names)
    if is_blocked_hostname(host) {
        return Err(SsrfError::BlockedAddress(host.to_string()));
    }

    // User-configured blocklist
    if is_blocklisted(host, &policy.blocked_hosts) {
        return Err(SsrfError::BlockedAddress(format!(
            "host in blocklist: {host}"
        )));
    }

    // DNS resolution + IP validation
    let port = url.port_or_known_default().unwrap_or(80);
    let pinned = resolve_and_validate(host, port, policy).await?;

    Ok((url, pinned))
}

/// Authentication/credential request headers stripped on cross-origin redirects.
///
/// Covers the standard `Authorization`/`Cookie` pair plus the de-facto API-key
/// and session-token header names that real services use, so a redirect to a
/// different origin cannot harvest credentials meant for the original host.
/// Header names are matched case-insensitively by `HeaderMap`.
const CROSS_ORIGIN_STRIPPED_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "x-api-key",
    "api-key",
    "x-auth-token",
    "x-access-token",
    "x-amz-security-token",
    "x-goog-api-key",
    "x-functions-key",
    "x-csrf-token",
];

/// Strips sensitive authentication headers from a `HeaderMap`.
fn strip_auth_headers(headers: &mut HeaderMap) {
    for name in CROSS_ORIGIN_STRIPPED_HEADERS {
        headers.remove(*name);
    }
}

/// Fetches a URL with full SSRF protection.
///
/// When `policy.enabled` is false, performs a normal fetch with auto-redirects.
/// Otherwise validates the URL and every redirect hop against the policy,
/// pins DNS to prevent rebinding, and strips auth headers on cross-origin redirects.
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub async fn safe_fetch(
    url: &str,
    policy: &SsrfPolicy,
    request: SafeFetchRequest,
) -> Result<SafeFetchResponse, SsrfError> {
    // Bypass mode — normal fetch when SSRF protection is disabled
    if !policy.enabled {
        return bypass_fetch(url, request).await;
    }

    // Full validation of the initial URL
    let (mut current_url, pinned_addr) = validate_url_full(url, policy).await?;

    // Build client with redirect disabled and DNS pinning
    let host = current_url.host_str().ok_or(SsrfError::NoHost)?.to_string();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, pinned_addr)
        .timeout(request.timeout)
        .build()
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    // Send the initial request
    // rust-doctor-disable-next-line excessive-clone
    let mut req_builder = client.request(request.method.clone(), current_url.as_str());
    // rust-doctor-disable-next-line excessive-clone
    req_builder = req_builder.headers(request.headers.clone());
    if let Some(body) = &request.body {
        // rust-doctor-disable-next-line excessive-clone
        req_builder = req_builder.body(body.clone());
    }

    let mut response = req_builder
        .send()
        .await
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    // Redirect loop
    let mut visited: HashSet<Url> = HashSet::new();
    // rust-doctor-disable-next-line excessive-clone
    visited.insert(current_url.clone());
    let mut redirect_count: u16 = 0;
    let mut current_method = request.method;
    let mut current_headers = request.headers;

    while response.status().is_redirection() {
        redirect_count += 1;
        if redirect_count > u16::from(policy.max_redirects) {
            return Err(SsrfError::TooManyRedirects(policy.max_redirects));
        }

        // Extract Location header
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                SsrfError::FetchFailed("redirect without valid Location header".to_string())
            })?
            .to_string();

        // Resolve relative URLs against current
        let next_url = current_url
            .join(&location)
            .map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

        // Loop detection
        if visited.contains(&next_url) {
            return Err(SsrfError::FetchFailed("redirect loop detected".to_string()));
        }
        // rust-doctor-disable-next-line excessive-clone
        visited.insert(next_url.clone());

        // Validate the redirect target
        let (next_url, next_pinned) = validate_url_full(next_url.as_str(), policy).await?;

        // Cross-origin detection → strip auth headers
        if policy.strip_auth_on_cross_origin && is_cross_origin(&current_url, &next_url) {
            strip_auth_headers(&mut current_headers);
        }

        // POST → GET on 301/302/303; 307/308 preserve method and body per RFC
        let status = response.status();
        let preserve_method_and_body = matches!(
            status,
            StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT
        );
        if !preserve_method_and_body
            && matches!(
                status,
                StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER
            )
            && current_method == Method::POST
        {
            current_method = Method::GET;
        }

        // Build new client with pinned address for the redirect target
        let next_host = next_url.host_str().ok_or(SsrfError::NoHost)?.to_string();
        let redirect_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&next_host, next_pinned)
            .timeout(request.timeout)
            .build()
            .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

        // rust-doctor-disable-next-line excessive-clone
        let mut req_builder = redirect_client.request(current_method.clone(), next_url.as_str());
        // rust-doctor-disable-next-line excessive-clone
        req_builder = req_builder.headers(current_headers.clone());
        // Body is forwarded only on 307/308; POST→GET on 301/302/303 drops body
        if preserve_method_and_body {
            if let Some(body) = &request.body {
                // rust-doctor-disable-next-line excessive-clone
                req_builder = req_builder.body(body.clone());
            }
        }

        response = req_builder
            .send()
            .await
            .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

        current_url = next_url;
    }

    // Read response body
    let status = response.status();
    // rust-doctor-disable-next-line excessive-clone
    let headers = response.headers().clone();
    let body = read_body_capped(response, request.max_body_bytes).await?;

    Ok(SafeFetchResponse {
        status,
        headers,
        body,
    })
}

/// Normal fetch without SSRF validation (used when policy.enabled is false).
async fn bypass_fetch(
    url: &str,
    request: SafeFetchRequest,
) -> Result<SafeFetchResponse, SsrfError> {
    let max_body_bytes = request.max_body_bytes;
    let parsed = Url::parse(url).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;
    validate_scheme(&parsed)?;

    let client = reqwest::Client::builder()
        .timeout(request.timeout)
        .build()
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    let mut req_builder = client.request(request.method, url);
    req_builder = req_builder.headers(request.headers);
    if let Some(body) = request.body {
        req_builder = req_builder.body(body);
    }

    let response = req_builder
        .send()
        .await
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    let status = response.status();
    // rust-doctor-disable-next-line excessive-clone
    let headers = response.headers().clone();
    let body = read_body_capped(response, max_body_bytes).await?;

    Ok(SafeFetchResponse {
        status,
        headers,
        body,
    })
}

/// Read a response body, enforcing an optional byte cap. When `max` is `Some`,
/// a declared `Content-Length` over the cap fails fast and the body is streamed
/// chunk-by-chunk so a chunked / length-lying response cannot buffer past the
/// cap (prevents OOM from a hostile or oversized upstream). `None` preserves the
/// prior unbounded `.bytes()` behavior for callers that opt out.
async fn read_body_capped(
    mut response: reqwest::Response,
    max: Option<usize>,
) -> Result<Vec<u8>, SsrfError> {
    let Some(limit) = max else {
        return Ok(response
            .bytes()
            .await
            .map_err(|e| SsrfError::FetchFailed(e.to_string()))?
            .to_vec());
    };
    if let Some(len) = response.content_length() {
        if len > limit as u64 {
            return Err(SsrfError::FetchFailed(format!(
                "response body too large: {len} bytes (max {limit})"
            )));
        }
    }
    let mut buf = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?
    {
        if buf.len() + chunk.len() > limit {
            return Err(SsrfError::FetchFailed(format!(
                "response body exceeded {limit} bytes"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::super::validate_scheme;
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn validate_scheme_accepts_http() {
        let url = Url::parse("http://example.com").unwrap();
        assert!(validate_scheme(&url).is_ok());
    }

    #[test]
    fn validate_scheme_accepts_https() {
        let url = Url::parse("https://example.com").unwrap();
        assert!(validate_scheme(&url).is_ok());
    }

    #[test]
    fn validate_scheme_rejects_ftp() {
        let url = Url::parse("ftp://example.com").unwrap();
        assert!(validate_scheme(&url).is_err());
    }

    #[test]
    fn validate_scheme_rejects_file() {
        let url = Url::parse("file:///etc/passwd").unwrap();
        assert!(validate_scheme(&url).is_err());
    }

    #[test]
    fn validate_scheme_rejects_gopher() {
        let url = Url::parse("gopher://example.com").unwrap();
        assert!(validate_scheme(&url).is_err());
    }

    #[test]
    fn cross_origin_detects_different_host() {
        let a = Url::parse("https://a.com/path").unwrap();
        let b = Url::parse("https://b.com/path").unwrap();
        assert!(is_cross_origin(&a, &b));
    }

    #[test]
    fn cross_origin_detects_different_scheme() {
        let a = Url::parse("https://example.com").unwrap();
        let b = Url::parse("http://example.com").unwrap();
        assert!(is_cross_origin(&a, &b));
    }

    #[test]
    fn cross_origin_detects_different_port() {
        let a = Url::parse("https://example.com:443").unwrap();
        let b = Url::parse("https://example.com:8443").unwrap();
        assert!(is_cross_origin(&a, &b));
    }

    #[test]
    fn same_origin_returns_false() {
        let a = Url::parse("https://example.com/a").unwrap();
        let b = Url::parse("https://example.com/b").unwrap();
        assert!(!is_cross_origin(&a, &b));
    }

    #[test]
    fn same_origin_with_explicit_default_port() {
        let a = Url::parse("https://example.com").unwrap();
        let b = Url::parse("https://example.com:443").unwrap();
        assert!(
            !is_cross_origin(&a, &b),
            "explicit default port should be same origin"
        );

        let c = Url::parse("http://example.com").unwrap();
        let d = Url::parse("http://example.com:80").unwrap();
        assert!(
            !is_cross_origin(&c, &d),
            "explicit default port should be same origin"
        );
    }

    #[test]
    fn safe_fetch_request_get_defaults() {
        let req = SafeFetchRequest::get(Duration::from_secs(30));
        assert_eq!(req.method, Method::GET);
        assert!(req.headers.is_empty());
        assert!(req.body.is_none());
        assert_eq!(req.timeout, Duration::from_secs(30));
    }

    #[test]
    fn safe_fetch_request_post_has_body() {
        let body = b"hello".to_vec();
        let req = SafeFetchRequest::post(body.clone(), Duration::from_secs(10));
        assert_eq!(req.method, Method::POST);
        assert_eq!(req.body.unwrap(), body);
    }

    #[test]
    fn safe_fetch_request_builder_chain() {
        let mut headers = HeaderMap::new();
        headers.insert("x-custom", HeaderValue::from_static("val"));
        let req = SafeFetchRequest::get(Duration::from_secs(5))
            .with_headers(headers)
            .with_method(Method::PUT);
        assert_eq!(req.method, Method::PUT);
        assert!(req.headers.contains_key("x-custom"));
    }

    #[test]
    fn strip_auth_headers_removes_sensitive() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer token"));
        headers.insert("cookie", HeaderValue::from_static("session=abc"));
        headers.insert("proxy-authorization", HeaderValue::from_static("Basic xyz"));
        // API-key / session-token style headers that real services use.
        headers.insert("x-api-key", HeaderValue::from_static("k-123"));
        headers.insert("x-auth-token", HeaderValue::from_static("t-456"));
        headers.insert(
            "x-amz-security-token",
            HeaderValue::from_static("aws-session"),
        );
        headers.insert("content-type", HeaderValue::from_static("text/html"));

        strip_auth_headers(&mut headers);

        assert!(!headers.contains_key("authorization"));
        assert!(!headers.contains_key("cookie"));
        assert!(!headers.contains_key("proxy-authorization"));
        assert!(!headers.contains_key("x-api-key"));
        assert!(!headers.contains_key("x-auth-token"));
        assert!(!headers.contains_key("x-amz-security-token"));
        // Non-credential headers are preserved.
        assert!(headers.contains_key("content-type"));
    }

    #[test]
    fn bypass_fetch_rejects_file_scheme() {
        // Even with SSRF disabled, bypass_fetch should reject non-http(s) schemes
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            bypass_fetch(
                "file:///etc/passwd",
                SafeFetchRequest::get(Duration::from_secs(5)),
            )
            .await
        });
        assert!(
            matches!(result, Err(SsrfError::InvalidUrl(ref s)) if s.contains("unsupported scheme")),
            "bypass_fetch should reject file:// scheme, got {:?}",
            result
        );
    }

    #[test]
    fn bypass_fetch_rejects_gopher_scheme() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            bypass_fetch(
                "gopher://example.com",
                SafeFetchRequest::get(Duration::from_secs(5)),
            )
            .await
        });
        assert!(
            matches!(result, Err(SsrfError::InvalidUrl(ref s)) if s.contains("unsupported scheme")),
            "bypass_fetch should reject gopher:// scheme, got {:?}",
            result
        );
    }
}
