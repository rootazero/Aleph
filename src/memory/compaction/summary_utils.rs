//! Shared summary utilities for compaction and session compactor.
//!
//! Provides the identifier preservation directive and the analysis-block
//! stripping helper used by both [`super::context_compactor`] and
//! [`crate::memory::session_compactor::summary_engine`].

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
- Version numbers (e.g., v2026.04.02)\n\
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
pub fn strip_analysis_block(text: &str) -> String {
    if let Some(start) = text.find("<analysis>") {
        if let Some(end) = text.find("</analysis>") {
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
    text.to_string()
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
}
