//! 纯函数:把群消息历史渲染为注入 prompt 的 transcript 文本。
//!
//! 带 `[发言人]` 前缀(openteams 风格,让 agent 看见谁说了什么),超 token 预算
//! 从尾部(最近)保留。无 IO,host 可测。

/// 把 `(from, content)` 历史渲染为 `[from]: content` 多行文本,oldest-first。
///
/// 超 `token_budget` 时从尾部(最近)保留,粗略按 `chars/4` 估 token。
#[must_use]
pub fn format_transcript(history: &[(String, String)], token_budget: usize) -> String {
    // 从最近往旧累加,直到预算用尽,再反转回 oldest-first。
    let mut kept_rev: Vec<String> = Vec::new();
    let mut used = 0usize;
    for (from, content) in history.iter().rev() {
        let line = format!("[{from}]: {content}");
        let cost = line.chars().count() / 4 + 1; // 粗略 token 估计
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
        let msgs = vec![line("user", "大家好"), line("alice", "你好"), line("bob", "我也在")];
        let out = format_transcript(&msgs, 10_000);
        assert_eq!(out, "[user]: 大家好\n[alice]: 你好\n[bob]: 我也在");
    }

    #[test]
    fn empty_history_yields_empty_string() {
        assert_eq!(format_transcript(&[], 10_000), "");
    }

    #[test]
    fn over_budget_keeps_most_recent_from_tail() {
        // 每行约 ~4 token(粗略 len/4),预算极小 → 只保留最后一行
        let msgs = vec![
            line("a", "aaaaaaaaaaaaaaaa"),
            line("b", "bbbbbbbbbbbbbbbb"),
            line("c", "cc"),
        ];
        let out = format_transcript(&msgs, 3); // 极小预算
        assert!(out.contains("[c]: cc"), "必须保留最近一条");
        assert!(!out.contains("[a]:"), "最旧的被截断");
    }
}
