//! Frozen renderings of MEMORY.md and USER.md, captured at session start
//! and reused for every prompt build until evicted by compression / SessionEnd.

use std::time::SystemTime;

use super::budget::{header, usage_pct};
use super::format::serialize;

#[derive(Debug, Clone)]
pub struct CuratedSnapshot {
    pub agent_id: String,
    pub agent_md_block: String,             // <CuratedMemory> XML
    pub user_md_block: Option<String>,      // <UserProfile> XML, optional
    pub captured_at: SystemTime,
}

/// Render the agent-side MEMORY.md as an XML envelope. Empty entries → empty string.
pub fn render_agent_block(
    entries: &[String],
    char_limit: usize,
    near_threshold: f32,
) -> String {
    if entries.is_empty() { return String::new(); }
    let head = header(entries, char_limit, near_threshold);
    let body = serialize(entries);
    format!("<CuratedMemory>\n{head}\n{body}\n</CuratedMemory>")
}

/// Render the user-profile body as an XML envelope with a budget header.
/// `body` is the synthesized USER.md content (already markdown). Truncated
/// to `char_limit` to enforce budget on synthesizer output.
pub fn render_user_block(
    body: &str,
    char_limit: usize,
    near_threshold: f32,
) -> String {
    if body.trim().is_empty() { return String::new(); }
    let truncated: String = if body.chars().count() > char_limit {
        body.chars().take(char_limit).collect()
    } else {
        body.to_string()
    };
    let head = user_header(&truncated, char_limit, near_threshold);
    format!("<UserProfile>\n{head}\n{truncated}\n</UserProfile>")
}

/// Render a budget header for a single user-profile body. Counts chars (not
/// bytes) so multibyte content (e.g., CJK) doesn't appear over-limit, and
/// uses no delimiter accounting (a user profile is a single body, not a
/// §-separated entry list).
fn user_header(body: &str, limit: usize, near_threshold: f32) -> String {
    let used = body.chars().count();
    let pct = usage_pct(used, limit);
    let label = if used > limit {
        format!("OVER BUDGET — {}%", pct)
    } else if (used as f32) >= (limit as f32) * near_threshold {
        format!("NEAR LIMIT — {}%", pct)
    } else {
        format!("{}%", pct)
    };
    format!("[{} — {}/{} chars]", label, used, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_entries_produce_empty_block() {
        assert_eq!(render_agent_block(&[], 100, 0.95), "");
    }

    #[test]
    fn agent_block_contains_header_and_body() {
        let e = vec!["fact one".to_string(), "fact two".to_string()];
        let block = render_agent_block(&e, 100, 0.95);
        assert!(block.starts_with("<CuratedMemory>"));
        assert!(block.ends_with("</CuratedMemory>"));
        assert!(block.contains("/100 chars"));
        assert!(block.contains("fact one"));
        assert!(block.contains("§"));
    }

    #[test]
    fn user_block_truncates_at_limit() {
        let body = "x".repeat(2000);
        let block = render_user_block(&body, 1375, 0.95);
        assert!(block.contains("<UserProfile>"));
        let inside = block.replace("<UserProfile>", "").replace("</UserProfile>", "");
        let xs = inside.matches('x').count();
        assert!(xs <= 1375, "got {xs} xs");
        // Lock in the fix: at-limit must report 100%, not OVER BUDGET.
        assert!(block.contains("100%"), "header should report 100%, not over: {block}");
        assert!(!block.contains("OVER BUDGET"), "must not over-report when truncated to limit");
    }

    #[test]
    fn user_block_empty_body_returns_empty() {
        assert_eq!(render_user_block("", 100, 0.95), "");
        assert_eq!(render_user_block("   \n  ", 100, 0.95), "");
    }

    #[test]
    fn user_block_handles_cjk_without_byte_overflow() {
        // 100 Chinese chars = ~300 bytes UTF-8; under a 200-char limit by char count.
        let body = "中".repeat(100);
        let block = render_user_block(&body, 200, 0.95);
        assert!(block.contains("<UserProfile>"));
        assert!(block.contains("100/200 chars"), "header should count chars not bytes: {block}");
        assert!(!block.contains("OVER BUDGET"));
    }
}
