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
    /// "not the bytes unit", "multi-range", and every *invalid* spec: one
    /// that fails to parse, and one whose last-byte-pos is below its
    /// first-byte-pos.
    Whole,
    /// Send 206 with `[start, end]`, both inclusive.
    Satisfiable { start: u64, end: u64 },
    /// Send 416 with `Content-Range: bytes */<total>`.
    ///
    /// Reserved for a spec that is syntactically VALID and simply does not
    /// overlap the resource: a start at or past the end, a zero-length
    /// suffix, or any range against an empty resource. Invalid input is
    /// [`Self::Whole`], not this — RFC 9110 §14.1.1 draws that line, and
    /// conflating the two is the easiest way to answer 416 to a request we
    /// could have served in full.
    Unsatisfiable,
}

impl RangeVerdict {
    /// Does answering this verdict hand the caller most of the resource?
    ///
    /// A route that prices ranged requests from a wider rate bucket — so that
    /// media seeking is not throttled — must still charge a full read as a
    /// full read. The trap is defining "full" as byte-exact. `Range: bytes=1-`
    /// returns every byte but the first, which for any real content type is a
    /// complete copy, and it is a fixed string requiring no knowledge of the
    /// resource's size. An exact-coverage test lets it through; this one does
    /// not. The question is how MUCH is sent, never whether literally
    /// everything is.
    ///
    /// The threshold is half, because a single request for more than half a
    /// resource is not seeking under any reading of the word.
    ///
    /// # What this bounds, and what it does not
    ///
    /// It does not make bulk reading impossible, and no per-request predicate
    /// can. A caller willing to split each resource into two requests stays
    /// under the threshold on both, so what it reaches is the WIDE bucket's
    /// rate halved — not the narrow bucket's. Lowering the threshold trades
    /// that against throttling real playback, which does pull large chunks.
    /// Closing it properly needs byte-budget accounting in the rate limiter
    /// instead of a boolean per request; that is recorded as a follow-up and
    /// deliberately not done here. Do not restate this as "the wide bucket
    /// cannot be used to read more of the resource" — that sentence was in
    /// this codebase once and it was false.
    #[must_use]
    pub fn is_bulk_read(&self, total: u64) -> bool {
        let served = match self {
            Self::Whole => total,
            // Inclusive bounds, and `parse_range` guarantees start <= end.
            Self::Satisfiable { start, end } => end - start + 1,
            Self::Unsatisfiable => 0,
        };
        // `served * 2` cannot overflow: `served <= total`, and `total` is a
        // buffer length, orders of magnitude below `u64::MAX / 2`.
        served * 2 > total
    }

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
        // An inverted range is INVALID, not unsatisfiable (RFC 9110
        // §14.1.1), so it joins the other invalid spellings at `Whole`.
        // §14.2 permits either ignoring or rejecting an invalid spec, so
        // 416 would also conform; ignoring is chosen because it is what
        // this function already does with every other invalid spelling,
        // and singling this one out would buy nothing — a client after the
        // whole body via a bad header can get it from `bytes=abc-def`
        // regardless.
        return RangeVerdict::Whole;
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

    /// Inverted is INVALID, not unsatisfiable — so it is ignored and the
    /// whole resource is sent, not refused with a 416. RFC 9110 §14.1.1
    /// defines the distinction; §14.2 would permit 416 too, and the reason
    /// this file does not take that option is written at the branch.
    #[test]
    fn an_inverted_range_falls_back_to_the_whole_resource() {
        assert_eq!(
            parse_range(Some("bytes=200-100"), TOTAL),
            RangeVerdict::Whole
        );
    }

    /// Whitespace inside the spec is outside the ABNF, but the two `.trim()`
    /// calls after the split absorb it. Untested until now: dropping both
    /// trims left all thirteen other tests green.
    #[test]
    fn whitespace_inside_the_spec_is_absorbed() {
        assert_eq!(
            parse_range(Some("bytes= 100 - 199 "), TOTAL),
            RangeVerdict::Satisfiable {
                start: 100,
                end: 199
            }
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

    /// The case an exact-coverage test misses. `bytes=1-` is size-independent
    /// and returns a complete usable copy of anything; if it is not a bulk
    /// read, a rate bucket meant to bound scraping is bypassed by one header.
    #[test]
    fn a_range_missing_only_the_first_byte_is_still_a_bulk_read() {
        let v = parse_range(Some("bytes=1-"), TOTAL);
        assert_eq!(v, RangeVerdict::Satisfiable { start: 1, end: 999 });
        assert!(v.is_bulk_read(TOTAL));
    }

    #[test]
    fn whole_and_full_coverage_are_bulk_reads() {
        assert!(RangeVerdict::Whole.is_bulk_read(TOTAL));
        assert!(parse_range(Some("bytes=0-"), TOTAL).is_bulk_read(TOTAL));
        assert!(parse_range(Some("bytes=0-999"), TOTAL).is_bulk_read(TOTAL));
        assert!(parse_range(Some("bytes=-1000"), TOTAL).is_bulk_read(TOTAL));
        // The tail half plus one byte — no start-at-zero, still most of it.
        assert!(parse_range(Some("bytes=499-"), TOTAL).is_bulk_read(TOTAL));
    }

    #[test]
    fn a_genuine_slice_is_not_a_bulk_read() {
        assert!(!parse_range(Some("bytes=10-19"), TOTAL).is_bulk_read(TOTAL));
        // Starting at zero does not by itself make a read bulk; a media
        // element's opening probe must stay in the wide bucket.
        assert!(!parse_range(Some("bytes=0-9"), TOTAL).is_bulk_read(TOTAL));
        // Exactly half is not MORE than half.
        assert!(!parse_range(Some("bytes=0-499"), TOTAL).is_bulk_read(TOTAL));
    }

    /// A refusal serves no bytes, so it must not be priced as a full read —
    /// otherwise junk 416 probes drain the narrow bucket.
    #[test]
    fn a_refusal_serves_nothing_and_is_not_a_bulk_read() {
        assert!(!RangeVerdict::Unsatisfiable.is_bulk_read(TOTAL));
        // An empty resource: `Whole` serves zero bytes, so still not bulk.
        assert!(!RangeVerdict::Whole.is_bulk_read(0));
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
