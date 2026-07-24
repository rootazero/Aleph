//! Incremental sentence splitter for the TTS pipeline. Pure — host-testable.
//! `push` receives the FULL accumulated text each time (the chat message
//! content signal grows monotonically); internal byte offset tracks what
//! has already been consumed. Code-fence bodies are skipped for speech.

const TERMINALS: &[char] = &['。', '！', '？', '!', '?', '.', '\n', '；', ';'];
/// Time-to-first-audio matters, so the FIRST chunk of a reply is emitted as soon
/// as it has a little substance. Every later chunk merges short sentences forward
/// until it reaches `MERGE_CHARS`, so the bulk of a long reply becomes fewer,
/// larger TTS requests. Each chunk is a separate round-trip to a sometimes-flaky
/// backend, so fewer-but-larger chunks mean fewer dropped fragments and fewer
/// inter-chunk gaps — robust stream-while-speaking (stream-while-speaking) for long replies.
const FIRST_CHUNK_CHARS: usize = 6;
const MERGE_CHARS: usize = 30;

#[derive(Default)]
pub(crate) struct SentenceSplitter {
    /// Byte offset into the accumulated text already consumed.
    consumed: usize,
    /// Short fragment held back, waiting to merge with the next sentence.
    pending: String,
    /// Once the first chunk is out, later chunks use the larger `MERGE_CHARS`
    /// threshold — the first chunk stays small for a fast time-to-first-audio.
    emitted_any: bool,
    in_code_fence: bool,
    /// The last `full_text` we consumed against. The bubble content is NOT always
    /// append-only: the agent loop REPLACES it with authoritative text
    /// (`set_step_text` on `text_emitted`, `finalize_answer` on `run_complete`).
    /// When the new text diverges from this before `consumed`, the byte offset is
    /// stale — [`resync`] rolls it back so the corrected tail is still spoken
    /// instead of silently dropped (the "last sentence has no voice" bug).
    last_full: String,
}

/// Longest shared leading run of two strings, backed off to a char boundary that
/// is valid in BOTH — safe to use as a byte offset into either.
fn common_prefix_len(a: &str, b: &str) -> usize {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let max = ab.len().min(bb.len());
    let mut i = 0;
    while i < max && ab[i] == bb[i] {
        i += 1;
    }
    while i > 0 && (!a.is_char_boundary(i) || !b.is_char_boundary(i)) {
        i -= 1;
    }
    i
}

impl SentenceSplitter {
    /// Detect non-monotonic content (a replacement, not an append) and roll the
    /// consumed offset back to the divergence point. A no-op for true append-only
    /// growth (the new text extends the old, so the divergence point is at or past
    /// `consumed`). On divergence within the consumed region, re-processing the
    /// corrected tail may re-speak a small diverged fragment — ephemeral audio,
    /// far better than dropping the final sentence entirely.
    fn resync(&mut self, full_text: &str) {
        let common = common_prefix_len(&self.last_full, full_text);
        if common < self.consumed {
            self.consumed = common;
            self.pending.clear();
            // Reset fence state too: a stale `in_code_fence` would make push()
            // swallow the re-processed tail as code body and finish_with() skip
            // it, dropping the authoritative reply from TTS. Err toward speaking.
            self.in_code_fence = false;
        }
    }

    /// Feed the full accumulated text; returns newly completed sentences.
    pub(crate) fn push(&mut self, full_text: &str) -> Vec<String> {
        self.resync(full_text);
        let Some(new) = full_text.get(self.consumed..) else {
            self.last_full = full_text.to_string();
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut seg_start = 0usize; // byte offset within `new`

        let mut iter = new.char_indices().peekable();
        while let Some((i, ch)) = iter.next() {
            // Track ``` fences on their own: toggle and cut the segment around them.
            if ch == '`' && new[i..].starts_with("```") {
                // flush text before the fence marker
                self.take_segment(&new[seg_start..i], &mut out);
                self.in_code_fence = !self.in_code_fence;
                // skip the marker itself
                let after = i + 3;
                // fast-forward iterator past the marker
                while iter.peek().is_some_and(|(j, _)| *j < after) {
                    iter.next();
                }
                seg_start = after;
                continue;
            }
            if self.in_code_fence {
                seg_start = i + ch.len_utf8();
                continue;
            }
            if TERMINALS.contains(&ch) {
                let end = i + ch.len_utf8();
                // Don't split "3.5" style decimals: '.' flanked by ascii digits.
                if ch == '.' {
                    let prev_digit = new[..i]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_ascii_digit());
                    let next_digit = new[end..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_digit());
                    if prev_digit && next_digit {
                        continue;
                    }
                }
                self.take_segment(&new[seg_start..end], &mut out);
                seg_start = end;
            }
        }
        // Everything before seg_start is consumed; the tail stays for next push.
        self.consumed += seg_start;
        self.last_full = full_text.to_string();
        out
    }

    /// Stream ended: flush whatever is held (pending fragment + nothing else).
    pub(crate) fn finish(&mut self) -> Option<String> {
        let tail = std::mem::take(&mut self.pending);
        let tail = tail.trim().to_string();
        (!tail.is_empty()).then_some(tail)
    }

    /// Called with finish after the final full text to flush the unconsumed tail.
    pub(crate) fn finish_with(&mut self, full_text: &str) -> Option<String> {
        self.resync(full_text);
        if let Some(rest) = full_text.get(self.consumed..) {
            if !self.in_code_fence {
                self.pending.push_str(rest);
            }
            self.consumed = full_text.len();
            self.last_full = full_text.to_string();
        }
        self.finish()
    }

