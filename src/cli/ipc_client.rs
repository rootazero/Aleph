//! HTTP client that forwards a CLI request to the running server's
//! `/v1/admin/*` namespace. Reads bearer token from `security.db` in
//! read-only WAL mode; auto-retries once on 401 to handle token rotation
//! that races with the request.

use std::path::Path;

use anyhow::Context;
use reqwest::StatusCode;

use crate::cli::endpoint::read_endpoint;
use crate::cli::policy::HttpMethod;
use crate::gateway::security::read_current_token_readonly;
use crate::utils::sqlite_open::open_sqlite_readonly;

const SECURITY_DB_FILENAME: &str = "security.db";

pub fn forward_to_server<T>(
    data_dir: &Path,
    method: HttpMethod,
    route: &str,
    body: serde_json::Value,
) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let endpoint = read_endpoint(data_dir)?.with_context(|| {
        format!(
            "server is initializing or crashed (no .ipc-endpoint.json at {}). \
             Try again or run `aleph stop` first.",
            data_dir.display()
        )
    })?;
    // Robust URL join via `url::Url::join` rather than the prior
    // `format!("{}/{...}")` slice: today the daemon writes a bare
    // `scheme://host:port`, but a future reverse-proxy prefix or query
    // string would be silently appended to the wrong segment.
    let base =
        url::Url::parse(&endpoint.url).context("endpoint URL is not a parseable URL")?;
    let joined = base
        .join(route)
        .with_context(|| format!("cannot join route {route:?} to base {base}"))?;
    // Strip any query string the base may carry; the admin routes do not
    // honour one, and a query from a tampered endpoint file would otherwise
    // be forwarded verbatim.
    let mut url = joined;
    url.set_query(None);
    let url = url.to_string();

    let token = read_token(data_dir)?;
    let resp = call_once(&url, method, &body, &token)?;

    if resp.status() == StatusCode::UNAUTHORIZED {
        // Token may have rotated between our read and our send.
        let fresh = read_token(data_dir)?;
        if fresh != token {
            let resp2 = call_once(&url, method, &body, &fresh)?;
            return finalize::<T>(resp2);
        }
        let text = resp
            .text()
            .unwrap_or_else(|e| format!("(failed to read response body: {e})"));
        anyhow::bail!(
            "authentication failed — bearer token rejected by server: {}",
            truncate_error_body(text)
        );
    }
    finalize::<T>(resp)
}

/// Cap the size of server-supplied text we splice into an error message.
/// A misbehaving server that returns a stack trace or large payload in
/// its error body should not leak those bytes verbatim into CLI output
/// (operator privacy, log noise) — 256 chars is enough to identify the
/// error class and matches the size budget used elsewhere in the admin
/// layer.
const MAX_ERROR_BODY_CHARS: usize = 256;

fn truncate_error_body(s: String) -> String {
    if s.len() <= MAX_ERROR_BODY_CHARS {
        s
    } else {
        // Truncate on a char boundary to keep the message valid UTF-8 even
        // when the cut lands inside a multi-byte sequence.
        let mut end = MAX_ERROR_BODY_CHARS;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        let mut truncated = s[..end].to_string();
        truncated.push_str("…[truncated]");
        truncated
    }
}

fn read_token(data_dir: &Path) -> anyhow::Result<String> {
    let conn = open_sqlite_readonly(&data_dir.join(SECURITY_DB_FILENAME))
        .context("cannot open security.db read-only — is data_dir set up?")?;
    let token = read_current_token_readonly(&conn)?.ok_or_else(|| {
        anyhow::anyhow!("no bearer token in security.db — has the server ever been started?")
    })?;
    Ok(token)
}

fn call_once(
    url: &str,
    method: HttpMethod,
    body: &serde_json::Value,
    token: &str,
) -> anyhow::Result<reqwest::blocking::Response> {
    // Built per call rather than cached: the CLI is a one-shot process that
    // sends at most two requests (initial + one 401 retry), so connection
    // reuse is irrelevant, and propagating a build failure beats panicking
    // on a user-facing path.
    let client = build_client(url)?;
    let req = client
        .request(method.as_reqwest(), url)
        .bearer_auth(token)
        .json(body);
    Ok(req.send()?)
}

/// Build a blocking HTTP client for the admin IPC channel.
///
/// The IPC channel carries a bearer token that grants `/v1/admin/*`, so the
/// TLS posture matters. We accept self-signed certs (the supported local
/// deployment uses one) but **only** when the endpoint is on loopback — any
/// non-loopback HTTPS host is a clear signal of operator error or a MITM
/// attempt and is refused outright rather than silently trusted.
///
/// Plain `http://` is allowed on any host: the bearer token is the only
/// authentication, but the trust model for plain HTTP is the caller's
/// problem (an operator who chose plain HTTP has accepted the consequences).
fn build_client(url: &str) -> anyhow::Result<reqwest::blocking::Client> {
    let mut builder =
        reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(10));
    let host = host_of(url).ok_or_else(|| {
        anyhow::anyhow!("could not parse host out of endpoint URL {url:?}")
    })?;
    if url.starts_with("https://") && !is_loopback_host(&host) {
        anyhow::bail!(
            "refusing to connect to admin IPC endpoint over HTTPS on \
             non-loopback host ({host} from {url}); a self-signed or otherwise \
             untrusted cert on a non-loopback host cannot be distinguished from \
             a MITM. Bind the daemon to 127.0.0.1 or ::1 and update \
             .ipc-endpoint.json."
        );
    }
    if url.starts_with("https://") {
        // Self-signed cert on loopback is the supported local TLS setup.
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder
        .build()
        .context("failed to build HTTP client for admin IPC")
}

