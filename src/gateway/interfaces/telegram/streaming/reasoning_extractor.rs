/// Incremental reasoning extractor for streaming `<think>` blocks.
pub struct ReasoningExtractor {
    state: ExtractorState,
    pending: String,
}

#[derive(Debug, Clone, PartialEq)]
enum ExtractorState {
    Answer,
    InReasoning,
    Done,
}

const MAX_TAG_LEN: usize = 20;
const THINK_TAGS: [&str; 4] = ["think", "thinking", "thought", "antthinking"];

impl ReasoningExtractor {
    pub fn new() -> Self {
        Self {
            state: ExtractorState::Answer,
            pending: String::new(),
        }
    }

    /// Process an incremental chunk.
    /// Returns `(reasoning_delta, answer_delta, reasoning_is_complete)`.
    pub fn process_chunk(&mut self, chunk: &str) -> (Option<String>, Option<String>, bool) {
        if self.state == ExtractorState::Done {
            return (None, Some(chunk.to_string()), true);
        }

        let input = if self.pending.is_empty() {
            chunk.to_string()
        } else {
            format!("{}{}", self.pending, chunk)
        };
        self.pending.clear();

        match self.state {
            ExtractorState::Answer => self.process_answer(&input),
            ExtractorState::InReasoning => self.process_reasoning(&input),
            ExtractorState::Done => unreachable!(),
        }
    }

    fn process_answer(&mut self, input: &str) -> (Option<String>, Option<String>, bool) {
        if let Some((answer_part, after_open)) = Self::split_at_think_open(input) {
            self.state = ExtractorState::InReasoning;
            if after_open.is_empty() {
                let answer = if answer_part.is_empty() {
                    None
                } else {
                    Some(answer_part.to_string())
                };
                return (None, answer, false);
            }
            let (reasoning, answer2, complete) = self.process_reasoning(after_open);
            let final_answer = match answer2 {
                Some(a) if !answer_part.is_empty() => Some(format!("{}\n{}", answer_part, a)),
                Some(a) => Some(a),
                None => {
                    if answer_part.is_empty() {
                        None
                    } else {
                        Some(answer_part.to_string())
                    }
                }
            };
            return (reasoning, final_answer, complete);
        }

        let split_pos = Self::find_tag_split(input);
        if split_pos < input.len() {
            self.pending = input[split_pos..].to_string();
            let answer = &input[..split_pos];
            if answer.is_empty() {
                (None, None, false)
            } else {
                (None, Some(answer.to_string()), false)
            }
        } else {
            if input.is_empty() {
                (None, None, false)
            } else {
                (None, Some(input.to_string()), false)
            }
        }
    }

    fn process_reasoning(&mut self, input: &str) -> (Option<String>, Option<String>, bool) {
        if let Some((reasoning_part, after_close)) = Self::split_at_think_close(input) {
            self.state = ExtractorState::Done;
            let reasoning = if reasoning_part.is_empty() {
                None
            } else {
                Some(reasoning_part.to_string())
            };
            let answer = if after_close.is_empty() {
                None
            } else {
                Some(after_close.to_string())
            };
            return (reasoning, answer, true);
        }

        let split_pos = Self::find_close_tag_split(input);
        if split_pos < input.len() {
            self.pending = input[split_pos..].to_string();
            let reasoning = &input[..split_pos];
            if reasoning.is_empty() {
                (None, None, false)
            } else {
                (Some(reasoning.to_string()), None, false)
            }
        } else {
            if input.is_empty() {
                (None, None, false)
            } else {
                (Some(input.to_string()), None, false)
            }
        }
    }

    fn split_at_think_open(input: &str) -> Option<(&str, &str)> {
        let bytes = input.as_bytes();
        for i in 0..bytes.len() {
            if bytes[i] == b'<' {
                for tag in THINK_TAGS.iter() {
                    let open_len = 1 + tag.len() + 1; // < + tag + >
                    let end = i + open_len;
                    if end <= bytes.len()
                        && input[i + 1..i + 1 + tag.len()].eq_ignore_ascii_case(tag)
                        && bytes[i + 1 + tag.len()] == b'>'
                    {
                        return Some((&input[..i], &input[end..]));
                    }
                }
            }
        }
        None
    }

