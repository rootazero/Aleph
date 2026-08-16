//! Streaming memory-fence scrubber — hermes-parity defensive filter for
//! LLM stream output that may echo `<memory>...</memory>` fences from the
//! injected envelope back to the user.
//!
//! Why this exists
//! ---------------
//! `MemoryEnvelope::render_markdown_v1` wraps recalled memory in
//! `<memory>...</memory>` markers before injecting it into the system prompt
//! (see `assembler/render.rs`). A naive LLM that echoes the framing back —
//! even partially — would leak that framing into the user-visible response.
//!
//! A one-shot regex on the full text works for non-streaming responses, but
//! cannot survive chunk boundaries: an `<memory>` opened in one delta and
//! closed in a later delta would still leak its payload to the UI because
//! the non-greedy block regex needs both tags in one string.
//!
//! [`StreamingContextScrubber`] runs a tiny state machine across deltas,
//! holds back partial-tag tails between calls, and discards the entire span
//! when both ends are observed. Designed to be cheap (no regex, no
//! allocations on the steady-state happy path) and safe (errs on the side of
//! holding output back rather than emitting a partial leak).
//!
//! Usage
//! -----
//! ```ignore
//! let mut scrubber = StreamingContextScrubber::default();
//! for delta in stream {
//!     let visible = scrubber.feed(&delta);
//!     if !visible.is_empty() { emit(&visible); }
//! }
//! let tail = scrubber.flush();
//! if !tail.is_empty() { emit(&tail); }
//! ```
//!
//! Re-entrancy: create a fresh instance per turn, or call [`Self::reset`].
//!
//! Scope
//! -----
//! The default tag set is `<memory>` (Aleph's `render_markdown_v1` envelope).
//! Custom tag pairs are supported via [`StreamingContextScrubber::with_tags`]
//! — the assembler's XML/JSON render styles use different framing and can
//! reuse the same machinery (`<MemoryEnvelope>` etc.) without duplicating
//! the state machine.

/// Default open fence emitted by `render_markdown_v1`.
pub const DEFAULT_OPEN_TAG: &str = "<memory>";
/// Default close fence emitted by `render_markdown_v1`.
pub const DEFAULT_CLOSE_TAG: &str = "</memory>";

/// The tag pairs the message assembler discards from the visible stream:
/// echoed memory framing, chain-of-thought, and loop completion markers.
/// All ASCII (byte-level scan safe). `<task-complete/>` is self-closing and
/// is handled by the finalize-time `sanitize_llm_output` pass, not here.
pub const DISCARD_TAG_PAIRS: &[(&str, &str)] = &[
    ("<memory-context>", "</memory-context>"),
    // `render_markdown_v1` wraps recalled memory in `<memory>…</memory>`
    // (slot fences like `<user_profile>` live inside it, so discarding the
    // outer pair removes the whole block — and the inner slot fences are
    // also echoed only inside it).
    ("<memory>", "</memory>"),
    ("<think>", "</think>"),
    ("<thinking>", "</thinking>"),
    ("<thought>", "</thought>"),
    ("<antthinking>", "</antthinking>"),
    ("<completion-check>", "</completion-check>"),
];

/// Streaming-safe memory-fence scrubber.
///
/// Holds back fragments at chunk boundaries that could plausibly be the
/// start of an open/close tag, and drops everything between a matched
/// `open` and `close` pair (inclusive). Survives:
///
/// - Tag split across deltas (`"<mem"` then `"ory>"`).
/// - Multiple tag pairs in a stream.
/// - Unterminated open: tail discarded on `flush()`, never leaked.
/// - False-positive partial tag: emitted verbatim on `flush()` if it
///   turned out not to be a real tag.
///
/// **Tag constraint**: both tags must be ASCII (the assembler's render
/// styles all use ASCII fences). The constructor `debug_assert`s this so
/// non-ASCII abuse surfaces in tests rather than silently corrupting
/// UTF-8 char-boundary slicing.
#[derive(Debug, Clone)]
pub struct StreamingContextScrubber {
    /// Discard tag pairs (open, close). One entry for the single-tag ctors.
    pairs: Vec<(String, String)>,
    /// Index into `pairs` of the currently open span, or `None`.
    active: Option<usize>,
    /// Bytes held back across calls because they might be the prefix of a tag.
    buf: String,
}

