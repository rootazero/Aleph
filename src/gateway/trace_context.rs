//! W3C Trace Context propagation for the JSON-RPC dispatch path.
//!
//! This is a deliberately small, dependency-free implementation of the
//! [W3C `traceparent`](https://www.w3.org/TR/trace-context/) format — *not*
//! an OpenTelemetry integration. The full `OTel` SDK (opentelemetry +
//! opentelemetry-otlp + tonic/gRPC) is one of the heaviest dependency trees in
//! the ecosystem and would violate redline R3 (core minimalism) for what is,
//! for Aleph, a correlation feature: the daemon already persists its own
//! execution traces ([`crate::resilience::database::traces`]) and logs through
//! `tracing`.
//!
//! What this gives us instead, at near-zero cost:
//! - **Inbound honouring.** A caller (or upstream proxy) that sends a
//!   `traceparent` in the request params has its trace id adopted, so the
//!   server's spans/logs join the caller's trace.
//! - **Root minting.** Absent an inbound trace, a fresh 128-bit trace id is
//!   minted so every request is still correlatable.
//! - **Span correlation.** The dispatch chokepoint opens a `tracing` span
//!   carrying `trace_id` / `span_id`, so all logs emitted while handling the
//!   request share the trace id.
//! - **Echo / downstream propagation.** The response carries a `traceparent`
//!   naming *our* span as the parent, so the caller can continue the trace and
//!   a multi-hop call graph stitches together.
//!
//! The `traceparent` wire format is:
//! `version "-" trace-id "-" parent-id "-" trace-flags`, e.g.
//! `00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01`.

use serde_json::Value;

/// Hex length of a W3C trace id (16 bytes).
const TRACE_ID_HEX: usize = 32;
/// Hex length of a W3C span/parent id (8 bytes).
const SPAN_ID_HEX: usize = 16;

/// A resolved trace context for a single request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    /// 32-hex-char (128-bit) trace id, shared across the whole call graph.
    pub trace_id: String,
    /// 16-hex-char (64-bit) id of *this* span (the gateway's handling of the
    /// request). Emitted as the `parent-id` in the echoed `traceparent` so a
    /// downstream hop nests under us.
    pub span_id: String,
    /// The inbound span id we descend from, if the request carried a
    /// `traceparent`. `None` for a freshly-minted root trace.
    pub parent_id: Option<String>,
    /// W3C trace-flags byte (bit 0 = sampled).
    pub flags: u8,
}

impl TraceContext {
    /// Resolve the trace context for a request from its JSON-RPC `params`.
    ///
    /// Honours a `params.traceparent` string when present and well-formed
    /// (adopting its trace id and recording its span as our parent), otherwise
    /// mints a fresh sampled root trace.
    pub fn from_request_params(params: Option<&Value>) -> Self {
        let inbound = params
            .and_then(|p| p.get("traceparent"))
            .and_then(|v| v.as_str())
            .and_then(parse_traceparent);

        match inbound {
            Some((trace_id, parent_span, flags)) => Self {
                trace_id,
                span_id: new_span_id(),
                parent_id: Some(parent_span),
                flags,
            },
            None => Self {
                trace_id: new_trace_id(),
                span_id: new_span_id(),
                parent_id: None,
                flags: 0x01, // sampled
            },
        }
    }

    /// Render a `traceparent` header naming *this* span as the parent id, for
    /// echoing back to the caller / propagating to a downstream hop.
    #[must_use]
    pub fn to_header(&self) -> String {
        format!("00-{}-{}-{:02x}", self.trace_id, self.span_id, self.flags)
    }
}

