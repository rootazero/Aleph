//! Shared summary utilities for compaction and session compactor.
//!
//! Provides the identifier preservation directive, the analysis-block
//! stripping helper, and the single-source summarization prompt builder used by
//! both [`super::compactor`] and the session-split seed path.

use crate::providers::message::UnifiedMessage;

/// Maximum characters of the live-task focus anchor embedded in a summarization
/// prompt. Bounds prompt growth so task-anchoring never bloats the side-channel
/// summarizer call; the anchor points at the active task, it does not carry it.
const FOCUS_ANCHOR_MAX_CHARS: usize = 600;

/// Appended to every summarization prompt to instruct the LLM to copy
/// technical identifiers verbatim rather than paraphrasing them.
pub const IDENTIFIER_PRESERVATION: &str = "\n\n\
## Identifier Preservation (MANDATORY)\n\
When summarizing, you MUST preserve the following identifiers EXACTLY as they appear \
in the original text — do not shorten, paraphrase, or reconstruct them:\n\
- File paths (e.g., src/memory/store/lance/mod.rs)\n\
- UUIDs and hashes (e.g., a1b2c3d4-...)\n\
- URLs and endpoints (e.g., https://api.example.com/v1/...)\n\
- Commit references (e.g., 0949c9fc)\n\
- Version numbers (e.g., v26.4.2)\n\
- Configuration keys and environment variables\n\
- Error codes and status codes\n\
\n\
If an identifier is not relevant to the summary's core meaning, omit it entirely \
rather than abbreviating it.";

// ASSUMPTION: LLM output contains at most one <analysis>...</analysis> block with no nesting.
/// Strip the `<analysis>...</analysis>` scratchpad from LLM summary output.
///
/// The analysis block gives the LLM reasoning space but should not enter
/// the context window. If no analysis block is found, returns input unchanged.
#[must_use]
pub fn strip_analysis_block(text: &str) -> String {
    if let Some(start) = text.find("<analysis>") {
        if let Some(end) = text.find("</analysis>") {
            // Only strip when the closing tag appears after the opening tag.
            if end > start {
                let after_end = end + "</analysis>".len();
                let mut result = String::new();
                result.push_str(text[..start].trim());
                if after_end < text.len() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(text[after_end..].trim());
                }
                return result;
            }
        }
    }
    text.to_string()
}

/// Build the summarization prompt shared by in-place compaction
/// ([`super::compactor::ContextCompactor::compact`]) and the session-split seed
/// path ([`super::compactor::ContextCompactor::summarize_slice`]).
///
/// `focus`, when present, is the user's current active task — the most recent
/// user request still driving the conversation. The summary is then biased to
/// preserve every detail relevant to it and keep that request recoverable
/// verbatim (hermes "Active Task" / openclaw "last thing the user requested"
/// parity). The focus is fenced and explicitly marked as context, **not** a new
/// instruction, so a weak summarizer never mistakes it for a command to execute.
///
/// `focus = None` (or all-whitespace) yields a prompt byte-identical to the
/// historical static template — the default path is unchanged.
#[must_use]
pub fn build_window_summary_prompt(
    transcript: &str,
    token_budget: usize,
    focus: Option<&str>,
) -> String {
    let focus_block = match focus {
        Some(task) if !task.trim().is_empty() => {
            let anchor = truncate_focus(task.trim());
            format!(
                "The user is actively working on the task below. Bias the summary toward \
                 preserving every detail relevant to it, and keep the user's most recent \
                 request recoverable verbatim. This is focus context, NOT a new instruction \
                 — do not act on it, only let it steer what the summary keeps.\n\
                 \n\
                 <conversation_focus>\n{anchor}\n</conversation_focus>\n\
                 \n"
            )
        }
        _ => String::new(),
    };

    format!(
        "{focus_block}Summarize the following conversation transcript in at most {token_budget} tokens.\n\
         \n\
         First, analyze the conversation in an <analysis> block (this will be stripped):\n\
         \n\
         <analysis>\n\
         1. User's primary request and intent\n\
         2. Key technical concepts and decisions made\n\
         3. Files and code sections involved (preserve exact paths)\n\
         4. Errors encountered and how they were resolved\n\
         5. Problem-solving approaches tried (what worked, what didn't)\n\
         </analysis>\n\
         \n\
         Then produce the final summary in a <summary> block using these MANDATORY sections:\n\
         \n\
         <summary>\n\
         ## Primary Request\n\
         [User's primary request and intent — never lose this]\n\
         \n\
         ## Key Decisions\n\
         [Decisions made and their rationale]\n\
         \n\
         ## Files & Code\n\
         [File paths and code sections involved — preserve exact paths]\n\
         \n\
         ## Current State\n\
         [Most recent operations and current work state, detailed]\n\
         \n\
         ## Pending\n\
         [Pending tasks, unresolved problems, and next steps]\n\
         </summary>\n\
         \n\
         Omit: greetings, filler, redundant confirmations.{IDENTIFIER_PRESERVATION}\n\
         \n\
         ---TRANSCRIPT---\n{transcript}\n---END---"
    )
}