impl Default for StreamingContextScrubber {
    fn default() -> Self {
        Self::with_tags(DEFAULT_OPEN_TAG, DEFAULT_CLOSE_TAG)
    }
}

impl StreamingContextScrubber {
    /// Construct a scrubber with custom open/close fences. Both tags must
    /// be ASCII; non-ASCII input is rejected via `debug_assert` so byte-
    /// level scanning never breaks UTF-8 char boundaries on real content.
    pub fn with_tags(open: impl Into<String>, close: impl Into<String>) -> Self {
        let open = open.into();
        let close = close.into();
        Self::with_tag_set(&[(open.as_str(), close.as_str())])
    }

    /// Multi-pair discard scrubber. Every pair's tags must be ASCII; non-ASCII
    /// input is rejected via `debug_assert` so byte-level scanning never breaks
    /// UTF-8 char boundaries on real content.
    pub fn with_tag_set(pairs: &[(&str, &str)]) -> Self {
        debug_assert!(!pairs.is_empty(), "at least one tag pair required");
        for (o, c) in pairs {
            debug_assert!(!o.is_empty() && !c.is_empty(), "tags must not be empty");
            debug_assert!(o.is_ascii() && c.is_ascii(), "tags must be ASCII");
        }
        Self {
            pairs: pairs
                .iter()
                .map(|(o, c)| ((*o).to_string(), (*c).to_string()))
                .collect(),
            active: None,
            buf: String::new(),
        }
    }

    /// Reset to fresh state — equivalent to dropping the instance and making
    /// a new one with the same tag pairs.
    pub fn reset(&mut self) {
        self.active = None;
        self.buf.clear();
    }

    /// True if the scrubber is currently inside an unterminated span. Useful
    /// for tests and metrics.
    #[must_use]
    pub const fn in_span(&self) -> bool {
        self.active.is_some()
    }

    /// Feed a streaming text chunk. Returns the visible portion to emit.
    ///
    /// A trailing fragment that could be the start of an open or close tag
    /// is held back internally and surfaced on the next [`Self::feed`] call,
    /// or resolved by [`Self::flush`] at end-of-stream.
    pub fn feed(&mut self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        // Combine carry-over from previous call with the new chunk.
        let mut work = std::mem::take(&mut self.buf);
        work.push_str(text);

        let work_bytes = work.as_bytes();
        let mut out = String::new();
        let mut cursor: usize = 0;

        loop {
            if let Some(active) = self.active {
                let close = self.pairs[active].1.as_bytes();
                match find_ascii_ci(&work_bytes[cursor..], close) {
                    Some(rel_idx) => {
                        // Skip span + close tag entirely.
                        cursor = cursor + rel_idx + close.len();
                        self.active = None;
                        // Loop continues to scan the rest of the buffer.
                    }
                    None => {
                        // No close yet — hold back a possible partial close
                        // tail; drop everything else (it's inside the span).
                        let tail_len = max_partial_suffix_ascii_ci(&work_bytes[cursor..], close);
                        if tail_len > 0 {
                            let start = work.len() - tail_len;
                            // start is on a char boundary: matched bytes are
                            // the ASCII tag prefix, and the byte before
                            // (if any) is a complete UTF-8 char's end.
                            self.buf = work[start..].to_string();
                        }
                        return out;
                    }
                }
            } else {
                // Earliest open tag among all pairs.
                let mut best: Option<(usize, usize)> = None; // (abs_idx, pair_idx)
                for (i, (open, _)) in self.pairs.iter().enumerate() {
                    if let Some(rel) = find_ascii_ci(&work_bytes[cursor..], open.as_bytes()) {
                        let abs = cursor + rel;
                        if best.is_none_or(|(b, _)| abs < b) {
                            best = Some((abs, i));
                        }
                    }
                }
                match best {
                    Some((abs, i)) => {
                        // Emit visible prefix before the open tag.
                        out.push_str(&work[cursor..abs]);
                        cursor = abs + self.pairs[i].0.len();
                        self.active = Some(i);
                        // Loop continues; next iteration searches for close.
                    }
                    None => {
                        // No open tag — hold back the longest partial suffix
                        // matching any open tag. Everything before is emittable.
                        let tail = self
                            .pairs
                            .iter()
                            .map(|(open, _)| {
                                max_partial_suffix_ascii_ci(&work_bytes[cursor..], open.as_bytes())
                            })
                            .max()
                            .unwrap_or(0);
                        if tail > 0 {
                            let split = work.len() - tail;
                            out.push_str(&work[cursor..split]);
                            self.buf = work[split..].to_string();
                        } else {
                            out.push_str(&work[cursor..]);
                        }
                        return out;
                    }
                }
            }
        }
    }

