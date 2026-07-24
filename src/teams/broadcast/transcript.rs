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
    // Accumulate from most recent backward until budget is exhausted, then reverse to oldest-first.
    let mut kept_rev: Vec<String> = Vec::new();
    let mut used = 0usize;
    for (from, content) in history.iter().rev() {
        let line = format!("[{from}]: {content}");
        let cost = line.chars().count() / 4 + 1; // rough token estimate
        if used + cost > token_budget && !kept_rev.is_empty() {
            break;
        }
        used += cost;
        kept_rev.push(line);
    }
    kept_rev.reverse();
    kept_rev.join("\n")
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