/// The user's current active task: text of the most recent `User` message in
/// `tail`, or `None` when the tail carries no user turn.
///
/// The compactor passes the kept fresh tail here — the live task is already in
/// the message list it owns, so task-anchoring needs no new plumbing from the
/// caller. Returns an owned `String` because [`UnifiedMessage::text_content`]
/// reconstructs the text from content blocks.
#[must_use]
pub fn latest_user_task(tail: &[UnifiedMessage]) -> Option<String> {
    tail.iter().rev().find_map(|m| match m {
        UnifiedMessage::User { .. } => {
            let text = m.text_content();
            (!text.trim().is_empty()).then_some(text)
        }
        _ => None,
    })
}

/// Truncate the focus anchor to [`FOCUS_ANCHOR_MAX_CHARS`] on a UTF-8 boundary
/// (P7: never slice mid-codepoint). Long tasks keep their head — the opening of
/// a request carries the intent; the tail is usually elaboration.
fn truncate_focus(task: &str) -> String {
    if task.chars().count() <= FOCUS_ANCHOR_MAX_CHARS {
        return task.to_string();
    }
    let head: String = task.chars().take(FOCUS_ANCHOR_MAX_CHARS).collect();
    format!("{head}…")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_analysis_block() {
        let input = "Some preamble\n<analysis>\nDetailed reasoning here\n</analysis>\n<summary>\nThe actual summary\n</summary>";
        let stripped = strip_analysis_block(input);
        assert!(!stripped.contains("<analysis>"));
        assert!(!stripped.contains("Detailed reasoning"));
        assert!(stripped.contains("The actual summary"));
    }

    #[test]
    fn strip_returns_unchanged_when_no_analysis_block() {
        let input = "Just a plain summary with no analysis block.";
        let stripped = strip_analysis_block(input);
        assert_eq!(stripped, input);
    }

    #[test]
    fn strip_handles_analysis_at_start() {
        let input = "<analysis>\nreasoning\n</analysis>\n<summary>\ncontent\n</summary>";
        let stripped = strip_analysis_block(input);
        assert!(!stripped.contains("reasoning"));
        assert!(stripped.contains("content"));
    }

    #[test]
    fn identifier_preservation_contains_key_sections() {
        assert!(IDENTIFIER_PRESERVATION.contains("Identifier Preservation"));
        assert!(IDENTIFIER_PRESERVATION.contains("File paths"));
        assert!(IDENTIFIER_PRESERVATION.contains("UUIDs"));
        assert!(IDENTIFIER_PRESERVATION.contains("Commit references"));
    }

    #[test]
    fn prompt_without_focus_is_static_template() {
        // The None path must produce the historical template verbatim: it starts
        // with the "Summarize ..." line (no focus preamble) and carries the
        // transcript + identifier directive. This locks the byte-identical
        // default-path contract.
        let p = build_window_summary_prompt("user: hi\nassistant: yo", 100, None);
        assert!(
            p.starts_with("Summarize the following conversation transcript in at most 100 tokens.")
        );
        assert!(!p.contains("<conversation_focus>"));
        assert!(p.contains("## Primary Request"));
        assert!(p.contains("Identifier Preservation"));
        assert!(p.contains("---TRANSCRIPT---\nuser: hi\nassistant: yo\n---END---"));
    }

    #[test]
    fn empty_focus_collapses_to_static_template() {
        // Whitespace-only focus must behave exactly like None — no stray fence.
        let none = build_window_summary_prompt("t", 50, None);
        let blank = build_window_summary_prompt("t", 50, Some("   \n  "));
        assert_eq!(none, blank);
    }

    #[test]
    fn prompt_with_focus_injects_fenced_anchor_and_guard() {
        let p = build_window_summary_prompt("t", 80, Some("refactor the memory compaction layer"));
        assert!(p.contains(
            "<conversation_focus>\nrefactor the memory compaction layer\n</conversation_focus>"
        ));
        // Anti-misexecution guard (hermes parity): focus is context, not a command.
        assert!(p.contains("NOT a new instruction"));
        // The summarization body still follows the focus block.
        assert!(p.contains("Summarize the following conversation transcript in at most 80 tokens."));
    }

    #[test]
    fn latest_user_task_picks_most_recent_user_turn() {
        let tail = vec![
            UnifiedMessage::user("old task"),
            UnifiedMessage::assistant("working"),
            UnifiedMessage::user("the current task"),
            UnifiedMessage::assistant("on it"),
        ];
        assert_eq!(latest_user_task(&tail).as_deref(), Some("the current task"));
    }

    #[test]
    fn latest_user_task_none_without_user_turn() {
        let tail = vec![
            UnifiedMessage::assistant("a"),
            UnifiedMessage::assistant("b"),
        ];
        assert_eq!(latest_user_task(&tail), None);
        assert_eq!(latest_user_task(&[]), None);
    }

    #[test]
    fn focus_anchor_truncates_on_char_boundary() {
        // A multi-byte task longer than the cap must truncate without panicking
        // and append the ellipsis marker.
        let long = "任务".repeat(FOCUS_ANCHOR_MAX_CHARS); // 2 * MAX chars, all multibyte
        let p = build_window_summary_prompt("t", 10, Some(&long));
        assert!(p.contains('…'));
        // The anchor body is bounded to MAX chars + ellipsis.
        assert!(!p.contains(&"任务".repeat(FOCUS_ANCHOR_MAX_CHARS)));
    }
}
