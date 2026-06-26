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

// ASSUMPTION: LLM output contains at most one <analysis>...</analysis> and one
// <summary>...</summary> block, neither nested.
/// Reduce raw LLM summarizer output to the clean summary text that enters the
/// context window, removing the prompt scaffolding the model was told to emit.
///
/// Two scaffolds are stripped (see [`build_window_summary_prompt`]):
/// - the `<analysis>…</analysis>` scratchpad — reasoning space that must never
///   enter context — is removed, keeping any surrounding text;
/// - the `<summary>…</summary>` wrapper — the model is instructed to place its
///   deliverable *inside* it — is unwrapped to its inner content, so stray
///   `<summary>` XML tags do not leak verbatim into every `[Context Summary]`.
///
/// When the output carries no `<summary>` block (e.g. a model that emits bare
/// prose, or a degenerate analysis-only response), the analysis-stripped text is
/// returned unchanged. That preserves the load-bearing contract that an
/// analysis-only response strips to an empty string and routes to deterministic
/// truncation rather than draining the window into an empty summary.
#[must_use]
pub fn strip_analysis_block(text: &str) -> String {
    let without_analysis = strip_tagged_block(text, "analysis");
    unwrap_tagged_block(&without_analysis, "summary").unwrap_or(without_analysis)
}

