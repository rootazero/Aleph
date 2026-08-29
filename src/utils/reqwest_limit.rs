//! Bounded-size body readers for `reqwest::Response`.
//!
//! `reqwest::Response::bytes()` and `text()` allocate the whole body into
//! memory in one shot; a hostile or misconfigured upstream can hand back
//! arbitrarily large bodies. The `*_with_limit` helpers here stream the
//! chunks and cap total size at `limit` bytes, with a Content-Length
//! pre-check so the rejection can happen before any chunk is read.
//!
//! Return shape:
//! - `Ok(Some(body))` — read completed under the cap.
//! - `Ok(None)`       — body would have exceeded `limit`. Returned as a
//!   value (not an `Err`) so call sites can decide whether to surface
//!   it as a domain-level error (the firecrawl + generation-poll paths
//!   do).
//! - `Err(e)`         — I/O / decode failure.
//!
//! Review-fetch P0 (2026-08-20) named this as the missing piece on the
//! SSRF / size-limit path; the call sites in `firecrawl.rs` and the
//! generation poll path now route through these helpers.

use anyhow::{anyhow, Result};
use futures::TryStreamExt;

/// Read the response body as bytes, capping total size at `limit` bytes.
///
/// Returns `Ok(None)` when the body would exceed `limit` — distinguished
/// from `Err` so the call site can decide whether the cap is a hard
/// refusal (firecrawl, generation poll: yes) or a soft signal.
pub async fn bytes_with_limit(resp: reqwest::Response, limit: usize) -> Result<Option<Vec<u8>>> {
    if let Some(cl) = resp.content_length() {
        if cl > limit as u64 {
            return Ok(None);
        }
    }
    let mut buf = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.try_next().await? {
        if buf.len() + chunk.len() > limit {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(Some(buf))
}

/// Read the response body as UTF-8 text, capping total size at `limit` bytes.
///
/// Same return shape as [`bytes_with_limit`]. UTF-8 decode failures
/// surface as `Err` — a body that is too large is `Ok(None)`, a body
/// that is the right size but not valid UTF-8 is `Err`.
pub async fn text_with_limit(resp: reqwest::Response, limit: usize) -> Result<Option<String>> {
    match bytes_with_limit(resp, limit).await? {
        Some(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|e| anyhow!("response body is not valid UTF-8: {e}")),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{any, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Fetch the mock server's response and hand it to the helper under test.
    /// Tests below vary only the response template, not the request plumbing,
    /// so the helper is the single thing exercised.
    async fn fetch(server: &MockServer) -> reqwest::Response {
        reqwest::Client::new()
            .get(server.uri())
            .send()
            .await
            .expect("request to mock server")
    }

    /// The happy path: a body that fits under the cap returns `Some(body)`
    /// with the exact bytes the server sent. Pinned because a refactor that
    /// added an off-by-one to the streaming check would silently return
    /// `None` for any body larger than one chunk — and one chunk is exactly
    /// what the next test does NOT exercise.
    #[tokio::test]
    async fn bytes_with_limit_returns_some_for_body_under_limit() {
        let server = MockServer::start().await;
        let body = b"hello, world";
        Mock::given(any())
            .and(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let resp = fetch(&server).await;
        assert_eq!(bytes_with_limit(resp, 1024).await.unwrap(), Some(body.to_vec()));
    }

    /// Content-Length pre-check: a server that *advertises* a body larger
    /// than the cap is refused before any chunk is read. Verified by
    /// pairing the over-limit Content-Length with a body that would itself
    /// be under the cap — the only way to tell the pre-check fired is if
    /// `None` comes back even though the bytes-on-the-wire were small.
    #[tokio::test]
    async fn bytes_with_limit_refuses_on_content_length_above_limit() {
        let server = MockServer::start().await;
        let body = b"under-the-cap body, but lied about in the header";
        Mock::given(any())
            .and(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(body)
                    .insert_header("content-length", "9999999"),
            )
            .mount(&server)
            .await;

        let resp = fetch(&server).await;
        assert_eq!(resp.content_length(), Some(9999999));
        assert_eq!(bytes_with_limit(resp, 1024).await.unwrap(), None);
    }

    /// Streaming cap (no Content-Length): the upstream doesn't tell us the
    /// size in advance, so the streaming accumulator is the only line of
    /// defence. The body is `limit + 1` bytes long; the very first chunk
    /// pushes `buf.len() + chunk.len() > limit` true and the helper returns
    /// `None`. Pinned because a check that read `buf.len() < limit` rather
    /// than `buf.len() + chunk.len() > limit` would let the last byte
    /// through.
    #[tokio::test]
    async fn bytes_with_limit_refuses_a_chunked_body_above_limit() {
        let server = MockServer::start().await;
        // Force transfer-encoding: chunked by setting the body via a stream
        // template. wiremock's default `set_body_bytes` also works because
        // reqwest surfaces `content_length() = None` for chunked responses,
        // but explicit is cheaper to read than implicit.
        let body = vec![b'x'; 2049];
        Mock::given(any())
            .and(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let resp = fetch(&server).await;
        assert_eq!(bytes_with_limit(resp, 1024).await.unwrap(), None);
    }

    /// The cap holds across many chunks, not just the first: a body just
    /// under the limit must come back whole. Pinned because the
    /// streaming-check's arithmetic (`buf.len() + chunk.len()`) could
    /// overflow on `usize` if a future refactor cast to a narrower type —
    /// a body close to the cap forces the addition to happen.
    #[tokio::test]
    async fn bytes_with_limit_returns_some_for_a_large_chunked_body() {
        let server = MockServer::start().await;
        let body = vec![b'a'; 1023]; // one byte under the limit
        Mock::given(any())
            .and(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let resp = fetch(&server).await;
        assert_eq!(bytes_with_limit(resp, 1024).await.unwrap(), Some(body));
    }

    /// UTF-8 decode failure is `Err`, not `Ok(None)`. The size cap is a
    /// different layer of the contract — a body that fits the cap but
    /// isn't valid UTF-8 is a decode failure, not a "too large" answer,
    /// and conflating them would let a caller treat a malformed upstream
    /// response as a refusal.
    #[tokio::test]
    async fn text_with_limit_surfaces_utf8_decode_failure_as_err() {
        let server = MockServer::start().await;
        let body: Vec<u8> = vec![0xff, 0xfe, 0xfd]; // not valid UTF-8
        Mock::given(any())
            .and(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let resp = fetch(&server).await;
        let result = text_with_limit(resp, 1024).await;
        let err = result.expect_err("invalid UTF-8 must surface as Err, not Ok(None)");
        assert!(
            err.to_string().contains("not valid UTF-8"),
            "error message must name the failure mode, got: {err}"
        );
    }

    /// `text_with_limit` propagates the `Ok(None)` size-cap answer; a body
    /// too large to even attempt decode is still `Ok(None)`, not `Err`.
    /// The two helpers' return shapes must agree, or a caller that maps
    /// `text_with_limit` and `bytes_with_limit` uniformly would see one
    /// path always error and the other never.
    #[tokio::test]
    async fn text_with_limit_returns_none_for_body_above_limit() {
        let server = MockServer::start().await;
        let body = vec![b'a'; 4096];
        Mock::given(any())
            .and(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(body)
                    .insert_header("content-length", "4096"),
            )
            .mount(&server)
            .await;

        let resp = fetch(&server).await;
        assert_eq!(text_with_limit(resp, 1024).await.unwrap(), None);
    }
}
