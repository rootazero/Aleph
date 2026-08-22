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