/// Extract the host portion of a `scheme://host:port/path` URL, correctly
/// handling bracketed IPv6 hosts (`[::1]:18790` → `::1`). Falls back to
/// `None` for unparseable input so the caller can produce a clear error.
fn host_of(url: &str) -> Option<String> {
    // Strip the scheme.
    let after_scheme = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url);
    // Strip userinfo (none expected today, but stay defensive).
    let after_userinfo = after_scheme.split_once('@').map(|(_, rest)| rest).unwrap_or(after_scheme);
    // Truncate at the first `:`, `/`, or `?` that closes the host portion.
    // For IPv6 these come in the form `[::1]:port` so use `url::Url::parse`
    // to do the bracket-aware split rather than the prior byte-split which
    // turned `[::1]:18790` into host `::1` only when an extra `:18790`
    // boundary survived — fragile and silently rejected loopback HTTPS in
    // production (see review-results/cli-logic-2026-08-26/REPORT.md Warning 2).
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .or_else(|| {
            // Manual fallback: strip everything from the host-closing delimiter.
            let mut end = after_userinfo.len();
            for (idx, ch) in after_userinfo.char_indices() {
                if matches!(ch, '/' | '?') {
                    end = idx;
                    break;
                }
            }
            let host = &after_userinfo[..end];
            // Strip the bracketed IPv6 form to bare `::1` etc.
            let host = host
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .unwrap_or(host);
            // Stop at the trailing `]:port` — none expected here since the
            // loop above already removed `:`, but defensive.
            let host = host.split(':').next().unwrap_or("");
            if host.is_empty() { None } else { Some(host.to_string()) }
        })
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

fn finalize<T>(resp: reqwest::blocking::Response) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let status = resp.status();
    if status.is_success() {
        let body = resp.json::<T>()?;
        Ok(body)
    } else {
        let text = resp
            .text()
            .unwrap_or_else(|e| format!("(failed to read response body: {e})"));
        anyhow::bail!("server returned {status}: {}", truncate_error_body(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration tests for forward_to_server live in tests/spec_c_cli_ipc.rs
    // because they need a real HTTP listener and a seeded security.db. Unit
    // tests here cover only the helpers that don't need a network.

    #[test]
    fn is_loopback_host_recognises_loopback_aliases() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        // Full-loopback IPv4 ranges:
        assert!(is_loopback_host("127.0.0.53"));
        assert!(is_loopback_host("127.255.255.254"));
        // IPv6 loopback ranges too (the std `IpAddr::is_loopback` flag
        // covers ::1 only on most platforms; the rest of ::/128 is not
        // loopback per the standard, so we don't claim it here).
    }

    #[test]
    fn is_loopback_host_rejects_external_addresses() {
        assert!(!is_loopback_host("10.0.0.1"));
        assert!(!is_loopback_host("192.168.1.1"));
        assert!(!is_loopback_host("8.8.8.8"));
        assert!(!is_loopback_host("2001:db8::1"));
        assert!(!is_loopback_host("example.com"));
    }

    #[test]
    fn build_client_refuses_https_to_non_loopback() {
        // The exact contract H1 in review-results/cli.md promises:
        // non-loopback HTTPS must be refused, not silently trusted.
        let err = build_client("https://10.0.0.1:9000/admin")
            .expect_err("non-loopback https should fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("refusing to connect"),
            "expected refusal message, got: {msg}"
        );
    }

    #[test]
    fn build_client_accepts_loopback_https() {
        // Loopback HTTPS: the supported local-TLS setup. We can't observe
        // the `danger_accept_invalid_certs` flag from a public handle, but
        // the absence of an error here is the observable contract: the
        // client builds, so loopback is allowed.
        let client =
            build_client("https://127.0.0.1:9000/admin").expect("loopback https should be allowed");
        drop(client);
    }

    #[test]
    fn build_client_accepts_plain_http_for_every_host() {
        // HTTP never needs the loopback gate — the trust model for
        // plain HTTP is the caller's problem (the bearer token is the
        // only authentication).
        let client = build_client("http://10.0.0.1:9000/admin")
            .expect("plain http should be allowed on any host");
        drop(client);
    }

    #[test]
    fn truncate_error_body_passes_through_short_strings() {
        assert_eq!(truncate_error_body("ok".into()), "ok");
        let short = "x".repeat(MAX_ERROR_BODY_CHARS);
        assert_eq!(truncate_error_body(short.clone()), short);
    }

    #[test]
    fn truncate_error_body_caps_long_strings() {
        let long = "y".repeat(MAX_ERROR_BODY_CHARS + 100);
        let out = truncate_error_body(long);
        assert!(out.ends_with("…[truncated]"));
        // The leading prefix is exactly the cap; the trailing marker is
        // the suffix.
        let prefix_len = MAX_ERROR_BODY_CHARS;
        assert!(out.is_char_boundary(prefix_len));
        assert!(out.starts_with(&"y".repeat(prefix_len)));
    }

    #[test]
    fn truncate_error_body_respects_char_boundaries() {
        // Build a string where the byte-boundary cut would land inside a
        // multi-byte char. The helper must walk back to a valid char
        // boundary so the result is still valid UTF-8.
        let mut s = "a".repeat(MAX_ERROR_BODY_CHARS - 1);
        s.push('ß'); // 2 bytes
        s.push_str(&"c".repeat(50));
        let out = truncate_error_body(s);
        assert!(out.ends_with("…[truncated]"));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }
}
