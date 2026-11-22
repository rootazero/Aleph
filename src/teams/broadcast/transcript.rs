//! Pure function: render group message history as transcript text for prompt injection.
//!
//! Uses `[sender]` prefix (openteams style, so the agent sees who said what); when the
//! token budget is exceeded, retains from the tail (most recent). Zero IO, host-testable.

/// Render `(from, content)` history as `[from]: content` multi-line text, oldest-first.
///
/// When exceeding `token_budget`, retains from the tail (most recent), roughly
/// estimating tokens as `chars/4`.
#[must_use]
pub fn format_transcript(history: &[(String, String)], token_budget: usize) -> String {
    let mut used = 0usize;
    let mut start_idx = history.len();
    for (i, (from, content)) in history.iter().enumerate().rev() {
        let char_count = from.chars().count() + content.chars().count() + 4; // "[", "]: "
        let cost = char_count / 4 + 1; // rough token estimate
        if used + cost > token_budget && start_idx < history.len() {
            break;
        }
        used += cost;
        start_idx = i;
    }

    let mut out = String::new();
    for (i, (from, content)) in history[start_idx..].iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push('[');
        out.push_str(from);
        out.push_str("]: ");
        out.push_str(content);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: &str, content: &str) -> (String, String) {
        (from.to_string(), content.to_string())
    }

    #[test]
    fn formats_with_sender_prefix_oldest_first() {
        let msgs = vec![
            line("user", "大家好"),
            line("alice", "你好"),
            line("bob", "我也在"),
        ];
        let out = format_transcript(&msgs, 10_000);
        assert_eq!(out, "[user]: 大家好\n[alice]: 你好\n[bob]: 我也在");
    }

    #[test]
    fn empty_history_yields_empty_string() {
        assert_eq!(format_transcript(&[], 10_000), "");
    }

    #[test]
    fn over_budget_keeps_most_recent_from_tail() {
        // each line ≈ 4 tokens (rough len/4), tiny budget → only last line retained
        let msgs = vec![
            line("a", "aaaaaaaaaaaaaaaa"),
            line("b", "bbbbbbbbbbbbbbbb"),
            line("c", "cc"),
        ];
        let out = format_transcript(&msgs, 3); // tiny budget
        assert!(out.contains("[c]: cc"), "most recent line must be retained");
        assert!(!out.contains("[a]:"), "oldest is truncated");
    }
}
