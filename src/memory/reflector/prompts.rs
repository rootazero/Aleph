//! Synthesis prompt for MemoryReflector.

pub const PROMPT_SYNTHESIS: &str = include_str!("prompts/snapshots/synthesis.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_nonempty_and_json_directed() {
        assert!(
            PROMPT_SYNTHESIS.len() > 200,
            "synthesis prompt suspiciously short: {} bytes",
            PROMPT_SYNTHESIS.len()
        );
        assert!(PROMPT_SYNTHESIS.contains("JSON"));
        assert!(PROMPT_SYNTHESIS.contains("sources"));
        assert!(PROMPT_SYNTHESIS.contains("relevance"));
    }

    #[test]
    fn prompt_enforces_no_hallucination() {
        assert!(
            PROMPT_SYNTHESIS.contains("Do not invent")
                || PROMPT_SYNTHESIS.contains("Do not hallucinate"),
            "prompt must instruct no invented facts"
        );
    }
}
