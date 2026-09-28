//! The ONE mapping from an agent definition file's markdown body to
//! `AgentDef::system_prompt`. Disk agents (`loader.rs`) and plugin agents
//! (`extension::plugin_agent_to_def`) both call it; before this module both
//! dropped the body, and a second mapping would be where they drift.

/// Claude Code's stated ceiling for an agent body. Soft: over it we `warn!`
/// and keep the whole text — an author's prompt is not ours to cut.
pub const SYSTEM_PROMPT_SOFT_CEILING_CHARS: usize = 10_000;

/// Trimmed body, or `None` when there is nothing to inject.
#[must_use]
pub fn body_to_system_prompt(agent_id: &str, body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    let chars = trimmed.chars().count();
    if chars > SYSTEM_PROMPT_SOFT_CEILING_CHARS {
        tracing::warn!(
            agent_id,
            chars,
            ceiling = SYSTEM_PROMPT_SOFT_CEILING_CHARS,
            "agent system prompt exceeds Claude Code's soft ceiling; kept whole"
        );
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_whitespace_bodies_are_none() {
        assert_eq!(body_to_system_prompt("a", ""), None);
        assert_eq!(body_to_system_prompt("a", "  \n\t"), None);
    }

    #[test]
    fn a_body_is_trimmed_and_kept_whole() {
        assert_eq!(
            body_to_system_prompt("a", "\nYou are a reviewer.\n\n## Process\n1. read\n"),
            Some("You are a reviewer.\n\n## Process\n1. read".to_string())
        );
    }

    #[test]
    fn an_oversized_body_is_kept_not_truncated() {
        // CC's 10 000-char ceiling is a soft-fail (warn), not a reject.
        let big = "x".repeat(SYSTEM_PROMPT_SOFT_CEILING_CHARS + 1);
        assert_eq!(
            body_to_system_prompt("a", &big).map(|s| s.len()),
            Some(big.len())
        );
    }
}
