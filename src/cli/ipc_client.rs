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

/// Hard ceiling on the number of response bytes the CLI buffers from a
/// single admin response.
///
/// The admin API answers with small JSON documents (a secret summary, a
/// resume receipt). `reqwest`'s `text()` / `json()` read the whole body
/// into memory before the CLI ever sees it, so a misbehaving server — or
/// an intercepting proxy on the path — can dictate how much the CLI
/// allocates. 1 MiB is orders of magnitude above any legitimate admin
/// response and turns an unbounded read into a bounded one.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

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
            "no usable .ipc-endpoint.json at {} (missing, empty, or unsupported version). \
             The server is initializing or crashed — try again or run `aleph-server stop` first.",
            data_dir.display()
        )
    })?;
    let base = url::Url::parse(&endpoint.url).context("endpoint URL is not a parseable URL")?;
    // The admin channel is local-only: the endpoint file is written by the
    // daemon on this host, and the bearer token is read from this host's
    // `security.db` — a remote deployment has no local token to send. A
    // non-loopback host in the endpoint file therefore means the file was
    // tampered with (or points at another machine), and a plain-HTTP forward
    // would put the admin bearer token and the secret payload on the wire
    // in cleartext. Refuse BEFORE the token is attached.
    let host = base
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("endpoint URL {base} carries no host"))?;
    if !is_loopback_host(host) {
        anyhow::bail!(
            "refusing to send the admin bearer token to the non-loopback endpoint host \
             {host} (from {base}); the admin IPC channel is local-only and a plain-HTTP \
             forward would carry the token in cleartext. Bind the daemon to 127.0.0.1 or \
             ::1 and update .ipc-endpoint.json."
        );
    }
    // `url::Url::join` resolves per RFC 3986: an absolute-path reference such
    // as `/v1/admin/secrets` REPLACES the base path. A prefixed endpoint URL
    // would therefore lose its prefix and silently address the wrong server.
    // The daemon publishes a bare `scheme://host:port`, so a prefix is refused
    // rather than dropped.
    if !matches!(base.path(), "" | "/") {
        anyhow::bail!(
            "refusing to use endpoint URL {base}: it carries the path prefix {:?}, which \
             the absolute admin route would replace. The daemon must publish a bare \
             scheme://host:port endpoint.",
            base.path()
        );
    }
    // The route is absolute (`/v1/admin/...`), so the join below replaces the
    // (empty or "/") base path — with the prefix case refused above there is
    // no prefix left to lose.
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
        let text = read_body_bytes(resp)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
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

/// Read at most `MAX_RESPONSE_BYTES` from the response body.
///
/// `reqwest::blocking::Response::text()` / `json()` buffer the whole body
/// before the caller sees it, so the peer picks the CLI's memory use. This
/// streams the body through a capped sink instead: the transfer is aborted
/// with an error as soon as the cap is passed, so an oversized body is never
/// fully buffered.
fn read_body_bytes(mut resp: reqwest::blocking::Response) -> anyhow::Result<Vec<u8>> {
    let mut capped = CappedBuffer::new(MAX_RESPONSE_BYTES);
    // Bind the outcome before inspecting the sink: the `&mut capped` borrow
    // ends with this statement, so the code below may read and move `capped`.
    let read = resp.copy_to(&mut capped);
    if let Err(e) = read {
        if capped.overflowed() {
            return Err(anyhow::anyhow!(
                "server response body exceeds the {MAX_RESPONSE_BYTES}-byte limit; \
                 refusing to buffer it"
            ));
        }
        return Err(anyhow::Error::new(e).context("failed to read the server response body"));
    }
    Ok(capped.into_inner())
}

/// A `std::io::Write` sink that buffers at most `limit` bytes and fails the
/// write as soon as more arrive, so the transfer stops instead of growing
/// the buffer.
struct CappedBuffer {
    buf: Vec<u8>,
    limit: usize,
    overflowed: bool,
}