    fn take_segment(&mut self, seg: &str, out: &mut Vec<String>) {
        let candidate = format!("{}{}", self.pending, seg);
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            self.pending.clear();
            return;
        }
        // First chunk stays small (fast first audio); later chunks merge forward
        // until they reach MERGE_CHARS, so a long reply is fewer, larger requests.
        let threshold = if self.emitted_any {
            MERGE_CHARS
        } else {
            FIRST_CHUNK_CHARS
        };
        if trimmed.chars().count() < threshold {
            // Too short on its own — hold and merge it into the next segment.
            self.pending = candidate;
        } else {
            out.push(trimmed.to_string());
            self.pending.clear();
            self.emitted_any = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_sentence_on_terminal_punct() {
        let mut sp = SentenceSplitter::default();
        assert!(sp.push("今天有 3 个安排").is_empty());
        assert_eq!(
            sp.push("今天有 3 个安排。第一个"),
            vec!["今天有 3 个安排。"]
        );
    }

    #[test]
    fn mixed_terminals_first_emits_rest_merges() {
        let mut sp = SentenceSplitter::default();
        // "Hello there!" (12 chars) clears FIRST_CHUNK_CHARS → emits immediately
        // (fast first audio). "你好吗？" (4 chars) is past the first chunk, so it
        // merges forward under the larger MERGE_CHARS threshold instead of being
        // synthesized as its own tiny request.
        let out = sp.push("Hello there! 你好吗？还行");
        assert_eq!(out, vec!["Hello there!"]);
    }

    #[test]
    fn short_fragment_merges_into_next() {
        let mut sp = SentenceSplitter::default();
        // "好。" alone is below FIRST_CHUNK_CHARS — held and merged with the next.
        assert!(sp.push("好。").is_empty());
        assert_eq!(
            sp.push("好。我马上安排今天的事项。"),
            vec!["好。我马上安排今天的事项。"]
        );
    }

    #[test]
    fn later_short_sentences_merge_until_finish() {
        let mut sp = SentenceSplitter::default();
        // First chunk emits fast (6 chars).
        assert_eq!(sp.push("我们开始吧。"), vec!["我们开始吧。"]);
        // Now in merge mode: short sentences accumulate instead of each becoming
        // its own TTS request — fewer round-trips to a flaky backend.
        assert!(sp.push("我们开始吧。好。").is_empty());
        assert!(sp.push("我们开始吧。好。行。").is_empty());
        // The held chunk flushes at stream end, so nothing is ever lost.
        assert_eq!(
            sp.finish_with("我们开始吧。好。行。"),
            Some("好。行。".to_string())
        );
    }

    #[test]
    fn code_fence_content_is_skipped() {
        let mut sp = SentenceSplitter::default();
        let text = "看这段代码。\n```rust\nfn main() { println!(\"x.y!\"); }\n```\n运行就好。";
        let out = sp.push(text);
        // Fence body never reaches TTS; the trailing short line merges forward and
        // is flushed at finish.
        assert_eq!(out, vec!["看这段代码。"]);
        assert_eq!(sp.finish_with(text), Some("运行就好。".to_string()));
    }

    #[test]
    fn finish_flushes_tail_without_terminal() {
        let mut sp = SentenceSplitter::default();
        let text = "最后一句没有标点";
        assert!(sp.push(text).is_empty());
        assert_eq!(sp.finish_with(text), Some("最后一句没有标点".to_string()));
    }

    #[test]
    fn authoritative_overwrite_shorter_still_flushes_tail() {
        // The bubble content is NOT append-only: `set_step_text` / `finalize_answer`
        // REPLACE the streamed preview with authoritative text that can be SHORTER
        // than the bytes already consumed. Before the resync guard, the offset went
        // stale and `full.get(consumed..)` returned None → the whole authoritative
        // reply was dropped (text on screen, no voice). It must still be spoken.
        let mut sp = SentenceSplitter::default();
        // Preview streams in; first sentence emitted, offset advances well past the
        // length of the shorter authoritative answer that replaces it.
        assert_eq!(
            sp.push("你好！我能听到你说话。有什么"),
            vec!["你好！我能听到你说话。"]
        );
        // Authoritative final text replaces the preview, shorter than `consumed`.
        assert!(sp.push("好的，没问题。").is_empty());
        assert_eq!(
            sp.finish_with("好的，没问题。"),
            Some("好的，没问题。".to_string())
        );
    }

    #[test]
    fn authoritative_overwrite_diverging_tail_not_dropped() {
        // Preview and authoritative share a lead but diverge at the tail (common in
        // practice — the model's final answer differs from the token preview). The
        // diverged authoritative tail must reach TTS, not be skipped by a stale
        // byte offset that lands mid-different-content.
        let mut sp = SentenceSplitter::default();
        assert_eq!(sp.push("今天的安排。还有"), vec!["今天的安排。"]);
        // Replace with authoritative text that keeps the spoken lead but rewrites
        // the unspoken tail with a real terminal sentence.
        let tail = sp.finish_with("今天的安排。还有三个会议要参加。");
        assert_eq!(tail, Some("还有三个会议要参加。".to_string()));
    }

    #[test]
    fn incremental_pushes_never_duplicate() {
        let mut sp = SentenceSplitter::default();
        let mut all = Vec::new();
        for cut in [
            "你好。",
            "你好。今天",
            "你好。今天天气很好。",
            "你好。今天天气很好。出门记得带伞。",
        ] {
            all.extend(sp.push(cut));
        }
        // The short lead "你好。" merges into the first chunk; the trailing short
        // sentence is held for the next chunk. Crucially, no text is duplicated
        // or lost across incremental pushes.
        assert_eq!(all, vec!["你好。今天天气很好。"]);
        assert_eq!(
            sp.finish_with("你好。今天天气很好。出门记得带伞。"),
            Some("出门记得带伞。".to_string())
        );
    }
}
