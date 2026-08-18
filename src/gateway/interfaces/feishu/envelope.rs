//! Reading a Lark response — both of the channels it answers on.
//!
//! Every Open Platform endpoint replies with the same envelope: `code`, `msg`,
//! and an optional `data`. What is easy to miss is that the *transport* carries
//! a second, partly-redundant copy of the same verdict, and neither channel is
//! complete on its own:
//!
//! * a throttle arrives as **HTTP 429** from the modern gateway but as
//!   **HTTP 400** from a documented set of legacy OpenAPI endpoints — and
//!   *both* carry `code: 99991400` in the body;
//! * a permission or gateway failure can arrive with **no JSON at all**, at
//!   which point the only thing anyone can act on is the status line.
//!
//! Before this module the client read exactly one channel per question and
//! picked the wrong one each time. [`FeishuSendError::RateLimited`] — the sole
//! error variant the outbound stack retries, and the reason
//! `ChannelRegistry`'s `SendRetryPolicy` exists at all — was minted from
//! `status == 429` alone, so a legacy-endpoint throttle became a generic
//! `Other`, which maps to `ChannelError::SendFailed`, which
//! `channel_registry::send` treats as terminal and
//! `delivery_queue::should_enqueue` refuses to re-enqueue. The reply was
//! dropped, and nothing above the channel was told why. Symmetrically, the
//! non-send endpoints read only the body: a 403 whose payload is the gateway's
//! HTML surfaced as `"Token response parse failed: error decoding response
//! body"`, which names neither the status nor the endpoint.
//!
//! So both questions are answered here, once, from the whole response.
//!
//! # The delay is Lark's to state
//!
//! Lark returns the wait in **`x-ogw-ratelimit-reset`** (documented as "恢复
//! limit 周期，单位：秒" — seconds to wait, not an absolute time). The client
//! read `retry-after`, which Lark does not send, so every honoured throttle
//! slept the hard-coded fallback instead of the number the server had just
//! supplied. `retry-after` is still read, second: it costs nothing and it is
//! what the rest of this tree's channels send. The constant is last and it is a
//! floor for "the server declined to say", not a guess at the real window.

use serde::de::DeserializeOwned;

/// Lark's frequency-limit code, on whichever channel it arrives.
///
/// Documented under "通用错误码 / Common error codes" as `request trigger
/// frequency limit`. This is the one value that is present in *both* the
/// modern (429) and legacy (400) shapes, which is why the predicate below is
/// an `||` over two channels rather than a status test.
pub(super) const RATE_LIMIT_CODE: i32 = 99_991_400;

/// Lark's own name for "seconds to wait before retrying".
const RATE_LIMIT_RESET_HEADER: &str = "x-ogw-ratelimit-reset";

/// What to wait when the server declined to say.
///
/// Not a guess at Lark's real window — the registry clamps the honoured value
/// anyway (`SendRetryPolicy::max_retry_after`). It exists so that a throttle
/// with no headers still backs off instead of hot-looping.
const DEFAULT_RETRY_AFTER_SECS: u64 = 5;

/// How many bytes of an unparseable body to quote back to the operator.
const BODY_EXCERPT_BYTES: usize = 200;

/// Seconds Lark is asking us to wait, or `None` if this is not a throttle.
///
/// The predicate is deliberately an `||` across the two channels: `429` is
/// unambiguous wherever it appears, and [`RATE_LIMIT_CODE`] is unambiguous
/// whatever status carried it. A bare `400` is *not* enough — that is the
/// generic bad-request status and treating it as a throttle would retry
/// malformed calls forever.
pub(super) fn throttle_retry_after(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body_code: Option<i32>,
) -> Option<u64> {
    let throttled =
        status == reqwest::StatusCode::TOO_MANY_REQUESTS || body_code == Some(RATE_LIMIT_CODE);
    throttled.then(|| header_secs(headers).unwrap_or(DEFAULT_RETRY_AFTER_SECS))
}

/// The delay Lark stated, from whichever header it stated it in.
fn header_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    [RATE_LIMIT_RESET_HEADER, "retry-after"]
        .into_iter()
        .find_map(|name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
        })
}

