//! Frozen renderings of MEMORY.md and USER.md, captured at session start
//! and reused for every prompt build until evicted by compression / `SessionEnd`.

use std::time::SystemTime;

use super::budget::{header, usage_pct};
use super::format::serialize;

#[derive(Debug, Clone)]
pub struct CuratedSnapshot {
    pub agent_id: String,
    pub agent_md_block: String,           // <CuratedMemory> XML
    pub user_md_block: Option<String>,    // <UserProfile> XML, optional
    pub open_loops_block: Option<String>, // <OpenLoops> XML, optional (Batch 2 open-loop tracking)
    pub captured_at: SystemTime,
}

/// Render the agent-side MEMORY.md as an XML envelope. Empty entries → empty string.
#[must_use]
pub fn render_agent_block(entries: &[String], char_limit: usize, near_threshold: f32) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let head = header(entries, char_limit, near_threshold);
    let body = serialize(entries);
    format!("<CuratedMemory>\n{head}\n{body}\n</CuratedMemory>")
}

/// Render the user-profile body as an XML envelope with a budget header.
/// `body` is the synthesized USER.md content (already markdown). Truncated
/// to `char_limit` to enforce budget on synthesizer output.
#[must_use]
pub fn render_user_block(body: &str, char_limit: usize, near_threshold: f32) -> String {
    if body.trim().is_empty() {
        return String::new();
    }
    let truncated: String = if body.chars().count() > char_limit {
        body.chars().take(char_limit).collect()
    } else {
        body.to_string()
    };
    let head = user_header(&truncated, char_limit, near_threshold);
    format!("<UserProfile>\n{head}\n{truncated}\n</UserProfile>")
}

/// Render last session's unresolved follow-ups as an XML envelope. Injected at
/// the start of the next session so the agent can proactively pick them back up
/// (R5 — "AI proactively reaches out"). `body` is the persisted `OPEN_LOOPS.md` markdown,
/// truncated to `char_limit` (counted in chars, CJK-safe) to bound the prompt.
/// Empty body → empty string (caller omits the block).
#[must_use]
pub fn render_open_loops_block(body: &str, char_limit: usize) -> String {
    if body.trim().is_empty() {
        return String::new();
    }
    let truncated: String = if body.chars().count() > char_limit {
        body.chars().take(char_limit).collect()
    } else {
        body.to_string()
    };
    format!(
        "<OpenLoops>\n[Unresolved items from your last session with this user. \
         Proactively follow up on any still relevant; skip ones already resolved.]\n{}\n</OpenLoops>",
        truncated.trim()
    )
}

/// Render a budget header for a single user-profile body. Counts chars (not
/// bytes) so multibyte content (e.g., CJK) doesn't appear over-limit, and
/// uses no delimiter accounting (a user profile is a single body, not a
/// §-separated entry list).
fn user_header(body: &str, limit: usize, near_threshold: f32) -> String {
    let used = body.chars().count();
    let pct = usage_pct(used, limit);
    let label = if used > limit {
        format!("OVER BUDGET — {pct}%")
    } else if (used as f32) >= (limit as f32) * near_threshold {
        format!("NEAR LIMIT — {pct}%")
    } else {
        format!("{pct}%")
    };
    format!("[{label} — {used}/{limit} chars]")
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
        let inside = block
            .replace("<UserProfile>", "")
            .replace("</UserProfile>", "");
        let xs = inside.matches('x').count();
        assert!(xs <= 1375, "got {xs} xs");
        // Lock in the fix: at-limit must report 100%, not OVER BUDGET.
        assert!(
            block.contains("100%"),
            "header should report 100%, not over: {block}"
        );
        assert!(
            !block.contains("OVER BUDGET"),
            "must not over-report when truncated to limit"
        );
    }

    #[test]
    fn user_block_empty_body_returns_empty() {
        assert_eq!(render_user_block("", 100, 0.95), "");
        assert_eq!(render_user_block("   \n  ", 100, 0.95), "");
    }

    #[test]
    fn open_loops_block_wraps_body_and_truncates() {
        assert_eq!(render_open_loops_block("", 100), "");
        assert_eq!(render_open_loops_block("   \n ", 100), "");
        let block =
            render_open_loops_block("- chase the failing wasm build\n- ask about deploy", 200);
        assert!(block.starts_with("<OpenLoops>"));
        assert!(block.ends_with("</OpenLoops>"));
        assert!(block.contains("chase the failing wasm build"));
        // Truncation honours char_limit (CJK-safe count, no byte panic).
        let long = "网".repeat(500);
        let block = render_open_loops_block(&long, 100);
        let inside = block.replace("<OpenLoops>", "").replace("</OpenLoops>", "");
        assert!(
            inside.matches('网').count() <= 100,
            "must truncate to char_limit"
        );
    }

    #[test]
    fn user_block_handles_cjk_without_byte_overflow() {
        // 100 Chinese chars = ~300 bytes UTF-8; under a 200-char limit by char count.
        let body = "中".repeat(100);
        let block = render_user_block(&body, 200, 0.95);
        assert!(block.contains("<UserProfile>"));
        assert!(
            block.contains("100/200 chars"),
            "header should count chars not bytes: {block}"
        );
        assert!(!block.contains("OVER BUDGET"));
    }
}
