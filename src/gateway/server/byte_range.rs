//! Single-range HTTP `Range` parsing, shared by every byte route.
//!
//! Both `/artifact` and `/canvas-asset` need this and neither may grow its own
//! copy. Without Range support, WebKitGTK — which plays media through
//! GStreamer — cannot seek: the scrub bar does nothing, audio does not buffer,
//! and large files can fail outright.
//!
//! # What is deliberately not here
//!
//! `multipart/byteranges`. Browser media elements and GStreamer issue single
//! ranges; multi-range buys nothing and costs a whole response encoding. A
//! multi-range request is answered with the WHOLE resource (RFC 9110 lets a
//! server ignore a `Range` it does not support), never with 416 — refusing a
//! request we can satisfy in full would be a regression, not a safety measure.
//!
//! # This is a representation concern
//!
//! Callers must apply it AFTER every authorization gate. A range must never be
//! the reason a byte is reachable.

/// What to do with a request's `Range` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeVerdict {
    /// No usable range — send 200 with the entire body. Covers "no header",
    /// "malformed", and "multi-range".
    Whole,
    /// Send 206 with `[start, end]`, both inclusive.
    Satisfiable { start: u64, end: u64 },
    /// Send 416 with `Content-Range: bytes */<total>`.
    Unsatisfiable,
}

impl RangeVerdict {
    /// The `Content-Range` header value for this verdict, or `None` when the
    /// response carries no `Content-Range` (i.e. [`Self::Whole`]).
    #[must_use]
    pub fn content_range(&self, total: u64) -> Option<String> {
        match self {
            Self::Whole => None,
            Self::Satisfiable { start, end } => Some(format!("bytes {start}-{end}/{total}")),
            Self::Unsatisfiable => Some(format!("bytes */{total}")),
        }
    }
}

/// Parse a single-range `Range` header against a known total length.
#[must_use]
pub fn parse_range(header: Option<&str>, total: u64) -> RangeVerdict {
    let Some(raw) = header else {
        return RangeVerdict::Whole;
    };
    let Some(spec) = raw.trim().strip_prefix("bytes=") else {
        // Any other unit ("items=0-5") is one we do not implement.
        return RangeVerdict::Whole;
    };
    let spec = spec.trim();
    if spec.contains(',') {
        // Multi-range: answer in full rather than refuse. See the module doc.
        return RangeVerdict::Whole;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return RangeVerdict::Whole;
    };
    let (first, last) = (first.trim(), last.trim());

    // A zero-length resource can satisfy no range at all.
    if total == 0 {
        return RangeVerdict::Unsatisfiable;
    }

    if first.is_empty() {
        // Suffix form: `bytes=-N` means the LAST N bytes.
        let Ok(n) = last.parse::<u64>() else {
            return RangeVerdict::Whole;
        };
        if n == 0 {
            return RangeVerdict::Unsatisfiable;
        }
        let start = total.saturating_sub(n);
        return RangeVerdict::Satisfiable {
            start,
            end: total - 1,
        };
    }

    let Ok(start) = first.parse::<u64>() else {
        return RangeVerdict::Whole;
    };
    if start >= total {
        return RangeVerdict::Unsatisfiable;
    }
    if last.is_empty() {
        // Open-ended: `bytes=N-`.
        return RangeVerdict::Satisfiable {
            start,
            end: total - 1,
        };
    }
    let Ok(end) = last.parse::<u64>() else {
        return RangeVerdict::Whole;
    };
    if end < start {
        return RangeVerdict::Unsatisfiable;
    }
    RangeVerdict::Satisfiable {
        start,
        // A client may ask past the end; clamp rather than refuse. The clamp
        // is not politeness — `end` reaches a caller as a slice bound, so an
        // unclamped one is an out-of-range index driven straight from a
        // request header.
        end: end.min(total - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOTAL: u64 = 1000;

    #[test]
    fn absent_header_is_whole() {
        assert_eq!(parse_range(None, TOTAL), RangeVerdict::Whole);
    }

    #[test]
    fn closed_range_is_inclusive() {
        assert_eq!(
            parse_range(Some("bytes=100-199"), TOTAL),
            RangeVerdict::Satisfiable {
                start: 100,
                end: 199
            }
        );
    }

    #[test]
    fn open_ended_runs_to_the_last_byte() {
        assert_eq!(
            parse_range(Some("bytes=900-"), TOTAL),
            RangeVerdict::Satisfiable {
                start: 900,
                end: 999
            }
        );
    }

    #[test]
    fn suffix_form_takes_the_last_n_bytes() {
        assert_eq!(
            parse_range(Some("bytes=-100"), TOTAL),
            RangeVerdict::Satisfiable {
                start: 900,
                end: 999
            }
        );
    }

    #[test]
    fn a_suffix_longer_than_the_resource_clamps_to_the_whole_resource() {
        assert_eq!(
            parse_range(Some("bytes=-5000"), TOTAL),
            RangeVerdict::Satisfiable { start: 0, end: 999 }
        );
    }

    #[test]
    fn an_end_past_the_resource_clamps_rather_than_refusing() {
        assert_eq!(
            parse_range(Some("bytes=900-99999"), TOTAL),
            RangeVerdict::Satisfiable {
                start: 900,
                end: 999
            }
        );
    }

    #[test]
    fn a_start_past_the_end_is_unsatisfiable() {
        assert_eq!(
            parse_range(Some("bytes=1000-"), TOTAL),
            RangeVerdict::Unsatisfiable
        );
        assert_eq!(
            parse_range(Some("bytes=5000-6000"), TOTAL),
            RangeVerdict::Unsatisfiable
        );
    }

    #[test]
    fn an_inverted_range_is_unsatisfiable() {
        assert_eq!(
            parse_range(Some("bytes=200-100"), TOTAL),
            RangeVerdict::Unsatisfiable
        );
    }

    #[test]
    fn a_zero_length_suffix_is_unsatisfiable() {
        assert_eq!(
            parse_range(Some("bytes=-0"), TOTAL),
            RangeVerdict::Unsatisfiable
        );
    }

    #[test]
    fn an_empty_resource_satisfies_nothing() {
        assert_eq!(
            parse_range(Some("bytes=0-0"), 0),
            RangeVerdict::Unsatisfiable
        );
    }

    /// Answering a multi-range request IN FULL is correct and deliberate.
    /// Returning 416 here would refuse a request we can satisfy.
    #[test]
    fn multi_range_falls_back_to_the_whole_resource() {
        assert_eq!(
            parse_range(Some("bytes=0-99,200-299"), TOTAL),
            RangeVerdict::Whole
        );
    }

    #[test]
    fn malformed_and_foreign_units_fall_back_to_the_whole_resource() {
        for h in ["bytes=", "bytes=abc-def", "items=0-5", "0-99", "bytes=--5"] {
            assert_eq!(
                parse_range(Some(h), TOTAL),
                RangeVerdict::Whole,
                "input: {h}"
            );
        }
    }

    #[test]
    fn content_range_renders_the_wire_form() {
        assert_eq!(
            RangeVerdict::Satisfiable {
                start: 100,
                end: 199
            }
            .content_range(TOTAL),
            Some("bytes 100-199/1000".to_string())
        );
        assert_eq!(
            RangeVerdict::Unsatisfiable.content_range(TOTAL),
            Some("bytes */1000".to_string())
        );
        assert_eq!(RangeVerdict::Whole.content_range(TOTAL), None);
    }
}