/// Parse a W3C `traceparent` into `(trace_id, parent_id, flags)`.
///
/// Returns `None` for any structurally invalid header: wrong field count,
/// unsupported version `ff`, mis-sized or non-hex ids, or the all-zero
/// (invalid) trace/parent ids the spec forbids.
fn parse_traceparent(header: &str) -> Option<(String, String, u8)> {
    let mut parts = header.trim().split('-');
    let version = parts.next()?;
    let trace_id = parts.next()?;
    let parent_id = parts.next()?;
    let flags = parts.next()?;
    if parts.next().is_some() {
        return None; // too many fields
    }

    // Version: two hex chars, `ff` is reserved/invalid.
    if version.len() != 2 || !is_hex(version) || version == "ff" {
        return None;
    }
    if trace_id.len() != TRACE_ID_HEX || !is_hex(trace_id) || is_all_zero(trace_id) {
        return None;
    }
    if parent_id.len() != SPAN_ID_HEX || !is_hex(parent_id) || is_all_zero(parent_id) {
        return None;
    }
    let flags_byte = u8::from_str_radix(flags, 16)
        .ok()
        .filter(|_| flags.len() == 2)?;

    Some((
        trace_id.to_ascii_lowercase(),
        parent_id.to_ascii_lowercase(),
        flags_byte,
    ))
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_all_zero(s: &str) -> bool {
    s.bytes().all(|b| b == b'0')
}

/// Mint a random 128-bit trace id (32 lowercase hex chars).
fn new_trace_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Mint a random 64-bit span id (16 lowercase hex chars).
fn new_span_id() -> String {
    // The simple form is 32 hex chars; the first 16 are a uniformly random
    // 64-bit value, which is exactly a span id.
    uuid::Uuid::new_v4().simple().to_string()[..SPAN_ID_HEX].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_valid_traceparent() {
        let (tid, pid, flags) =
            parse_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap();
        assert_eq!(tid, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(pid, "00f067aa0ba902b7");
        assert_eq!(flags, 1);
    }

    #[test]
    fn rejects_malformed_headers() {
        assert!(parse_traceparent("garbage").is_none());
        assert!(parse_traceparent("00-tooshort-00f067aa0ba902b7-01").is_none());
        assert!(
            parse_traceparent("ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").is_none()
        ); // version ff
        assert!(
            parse_traceparent("00-00000000000000000000000000000000-00f067aa0ba902b7-01").is_none()
        ); // zero trace id
        assert!(
            parse_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01").is_none()
        ); // zero span id
        assert!(
            parse_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra")
                .is_none()
        );
        assert!(
            parse_traceparent("00-XYZ2f3577b34da6a3ce929d0e0e4736x-00f067aa0ba902b7-01").is_none()
        ); // non-hex
    }

    #[test]
    fn inbound_traceparent_is_adopted_with_fresh_span() {
        let params =
            json!({ "traceparent": "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01" });
        let tc = TraceContext::from_request_params(Some(&params));
        assert_eq!(tc.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(tc.parent_id.as_deref(), Some("00f067aa0ba902b7"));
        assert_eq!(tc.flags, 1);
        // Our own span id is fresh (not the inbound parent).
        assert_ne!(tc.span_id, "00f067aa0ba902b7");
        assert_eq!(tc.span_id.len(), SPAN_ID_HEX);
    }

    #[test]
    fn missing_traceparent_mints_root() {
        let tc = TraceContext::from_request_params(None);
        assert_eq!(tc.trace_id.len(), TRACE_ID_HEX);
        assert_eq!(tc.span_id.len(), SPAN_ID_HEX);
        assert!(tc.parent_id.is_none());
        assert_eq!(tc.flags, 0x01);
        // A minted root must round-trip through its own header form.
        assert!(parse_traceparent(&tc.to_header()).is_some());
    }

    #[test]
    fn to_header_names_our_span_as_parent() {
        let tc = TraceContext {
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
            span_id: "00f067aa0ba902b7".into(),
            parent_id: None,
            flags: 1,
        };
        assert_eq!(
            tc.to_header(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
    }

    #[test]
    fn malformed_inbound_falls_back_to_root() {
        let params = json!({ "traceparent": "not-valid" });
        let tc = TraceContext::from_request_params(Some(&params));
        assert!(tc.parent_id.is_none());
        assert_eq!(tc.trace_id.len(), TRACE_ID_HEX);
    }
}
