//! Reflection prompt template for session-end LLM call.

/// Build the reflection system prompt.
pub fn reflection_system_prompt() -> &'static str {
    r#"You are a reflection engine. Analyze the conversation and extract structured insights.

Output EXACTLY this markdown format:

## Invariants
- {Durable user preferences, work patterns, identity traits that will hold across sessions}

## Derived
- {New information learned THIS session — temporary context, current task details}

## Lessons
- {symptom}: {root cause} → {fix or prevention strategy}

## Skills
- {skill name}: {concise reusable steps or key insight}

## Open Loops
- {Follow-up actions with action verbs: investigate, verify, update, test, check}

Rules:
1. Write in third person ("The user prefers..." not "You prefer...")
2. Be specific and concrete — avoid vague statements
3. Invariants must be TRUE ACROSS SESSIONS, not session-specific
4. Lessons MUST have the symptom: cause → fix format
5. Skills: only include if the approach is non-trivial (5+ steps or non-obvious) and likely to recur
6. Open Loops MUST start with an action verb
7. If a section has no items, write: - (none)
8. Do NOT repeat facts that are in the ALREADY EXTRACTED list below"#
}

/// Build the reflection user prompt with conversation context.
pub fn reflection_user_prompt(
    conversation_summary: &str,
    already_extracted_facts: &[String],
) -> String {
    let facts_section = if already_extracted_facts.is_empty() {
        "No facts extracted yet.".to_string()
    } else {
        already_extracted_facts
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{}. {}", i + 1, f))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "## ALREADY EXTRACTED (do not repeat)\n{}\n\n## CONVERSATION TO REFLECT ON\n{}",
        facts_section, conversation_summary
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_contains_all_sections() {
        let prompt = reflection_system_prompt();
        assert!(prompt.contains("## Invariants"));
        assert!(prompt.contains("## Derived"));
        assert!(prompt.contains("## Lessons"));
        assert!(prompt.contains("## Skills"));
        assert!(prompt.contains("## Open Loops"));
    }

    #[test]
    fn user_prompt_includes_facts() {
        let facts = vec![
            "User prefers dark mode".to_string(),
            "User works in Rust".to_string(),
        ];
        let result = reflection_user_prompt("some conversation", &facts);
        assert!(result.contains("1. User prefers dark mode"));
        assert!(result.contains("2. User works in Rust"));
        assert!(result.contains("some conversation"));
    }

    #[test]
    fn user_prompt_handles_empty_facts() {
        let result = reflection_user_prompt("some conversation", &[]);
        assert!(result.contains("No facts extracted yet."));
    }
}