/// A response body, kept whole so both channels can be read from it.
///
/// The body has to be taken as bytes rather than by `resp.json()` because the
/// `code` inside it is half of the throttle predicate: deciding before parsing
/// is what made the legacy shape invisible, and parsing without keeping the
/// status is what made a 403 unreportable.
pub(super) struct Envelope {
    pub(super) status: reqwest::StatusCode,
    pub(super) headers: reqwest::header::HeaderMap,
    pub(super) body: Vec<u8>,
}

impl Envelope {
    /// Drain a response into memory. Fails only if the transport did.
    pub(super) async fn read(resp: reqwest::Response, what: &str) -> Result<Self, String> {
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("{what} response body failed: {e}"))?
            .to_vec();
        Ok(Self {
            status,
            headers,
            body,
        })
    }

    /// Parse the envelope, or say what actually came back.
    ///
    /// The failure text names the status and quotes the head of the body. The
    /// version this replaced said only `error decoding response body`, which is
    /// the same sentence for an expired credential, a 502 from a proxy and a
    /// genuinely malformed payload.
    pub(super) fn parse<T: DeserializeOwned>(&self, what: &str) -> Result<T, String> {
        serde_json::from_slice(&self.body).map_err(|e| {
            format!(
                "{what} parse failed: HTTP {} — {e}; body: {}",
                self.status.as_u16(),
                self.excerpt(),
            )
        })
    }

    /// Head of the body, on one line, for a diagnostic.
    pub(super) fn excerpt(&self) -> String {
        let text = String::from_utf8_lossy(&self.body);
        let cut = text
            .char_indices()
            .nth(BODY_EXCERPT_BYTES)
            .map_or(text.len(), |(i, _)| i);
        let head: String = text[..cut].split_whitespace().collect::<Vec<_>>().join(" ");
        if cut < text.len() {
            format!("{head}…")
        } else {
            head
        }
    }
}

/// The `code` / `msg` pair every Open Platform response carries.
///
/// Deliberately a trait on the response type rather than a closure at the call
/// site: the verdict is a property of the envelope, and a new response struct
/// that forgets to say where its code lives simply cannot use [`read_checked`].
/// The alternative — every caller passing `|r| (r.code, r.msg.clone())` — puts
/// the same fact in seven places and lets the eighth one differ.
pub(super) trait LarkEnvelope {
    fn code(&self) -> i32;
    fn message(&self) -> String;
}