impl CappedBuffer {
    fn new(limit: usize) -> Self {
        Self {
            buf: Vec::new(),
            limit,
            overflowed: false,
        }
    }

    fn overflowed(&self) -> bool {
        self.overflowed
    }

    fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

impl std::io::Write for CappedBuffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let room = self.limit.saturating_sub(self.buf.len());
        if data.len() > room {
            // Keep what fits (bounded by `limit`) and stop the transfer.
            self.buf.extend_from_slice(&data[..room]);
            self.overflowed = true;
            return Err(std::io::Error::other(
                "response body exceeds the configured limit",
            ));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn truncate_error_body(s: String) -> String {
    if s.len() <= MAX_ERROR_BODY_CHARS {
        s
    } else {
        // Floor to a char boundary to keep the message valid UTF-8 even
        // when the cut would land inside a multi-byte sequence. `str` on
        // the project's MSRV ships `floor_char_boundary` as stable, so the
        // hand-rolled `while !is_char_boundary` walk is no longer needed.
        let cut = s.floor_char_boundary(MAX_ERROR_BODY_CHARS);
        let mut truncated = s[..cut].to_string();
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
/// Plain `http://` is allowed by this function on any host: the bearer token
/// is the only authentication, and the trust model for plain HTTP is the
/// caller's problem (an operator who chose plain HTTP has accepted the
/// consequences). The channel as a whole is local-only — `forward_to_server`
/// refuses a non-loopback endpoint for *every* scheme before the token is
/// attached — but this function keeps its own narrower rule so the two
/// concerns stay separable.
fn build_client(url: &str) -> anyhow::Result<reqwest::blocking::Client> {
    let mut builder =
        reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(10));
    let is_https = url.starts_with("https://");
    let host = host_of(url)
        .ok_or_else(|| anyhow::anyhow!("could not parse host out of endpoint URL {url:?}"))?;
    if is_https && !is_loopback_host(&host) {
        anyhow::bail!(
            "refusing to connect to admin IPC endpoint over HTTPS on \
             non-loopback host ({host} from {url}); a self-signed or otherwise \
             untrusted cert on a non-loopback host cannot be distinguished from \
             a MITM. Bind the daemon to 127.0.0.1 or ::1 and update \
             .ipc-endpoint.json."
        );
    }
    if is_https {
        // Self-signed cert on loopback is the supported local TLS setup.
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder
        .build()
        .context("failed to build HTTP client for admin IPC")
}

/// Extract the host portion of a `scheme://host:port/path` URL, correctly
/// handling bracketed IPv6 hosts (`[::1]:18790` → `::1`). Returns `None` for
/// unparseable input so the caller can produce a clear error.
///
/// `url::Url::parse` is the single parser: it understands the bracketed
/// IPv6 form natively, so `host_str()` already returns `::1` for
/// `https://[::1]:18790/admin`. The earlier manual fallback (a hand-rolled
/// char-by-char truncation with its own bracket-stripping) was removed: it
/// was unreachable in practice (every real input passes `Url::parse`) and
/// had a bracket-vs-`]`-suffix bug that would have produced
/// `"::1]"`-with-trailing-bracket for any unparseable IPv6 URL, which
/// `is_loopback_host` then could not parse (see
/// review-results/cli-logic-2026-08-26/REPORT.md Warning 2 for the prior
/// byte-split incident this replaced).
fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
}

fn is_loopback_host(host: &str) -> bool {
    // `url::Url::host_str` returns an IPv6 literal in its BRACKETED form
    // ("[::1]"), while `IpAddr::parse` wants the bare address. Strip the
    // brackets first, otherwise an IPv6 loopback endpoint ("http://[::1]:port",
    // which `build_endpoint_url` publishes for an IPv6 bind) is read as a
    // non-loopback host and refused.
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    // `IpAddr::is_loopback` covers `127.0.0.0/8` and `::1` (and the rest of
    // the loopback range on platforms that recognise it), which subsumes
    // both literal aliases — no need to spell them out.
    host == "localhost"
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
    let body = read_body_bytes(resp);
    if status.is_success() {
        let bytes = body?;
        Ok(
            serde_json::from_slice(&bytes)
                .context("failed to decode the server's JSON response")?,
        )
    } else {
        let text = match body {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) => format!("(failed to read response body: {e})"),
        };
        anyhow::bail!("server returned {status}: {}", truncate_error_body(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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

    /// The capped sink must accept exactly the limit and refuse one byte
    /// more — that boundary is the whole point of the bound.
    #[test]
    fn capped_buffer_accepts_exactly_the_limit() {
        let mut capped = CappedBuffer::new(8);
        capped
            .write_all(b"12345678")
            .expect("write within the limit");
        assert!(!capped.overflowed());
        assert_eq!(capped.into_inner(), b"12345678".to_vec());
    }

    #[test]
    fn capped_buffer_refuses_more_than_the_limit() {
        let mut capped = CappedBuffer::new(8);
        let err = capped
            .write_all(b"123456789")
            .expect_err("one byte past the limit must fail");
        assert!(err.to_string().contains("exceeds"), "{err}");
        assert!(capped.overflowed());
        // The buffer itself stays bounded by the limit.
        assert_eq!(capped.into_inner().len(), 8);
    }

    /// The admin bearer token must never leave the box in cleartext: a
    /// non-loopback endpoint host is refused before the token is read and
    /// attached. The address below is TEST-NET-1 (RFC 5737) and never
    /// resolves, but the refusal happens before any network call at all.
    #[test]
    fn forward_to_server_refuses_a_non_loopback_endpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::cli::endpoint::write_endpoint(
            dir.path(),
            &crate::cli::endpoint::IpcEndpoint::current("http://192.0.2.1:18790"),
        )
        .expect("write endpoint");
        let err = forward_to_server::<serde_json::Value>(
            dir.path(),
            HttpMethod::Get,
            "/v1/admin/secrets",
            serde_json::Value::Null,
        )
        .expect_err("a non-loopback endpoint must be refused");
        let msg = format!("{err:#}");
        assert!(msg.contains("non-loopback"), "{msg}");
        assert!(msg.contains("refusing"), "{msg}");
    }

    /// `Url::join` drops a base path prefix, so a prefixed endpoint URL
    /// would address the wrong server. Fail closed instead.
    #[test]
    fn forward_to_server_refuses_an_endpoint_with_a_path_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::cli::endpoint::write_endpoint(
            dir.path(),
            &crate::cli::endpoint::IpcEndpoint::current("http://127.0.0.1:18790/prefix"),
        )
        .expect("write endpoint");
        let err = forward_to_server::<serde_json::Value>(
            dir.path(),
            HttpMethod::Get,
            "/v1/admin/secrets",
            serde_json::Value::Null,
        )
        .expect_err("a prefixed endpoint must be refused");
        let msg = format!("{err:#}");
        assert!(msg.contains("path prefix"), "{msg}");
        assert!(msg.contains("refusing"), "{msg}");
    }

    /// `read_endpoint` reports a missing file, an empty file and an
    /// unsupported version the same way (`Ok(None)`); the caller's message
    /// must therefore cover all three instead of claiming the file is
    /// absent.
    #[test]
    fn forward_to_server_reports_an_unusable_endpoint_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = forward_to_server::<serde_json::Value>(
            dir.path(),
            HttpMethod::Get,
            "/v1/admin/secrets",
            serde_json::Value::Null,
        )
        .expect_err("no endpoint file must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("no usable .ipc-endpoint.json"), "{msg}");
        assert!(
            msg.contains("missing, empty, or unsupported version"),
            "{msg}"
        );
        // Pinned by tests/spec_c_cli_endpoint_missing.rs.
        assert!(msg.contains("server is initializing"), "{msg}");
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