    fn split_at_think_close(input: &str) -> Option<(&str, &str)> {
        let bytes = input.as_bytes();
        for i in 0..bytes.len().saturating_sub(1) {
            if bytes[i] == b'<' && bytes[i + 1] == b'/' {
                for tag in THINK_TAGS.iter() {
                    let close_len = 2 + tag.len() + 1; // </ + tag + >
                    let end = i + close_len;
                    if end <= bytes.len()
                        && input[i + 2..i + 2 + tag.len()].eq_ignore_ascii_case(tag)
                        && bytes[i + 2 + tag.len()] == b'>'
                    {
                        return Some((&input[..i], &input[end..]));
                    }
                }
            }
        }
        None
    }

    fn find_tag_split(input: &str) -> usize {
        let start = input.len().saturating_sub(MAX_TAG_LEN);
        if let Some(pos) = input[start..].rfind('<') {
            start + pos
        } else {
            input.len()
        }
    }

    fn find_close_tag_split(input: &str) -> usize {
        let start = input.len().saturating_sub(MAX_TAG_LEN);
        if let Some(pos) = input[start..].rfind("</") {
            start + pos
        } else {
            input.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_think_tags() {
        let mut extractor = ReasoningExtractor::new();
        let (reasoning, answer, complete) = extractor.process_chunk("Hello world");
        assert!(reasoning.is_none());
        assert_eq!(answer, Some("Hello world".to_string()));
        assert!(!complete);
    }

    #[test]
    fn test_case_insensitive_tags() {
        let mut extractor = ReasoningExtractor::new();
        let (reasoning, answer, complete) = extractor.process_chunk("Start <THINK>R1</THINK> End");
        assert_eq!(reasoning, Some("R1".to_string()));
        assert_eq!(answer, Some("Start \n End".to_string()));
        assert!(complete);
    }

    #[test]
    fn test_partial_open_tag_across_chunks() {
        let mut extractor = ReasoningExtractor::new();
        let (r1, a1, c1) = extractor.process_chunk("Hello <thi");
        assert!(r1.is_none());
        assert_eq!(a1, Some("Hello ".to_string()));
        assert!(!c1);

        let (r2, a2, c2) = extractor.process_chunk("nk>reasoning</think> world");
        assert_eq!(r2, Some("reasoning".to_string()));
        assert_eq!(a2, Some(" world".to_string()));
        assert!(c2);
    }

    #[test]
    fn test_partial_close_tag_across_chunks() {
        let mut extractor = ReasoningExtractor::new();
        let (r1, a1, c1) = extractor.process_chunk("<think>reasoning </thi");
        assert_eq!(r1, Some("reasoning ".to_string()));
        assert!(a1.is_none());
        assert!(!c1);

        let (r2, a2, c2) = extractor.process_chunk("nk> world");
        assert!(r2.is_none());
        assert_eq!(a2, Some(" world".to_string()));
        assert!(c2);
    }

    #[test]
    fn test_multiple_chunks_reasoning_stream() {
        let mut extractor = ReasoningExtractor::new();
        extractor.process_chunk("<think>");
        let (r1, a1, c1) = extractor.process_chunk("step 1...");
        assert_eq!(r1, Some("step 1...".to_string()));
        assert!(a1.is_none());
        assert!(!c1);

        let (r2, a2, c2) = extractor.process_chunk(" step 2...");
        assert_eq!(r2, Some(" step 2...".to_string()));
        assert!(a2.is_none());
        assert!(!c2);

        let (r3, a3, c3) = extractor.process_chunk("</think> answer");
        assert!(r3.is_none());
        assert_eq!(a3, Some(" answer".to_string()));
        assert!(c3);
    }

    #[test]
    fn test_answer_after_complete_is_all_answer() {
        let mut extractor = ReasoningExtractor::new();
        extractor.process_chunk("<think>r</think> a1");
        let (r, a, c) = extractor.process_chunk(" a2");
        assert!(r.is_none());
        assert_eq!(a, Some(" a2".to_string()));
        assert!(c);
    }
}