/// Read a Lark envelope end to end: transport, parse, then `code`.
///
/// The single funnel for the endpoints that have no throttle handling of their
/// own. They previously hand-rolled three steps each and disagreed about all
/// three; converging them is what makes "a 403 says 403" true everywhere
/// rather than at the one call site somebody happened to be looking at.
pub(super) async fn read_checked<T: DeserializeOwned + LarkEnvelope>(
    resp: reqwest::Response,
    what: &str,
) -> Result<T, String> {
    let env = Envelope::read(resp, what).await?;
    let parsed: T = env.parse(what)?;
    if parsed.code() != 0 {
        return Err(format!(
            "{what} error: code={}, msg={} (HTTP {})",
            parsed.code(),
            parsed.message(),
            env.status.as_u16(),
        ));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};
    use reqwest::StatusCode;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    /// The modern shape: 429 plus the code.
    #[test]
    fn a_429_is_a_throttle() {
        assert_eq!(
            throttle_retry_after(
                StatusCode::TOO_MANY_REQUESTS,
                &headers(&[("x-ogw-ratelimit-reset", "52")]),
                Some(RATE_LIMIT_CODE),
            ),
            Some(52),
        );
    }

    /// The legacy shape, and the whole reason this predicate reads two
    /// channels: a documented set of Open Platform endpoints answers a throttle
    /// with **400**. Reading the status alone turns this into
    /// `ChannelError::SendFailed`, which the registry never retries and the
    /// delivery queue never re-enqueues — a silently dropped reply.
    #[test]
    fn a_400_carrying_the_rate_limit_code_is_still_a_throttle() {
        assert_eq!(
            throttle_retry_after(
                StatusCode::BAD_REQUEST,
                &headers(&[("x-ogw-ratelimit-reset", "7")]),
                Some(RATE_LIMIT_CODE),
            ),
            Some(7),
        );
    }

    /// A bare 400 is the generic bad-request status. Treating it as a throttle
    /// would retry a malformed call until the budget ran out and report a
    /// rate limit that never happened.
    #[test]
    fn a_400_without_the_code_is_not_a_throttle() {
        assert_eq!(
            throttle_retry_after(StatusCode::BAD_REQUEST, &HeaderMap::new(), Some(230_020)),
            None,
        );
        assert_eq!(
            throttle_retry_after(StatusCode::BAD_REQUEST, &HeaderMap::new(), None),
            None,
        );
    }

    /// A 429 whose body did not parse is still a throttle — the status alone
    /// carries that. This is the half the old code got right and it must
    /// survive the widening.
    #[test]
    fn a_429_with_no_parseable_body_is_still_a_throttle() {
        assert_eq!(
            throttle_retry_after(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), None),
            Some(DEFAULT_RETRY_AFTER_SECS),
        );
    }

    /// Lark's header wins over `retry-after`, and the constant is only reached
    /// when the server said nothing.
    ///
    /// Falsified by deleting the `x-ogw-ratelimit-reset` arm: the first case
    /// then answers 9 — the number Lark did *not* send — and every honoured
    /// throttle sleeps something the server never asked for.
    #[test]
    fn the_delay_comes_from_larks_own_header_first() {
        assert_eq!(
            throttle_retry_after(
                StatusCode::TOO_MANY_REQUESTS,
                &headers(&[("x-ogw-ratelimit-reset", "31"), ("retry-after", "9")]),
                None,
            ),
            Some(31),
        );
        assert_eq!(
            throttle_retry_after(
                StatusCode::TOO_MANY_REQUESTS,
                &headers(&[("retry-after", "9")]),
                None,
            ),
            Some(9),
        );
    }

    /// A non-numeric header does not become a zero-second back-off.
    #[test]
    fn an_unparseable_delay_falls_through_to_the_floor() {
        assert_eq!(
            throttle_retry_after(
                StatusCode::TOO_MANY_REQUESTS,
                &headers(&[("x-ogw-ratelimit-reset", "soon")]),
                None,
            ),
            Some(DEFAULT_RETRY_AFTER_SECS),
        );
    }

    /// A 200 with a healthy body is not a throttle.
    #[test]
    fn a_success_is_not_a_throttle() {
        assert_eq!(
            throttle_retry_after(StatusCode::OK, &HeaderMap::new(), Some(0)),
            None,
        );
    }

    /// The diagnostic names the status and quotes the body.
    #[derive(Debug, serde::Deserialize)]
    struct Probe {
        #[allow(dead_code)]
        code: i32,
    }

    #[test]
    fn a_parse_failure_reports_the_status_and_the_body() {
        let env = Envelope {
            status: StatusCode::FORBIDDEN,
            headers: HeaderMap::new(),
            body: b"<html>\n  <body>403 Forbidden</body>\n</html>".to_vec(),
        };
        let err = env.parse::<Probe>("Bot info").unwrap_err();
        assert!(err.contains("HTTP 403"), "no status in {err:?}");
        assert!(err.contains("403 Forbidden"), "no body excerpt in {err:?}");
        assert!(
            !err.contains('\n'),
            "the excerpt kept its newlines and will wrap a log line: {err:?}",
        );
    }

    /// A long body is cut, and the cut is on a character boundary.
    #[test]
    fn a_long_body_excerpt_is_cut_without_splitting_a_char() {
        let env = Envelope {
            status: StatusCode::BAD_GATEWAY,
            headers: HeaderMap::new(),
            body: "网关错误".repeat(200).into_bytes(),
        };
        let excerpt = env.excerpt();
        assert!(excerpt.ends_with('…'), "not marked as cut: {excerpt:?}");
        assert!(excerpt.chars().count() <= BODY_EXCERPT_BYTES + 1);
    }
}
