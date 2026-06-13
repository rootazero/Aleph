//! Incremental sentence splitter for the TTS pipeline. Pure — host-testable.
//! `push` receives the FULL accumulated text each time (the chat message
//! content signal grows monotonically); internal byte offset tracks what
//! has already been consumed. Code-fence bodies are skipped for speech.

const TERMINALS: &[char] = &['。', '！', '？', '!', '?', '.', '\n', '；', ';'];
/// Sentences shorter than this (in chars) are held and merged forward.
const MIN_CHARS: usize = 3;

#[derive(Default)]
pub(crate) struct SentenceSplitter {
    /// Byte offset into the accumulated text already consumed.
    consumed: usize,
    /// Short fragment held back, waiting to merge with the next sentence.
    pending: String,
    in_code_fence: bool,
}

impl SentenceSplitter {
    /// Feed the full accumulated text; returns newly completed sentences.
    pub(crate) fn push(&mut self, full_text: &str) -> Vec<String> {
        let Some(new) = full_text.get(self.consumed..) else { return Vec::new() };
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
                    let prev_digit =
                        new[..i].chars().next_back().is_some_and(|c| c.is_ascii_digit());
                    let next_digit = new[end..].chars().next().is_some_and(|c| c.is_ascii_digit());
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
        if let Some(rest) = full_text.get(self.consumed..) {
            if !self.in_code_fence {
                self.pending.push_str(rest);
            }
            self.consumed = full_text.len();
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
        if trimmed.chars().count() < MIN_CHARS {
            self.pending = candidate;
        } else {
            out.push(trimmed.to_string());
            self.pending.clear();
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
        assert_eq!(sp.push("今天有 3 个安排。第一个"), vec!["今天有 3 个安排。"]);
    }

    #[test]
    fn handles_mixed_cjk_ascii_terminals() {
        let mut sp = SentenceSplitter::default();
        let out = sp.push("Hello there! 你好吗？还行");
        assert_eq!(out, vec!["Hello there!", "你好吗？"]);
    }

    #[test]
    fn short_fragment_merges_into_next() {
        let mut sp = SentenceSplitter::default();
        // "好。" alone is below MIN_CHARS — held and merged with the next sentence
        assert!(sp.push("好。").is_empty());
        assert_eq!(sp.push("好。我马上安排今天的事项。"), vec!["好。我马上安排今天的事项。"]);
    }

    #[test]
    fn code_fence_content_is_skipped() {
        let mut sp = SentenceSplitter::default();
        let text = "看这段代码。\n```rust\nfn main() { println!(\"x.y!\"); }\n```\n运行就好。";
        let out = sp.push(text);
        assert_eq!(out, vec!["看这段代码。", "运行就好。"]);
    }

    #[test]
    fn finish_flushes_tail_without_terminal() {
        let mut sp = SentenceSplitter::default();
        let text = "最后一句没有标点";
        assert!(sp.push(text).is_empty());
        assert_eq!(sp.finish_with(text), Some("最后一句没有标点".to_string()));
    }

    #[test]
    fn incremental_pushes_never_duplicate() {
        let mut sp = SentenceSplitter::default();
        let mut all = Vec::new();
        for cut in ["你好。", "你好。今天", "你好。今天天气很好。", "你好。今天天气很好。出门记得带伞。"] {
            all.extend(sp.push(cut));
        }
        assert_eq!(all, vec!["你好。", "今天天气很好。", "出门记得带伞。"]);
    }
}