    /// Drain held-back bytes at end-of-stream.
    ///
    /// - Inside an unterminated span: the partial payload is **discarded**
    ///   (leaking a half-fence to the UI is worse than truncating).
    /// - Otherwise: any held tail is emitted verbatim (it turned out not to
    ///   be a real tag).
    pub fn flush(&mut self) -> String {
        if self.active.is_some() {
            self.buf.clear();
            self.active = None;
            return String::new();
        }
        std::mem::take(&mut self.buf)
    }
}

/// ASCII-case-insensitive substring search. The needle is the (ASCII-only)
/// tag bytes; the haystack may contain arbitrary UTF-8 but ASCII bytes can't
/// appear inside a multi-byte sequence (UTF-8 self-synchronisation), so a
/// byte-level match never lands inside one.
fn find_ascii_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    (0..=last).find(|&i| haystack[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Return the length of the longest suffix of `buf` that is a strict prefix
/// of `tag`, comparing ASCII case-insensitively. Returns 0 if none.
///
/// Example: `buf="hello <me"`, `tag="<memory>"` → returns 3 (`<me`).
fn max_partial_suffix_ascii_ci(buf: &[u8], tag: &[u8]) -> usize {
    if tag.len() <= 1 {
        return 0;
    }
    let max_check = std::cmp::min(buf.len(), tag.len() - 1);
    // Walk backwards: try the longest suffix first, return on first match.
    for i in (1..=max_check).rev() {
        if buf[buf.len() - i..].eq_ignore_ascii_case(&tag[..i]) {
            return i;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_feed_returns_empty() {
        let mut s = StreamingContextScrubber::default();
        assert_eq!(s.feed(""), "");
        assert_eq!(s.flush(), "");
    }

    #[test]
    fn text_without_fence_passes_through() {
        let mut s = StreamingContextScrubber::default();
        assert_eq!(s.feed("hello world"), "hello world");
        assert_eq!(s.flush(), "");
    }

    #[test]
    fn full_fence_in_one_chunk_is_stripped() {
        let mut s = StreamingContextScrubber::default();
        let out = s.feed("before <memory>secret</memory> after");
        assert_eq!(out, "before  after");
        assert_eq!(s.flush(), "");
        assert!(!s.in_span());
    }

    #[test]
    fn fence_split_across_chunks_is_stripped() {
        let mut s = StreamingContextScrubber::default();
        // Open tag split across boundary.
        let a = s.feed("hello <mem");
        let b = s.feed("ory>SECRET DATA</mem");
        let c = s.feed("ory> trailing");
        let combined = format!("{a}{b}{c}");
        assert_eq!(combined, "hello  trailing");
        assert_eq!(s.flush(), "");
    }

    #[test]
    fn unterminated_span_discards_payload_on_flush() {
        let mut s = StreamingContextScrubber::default();
        let _ = s.feed("ok <memory>leak ");
        let _ = s.feed("more leak");
        // No close tag arrives.
        let tail = s.flush();
        assert_eq!(tail, "", "unterminated span must NOT leak its body");
        assert!(!s.in_span());
    }

    #[test]
    fn partial_open_tag_at_eof_emits_verbatim_on_flush() {
        let mut s = StreamingContextScrubber::default();
        let visible = s.feed("hello <me");
        // Should hold "<me" back; only "hello " is emitted.
        assert_eq!(visible, "hello ");
        let tail = s.flush();
        assert_eq!(tail, "<me", "false-positive tail must be emitted verbatim");
    }

    #[test]
    fn multiple_fence_pairs_in_stream_are_all_stripped() {
        let mut s = StreamingContextScrubber::default();
        let out = s.feed("A <memory>x</memory> B <memory>y</memory> C");
        assert_eq!(out, "A  B  C");
    }

    #[test]
    fn case_insensitive_tag_matching() {
        let mut s = StreamingContextScrubber::default();
        let out = s.feed("p <MEMORY>data</Memory> q");
        assert_eq!(out, "p  q");
    }

    #[test]
    fn custom_tags_for_memory_envelope_xml_style() {
        let mut s = StreamingContextScrubber::with_tags("<MemoryEnvelope>", "</MemoryEnvelope>");
        let out = s.feed("x <MemoryEnvelope>blob</MemoryEnvelope> y");
        assert_eq!(out, "x  y");
    }

    #[test]
    fn no_false_strip_for_partial_match_inside_text() {
        let mut s = StreamingContextScrubber::default();
        // "<memo" is a prefix of "<memory>" but the rest doesn't follow —
        // the scrubber will hold "<memo" and emit it on the next chunk
        // when the disambiguating char arrives.
        let visible1 = s.feed("foo <memo");
        let visible2 = s.feed("rial bar"); // not a real tag
        assert_eq!(format!("{visible1}{visible2}"), "foo <memorial bar");
        assert_eq!(s.flush(), "");
    }

    #[test]
    fn reset_clears_state_between_turns() {
        let mut s = StreamingContextScrubber::default();
        let _ = s.feed("a <memory>x"); // mid-span
        assert!(s.in_span());
        s.reset();
        assert!(!s.in_span());
        let out = s.feed("clean text");
        assert_eq!(out, "clean text");
    }

    #[test]
    fn many_small_chunks_one_byte_at_a_time() {
        // Stress: feed byte-by-byte, prove the result equals the one-shot
        // behaviour.
        let input = "lead-in <memory>SECRET</memory> tail";
        let expected = "lead-in  tail";
        let mut s = StreamingContextScrubber::default();
        let mut out = String::new();
        for ch in input.chars() {
            let mut buf = [0u8; 4];
            out.push_str(&s.feed(ch.encode_utf8(&mut buf)));
        }
        out.push_str(&s.flush());
        assert_eq!(out, expected);
    }

    #[test]
    fn max_partial_suffix_unit() {
        // "hello <me" + tag "<memory>" => 3 ("<me").
        assert_eq!(max_partial_suffix_ascii_ci(b"hello <me", b"<memory>"), 3);
        // No suffix-prefix overlap.
        assert_eq!(max_partial_suffix_ascii_ci(b"hello", b"<memory>"), 0);
        // Exact-length-minus-one boundary: "<memory" (7) is a strict prefix
        // of "<memory>" (8); since loop iterates 1..=min(7,7), expect 7.
        assert_eq!(max_partial_suffix_ascii_ci(b"<memory", b"<memory>"), 7);
        // Single-character tag has no strict prefix worth holding.
        assert_eq!(max_partial_suffix_ascii_ci(b"x", b">"), 0);
        // Case-insensitive: "<MEM" should still count as prefix of "<memory>".
        assert_eq!(max_partial_suffix_ascii_ci(b"<MEM", b"<memory>"), 4);
    }

    #[test]
    fn find_ascii_ci_unit() {
        assert_eq!(find_ascii_ci(b"hello world", b"world"), Some(6));
        assert_eq!(find_ascii_ci(b"hello WORLD", b"world"), Some(6));
        assert_eq!(find_ascii_ci(b"hello", b"xyz"), None);
        assert_eq!(find_ascii_ci(b"abc", b""), None);
        assert_eq!(find_ascii_ci(b"", b"abc"), None);
    }

    #[test]
    fn fence_with_trailing_newline_is_stripped_cleanly() {
        // Real render output ends with a trailing newline after </memory>.
        let mut s = StreamingContextScrubber::default();
        let out = s.feed("\n<memory>\n\n<user_profile>\nbody\n</user_profile>\n\n</memory>\n");
        assert_eq!(
            out, "\n\n",
            "fence body must be stripped, surrounding newlines preserved"
        );
    }

    #[test]
    fn utf8_text_around_fence_does_not_panic() {
        // Multi-byte chars adjacent to ASCII fence must not break char boundary slicing.
        let mut s = StreamingContextScrubber::default();
        let input = "前置内容 <memory>secret</memory> 尾部内容";
        let out = s.feed(input);
        assert_eq!(out, "前置内容  尾部内容");
        assert_eq!(s.flush(), "");
    }

    #[test]
    fn utf8_text_with_partial_tag_holdback_preserves_boundary() {
        let mut s = StreamingContextScrubber::default();
        // The buffer ends mid-tag right after a multi-byte char ("好" = 3 bytes).
        let a = s.feed("你好 <mem");
        let b = s.feed("ory>x</memory> 完成");
        assert_eq!(format!("{a}{b}"), "你好  完成");
    }

    #[test]
    fn tag_set_strips_think_and_memory_across_boundaries() {
        let mut s = StreamingContextScrubber::with_tag_set(&[
            ("<memory-context>", "</memory-context>"),
            ("<think>", "</think>"),
        ]);
        // <think> split across deltas, interleaved with a memory-context span.
        let a = s.feed("answer <thi");
        let b = s.feed("nk>hidden reasoning</think> and <memory-context>x</memory-context> tail");
        assert_eq!(format!("{a}{b}"), "answer  and  tail");
        assert_eq!(s.flush(), "");
        assert!(!s.in_span());
    }

    #[test]
    fn tag_set_holds_ambiguous_open_prefix_until_disambiguated() {
        let mut s = StreamingContextScrubber::with_tag_set(DISCARD_TAG_PAIRS);
        // "<th" is a prefix of "<think>"/"<thinking>"/"<thought>" — must hold back.
        let v1 = s.feed("keep <th");
        let v2 = s.feed("ursday plans"); // not a tag
        assert_eq!(format!("{v1}{v2}"), "keep <thursday plans");
    }

    /// Drift guard: the memory-context entry in `DISCARD_TAG_PAIRS` must stay
    /// byte-identical to the canonical fence the assembler injects, or echoed
    /// recalled-memory would leak to the user. Pins the literals to the source
    /// of truth without coupling this generic scrubber to that module at
    /// compile time.
    #[test]
    fn discard_tag_pairs_memory_context_matches_canonical_fence() {
        use crate::memory::assembler::context_block::{MEMORY_CONTEXT_CLOSE, MEMORY_CONTEXT_OPEN};
        assert_eq!(
            DISCARD_TAG_PAIRS[0],
            (MEMORY_CONTEXT_OPEN, MEMORY_CONTEXT_CLOSE),
            "DISCARD_TAG_PAIRS[0] drifted from the canonical <memory-context> fence"
        );
    }

    /// Drift guard for the MarkdownV1 outer fence: `render_markdown_v1` wraps
    /// recalled memory in `<memory>…</memory>`, so the scrubber must discard
    /// exactly that pair or an echoed envelope leaks its recalled content.
    /// The literal pins against the renderer's output shape without coupling
    /// this generic scrubber to the assembler at compile time.
    #[test]
    fn discard_tag_pairs_memory_matches_markdown_v1_fence() {
        assert_eq!(
            DISCARD_TAG_PAIRS[1],
            ("<memory>", "</memory>"),
            "DISCARD_TAG_PAIRS[1] drifted from the render_markdown_v1 <memory> fence"
        );
    }
}
