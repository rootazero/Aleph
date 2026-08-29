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
    if declared_over_limit(resp.content_length(), limit) {
        return Ok(None);
    }
    accumulate_capped(resp.bytes_stream(), limit).await
}

/// Does the upstream's *advertised* size already exceed the cap?
///
/// A named function over two scalars rather than an arm of
/// [`bytes_with_limit`], because that is the only shape a test can reach
/// honestly. The test that used to cover this branch stood up a mock server
/// advertising `content-length: 9999999` over a 47-byte body — and an HTTP
/// client that trusts Content-Length reads a short body as a *truncated
/// message*, so `reqwest::send()` failed with `IncompleteMessage` before the
/// helper was ever called. It failed that way on every run from the commit
/// that added it; what it measured was hyper's framing, never this line.
///
/// `>` and not `>=`: `limit` bytes is exactly the cap, not past it, and the
/// streaming half agrees (`buf.len() + chunk.len() > limit`). The two halves
/// disagreeing by one is the shape this boundary exists to pin.
fn declared_over_limit(content_length: Option<u64>, limit: usize) -> bool {
    content_length.is_some_and(|cl| cl > limit as u64)
}

/// Accumulate a body stream, refusing the chunk that would cross `limit`.
///
/// Split out for the same reason as [`declared_over_limit`], and with more at
/// stake: reaching this loop from a mock server requires a response with **no**
/// Content-Length, and `wiremock`'s `set_body_bytes` always sets one. Both
/// server-driven tests that named this loop in their docs — one of them
/// claiming to pin `buf.len() + chunk.len() > limit` against an off-by-one —
/// were answered by [`declared_over_limit`] and returned before a chunk was
/// ever read. Measured, not reasoned: probing `resp.content_length()` inside
/// the "chunked" test returns `Some(2049)`.
///
/// Generic over the stream so the test can supply the chunk boundaries
/// directly; that is what makes "the cap holds *across* chunks" an assertion
/// rather than a hope about how the network split the body.
async fn accumulate_capped<S, E>(stream: S, limit: usize) -> Result<Option<Vec<u8>>>
where
    S: futures::Stream<Item = std::result::Result<bytes::Bytes, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut buf = Vec::new();
    let mut stream = std::pin::pin!(stream);
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

    /// The advertised-size boundary, asked directly.
    ///
    /// `limit` bytes is the cap, not past it. Its predecessor tried to reach
    /// this branch through a mock server that lied in its `content-length`
    /// header, which no HTTP client will carry — see [`declared_over_limit`]
    /// for what that test actually measured.
    #[test]
    fn declared_over_limit_refuses_only_strictly_above_the_cap() {
        assert!(!declared_over_limit(None, 1024), "no header is not a refusal");
        assert!(!declared_over_limit(Some(0), 1024));
        assert!(
            !declared_over_limit(Some(1024), 1024),
            "a body of exactly `limit` bytes fits; refusing it here would \
             disagree with the streaming half, which admits it"
        );
        assert!(declared_over_limit(Some(1025), 1024));
    }

    /// The streaming cap, across chunk boundaries the test chooses.
    ///
    /// This is the assertion the two server-driven "chunked" tests were
    /// believed to be making. Three chunks of 500 bytes: the first two are
    /// admitted, the third would take the total to 1500 and is refused — so a
    /// check written `buf.len() < limit` (true at 1000) instead of
    /// `buf.len() + chunk.len() > limit` would let it through and return 1500
    /// bytes for a 1024-byte cap.
    #[tokio::test]
    async fn accumulate_capped_refuses_the_chunk_that_would_cross_the_cap() {
        let chunks: Vec<std::result::Result<bytes::Bytes, std::io::Error>> = (0..3)
            .map(|_| Ok(bytes::Bytes::from(vec![b'c'; 500])))
            .collect();
        let out = accumulate_capped(futures::stream::iter(chunks), 1024)
            .await
            .unwrap();
        assert_eq!(out, None, "1500 bytes must not pass a 1024-byte cap");
    }

    /// The mirror of the case above: chunks summing to exactly the cap are
    /// admitted whole. Pins the same `>` the boundary test pins on the
    /// declared side, so the two halves cannot drift apart by one byte.
    #[tokio::test]
    async fn accumulate_capped_admits_chunks_summing_to_exactly_the_cap() {
        let chunks: Vec<std::result::Result<bytes::Bytes, std::io::Error>> = (0..2)
            .map(|_| Ok(bytes::Bytes::from(vec![b'c'; 512])))
            .collect();
        let out = accumulate_capped(futures::stream::iter(chunks), 1024)
            .await
            .unwrap();
        assert_eq!(out, Some(vec![b'c'; 1024]));
    }

    /// A stream error is an `Err`, not a silent short read.
    ///
    /// The size cap answers with `Ok(None)`; a transport failure must not be
    /// folded into that answer, or a caller treating `None` as "upstream is
    /// too big" would report a network fault as a policy refusal.
    #[tokio::test]
    async fn accumulate_capped_surfaces_a_stream_error_as_err() {
        let chunks: Vec<std::result::Result<bytes::Bytes, std::io::Error>> = vec![
            Ok(bytes::Bytes::from_static(b"partial")),
            Err(std::io::Error::other("upstream reset")),
        ];
        let err = accumulate_capped(futures::stream::iter(chunks), 1024)
            .await
            .expect_err("a stream error must not be reported as a size refusal");
        assert!(err.to_string().contains("upstream reset"), "got: {err}");
    }

    /// End-to-end refusal of an over-cap body, through a real response.
    ///
    /// It does **not** exercise the streaming accumulator, whatever its name
    /// used to say: `wiremock`'s `set_body_bytes` sets a Content-Length, so
    /// this response arrives as `Some(2049)` (measured, by asserting it) and
    /// [`declared_over_limit`] answers before a chunk is read. The
    /// `buf.len() + chunk.len() > limit` arithmetic its old doc claimed to pin
    /// is pinned by `accumulate_capped_refuses_the_chunk_that_would_cross_the_cap`.
    ///
    /// What it is still worth keeping for is the seam: that a real
    /// `reqwest::Response` reaches the cap at all, and that the pre-check is
    /// wired into `bytes_with_limit` rather than merely existing.
    #[tokio::test]
    async fn bytes_with_limit_refuses_a_response_whose_declared_size_is_over_the_cap() {
        let server = MockServer::start().await;
        let body = vec![b'x'; 2049];
        Mock::given(any())
            .and(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let resp = fetch(&server).await;
        assert_eq!(
            resp.content_length(),
            Some(2049),
            "this response is not chunked; if wiremock ever stops setting \
             Content-Length, this test starts covering a different branch and \
             its doc above becomes false"
        );
        assert_eq!(bytes_with_limit(resp, 1024).await.unwrap(), None);
    }

    /// A body one byte under the cap comes back whole, through a real
    /// response. Also not the streaming path (see the test above): its value
    /// is that `bytes_with_limit` returns the bytes rather than the refusal
    /// when the declared size fits, which is the other side of the same seam.
    #[tokio::test]
    async fn bytes_with_limit_returns_a_response_just_under_the_cap_whole() {
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