/// Remove the first `<tag>…</tag>` block, keeping (and trimming) the text on
/// either side. Returns the input unchanged when the block is absent or its
/// closing tag does not follow its opening tag. Tags are ASCII, so every byte
/// offset from `find` lands on a UTF-8 boundary.
fn strip_tagged_block(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = text.find(open.as_str()) {
        if let Some(end) = text.find(close.as_str()) {
            // Only strip when the closing tag appears after the opening tag.
            if end > start {
                let after_end = end + close.len();
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

/// Return the trimmed inner content of the first well-formed `<tag>…</tag>`
/// block, or `None` when no such block exists (so the caller keeps the
/// unwrapped text). An empty block yields `Some("")` — a degenerate summary the
/// caller's emptiness check routes to the truncation fallback.
fn unwrap_tagged_block(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(open.as_str())?;
    let inner_start = start + open.len();
    let end = text.find(close.as_str())?;
    if end < inner_start {
        return None;
    }
    Some(text[inner_start..end].trim().to_string())
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
    let focus_preamble = focus_block(focus);

    format!(
        "{focus_preamble}Summarize the following conversation transcript in at most {token_budget} tokens.\n\
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

/// Render the optional live-task focus preamble shared by the from-scratch
/// ([`build_window_summary_prompt`]) and merge ([`build_merge_summary_prompt`])
/// summarization prompts. Returns an empty string when `focus` is `None` or
/// all-whitespace, so the default path stays byte-identical to the historical
/// static template. The anchor is fenced and explicitly marked as context, not a
/// command (hermes anti-misexecution parity).
fn focus_block(focus: Option<&str>) -> String {
    match focus {
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
    }
}

/// Build the iterative "preserve-and-extend" summarization prompt for merging an
/// existing `[Context Summary]` with newer un-summarized turns.
///
/// Unlike [`build_window_summary_prompt`], which condenses a raw transcript from
/// scratch, this prompt carries `prior_summary` VERBATIM and instructs the model
/// to keep all of it while folding in only `new_transcript`. This is the openclaw
/// `UPDATE_SUMMARIZATION_PROMPT` / hermes iterative-update / pi UPDATE parity:
/// re-running the from-scratch prompt over an already-condensed summary
/// re-compresses it, decaying detail on every merge (summary-of-summary rot).
/// Treating the prior summary as a fixed floor stops that decay.
///
/// `token_budget` bounds the *output*; the caller sizes it to cover the prior
/// summary plus a condensed share of the new turns (never below the prior
/// summary alone — see [`super::compactor::ContextCompactor`]'s cache-extension
/// path). `focus` anchors to the live task exactly as in
/// [`build_window_summary_prompt`]. The `<analysis>`/`<summary>` scaffold is
/// identical, so [`strip_analysis_block`] handles the output unchanged.
#[must_use]
pub fn build_merge_summary_prompt(
    prior_summary: &str,
    new_transcript: &str,
    token_budget: usize,
    focus: Option<&str>,
) -> String {
    let focus_preamble = focus_block(focus);
    format!(
        "{focus_preamble}You are MERGING new conversation turns into an EXISTING context summary.\n\
         \n\
         Rules:\n\
         - PRESERVE every fact, decision, file path, and pending item already in the existing summary below. Do NOT drop or re-compress them.\n\
         - INTEGRATE the new turns: move finished work to the right section, append new decisions, files, and pending items.\n\
         - Keep the SAME section structure as the existing summary.\n\
         - Stay within {token_budget} tokens; if space is tight, condense the NEW turns — never the preserved content.\n\
         \n\
         First reason in an <analysis> block (this will be stripped), then emit the merged result in a <summary> block.{IDENTIFIER_PRESERVATION}\n\
         \n\
         ---EXISTING SUMMARY---\n{prior_summary}\n---END EXISTING SUMMARY---\n\
         \n\
         ---NEW TURNS---\n{new_transcript}\n---END NEW TURNS---"
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
        // Both scaffolds are gone: the analysis scratchpad AND the <summary>
        // wrapper tags. Only the deliverable content survives.
        assert!(!stripped.contains("<summary>"));
        assert!(!stripped.contains("</summary>"));
        assert_eq!(stripped, "The actual summary");
    }

    #[test]
    fn unwraps_summary_block_without_analysis() {
        // A model that wraps its deliverable but emits no analysis block: the
        // <summary> tags must still be stripped so they never leak into context.
        let input = "<summary>\n## Primary Request\nmigrate the store\n</summary>";
        let stripped = strip_analysis_block(input);
        assert_eq!(stripped, "## Primary Request\nmigrate the store");
        assert!(!stripped.contains("summary>"));
    }

    #[test]
    fn analysis_only_response_strips_to_empty() {
        // The load-bearing fallback contract: an analysis-only response (no
        // <summary> block) collapses to an empty string so the caller routes to
        // deterministic truncation instead of draining the window into "".
        let input = "<analysis>\nreasoning only, no summary block\n</analysis>";
        assert!(strip_analysis_block(input).trim().is_empty());
    }

    #[test]
    fn bare_prose_without_summary_block_is_kept() {
        // No <summary> wrapper → analysis-stripped text passes through unchanged,
        // so models that emit plain prose are unaffected.
        let input = "## Primary Request\njust prose, no wrapper";
        assert_eq!(strip_analysis_block(input), input);
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

    #[test]
    fn merge_prompt_carries_prior_summary_verbatim_and_preserve_directive() {
        // The merge prompt must embed the existing summary unchanged and instruct
        // the model to preserve it — the openclaw/hermes/pi iterative-update
        // parity that stops summary-of-summary decay across cache extensions.
        let prior = "## Primary Request\nmigrate the vector store\n## Pending\nwire the gateway";
        let p = build_merge_summary_prompt(prior, "user: also add tests", 500, None);
        assert!(p.contains(prior), "prior summary must ride verbatim");
        assert!(p.contains("PRESERVE every fact"));
        assert!(p.contains("never the preserved content"));
        // New turns and budget are both present.
        assert!(p.contains("user: also add tests"));
        assert!(p.contains("500 tokens"));
        // Same analysis/summary scaffold so strip_analysis_block still applies.
        assert!(p.contains("<analysis>") && p.contains("<summary>"));
        assert!(p.contains("Identifier Preservation"));
    }

    #[test]
    fn merge_prompt_threads_focus_through_shared_block() {
        // Focus anchoring is shared with the from-scratch prompt via focus_block.
        let with = build_merge_summary_prompt("prior", "new", 100, Some("ship the release"));
        assert!(with.contains("<conversation_focus>\nship the release\n</conversation_focus>"));
        assert!(with.contains("NOT a new instruction"));
        // None / blank focus collapses to no preamble (default path unchanged).
        let none = build_merge_summary_prompt("prior", "new", 100, None);
        let blank = build_merge_summary_prompt("prior", "new", 100, Some("   "));
        assert_eq!(none, blank);
        assert!(!none.contains("<conversation_focus>"));
    }
}
