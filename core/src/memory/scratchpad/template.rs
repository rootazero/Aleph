// core/src/memory/scratchpad/template.rs

//! Scratchpad Markdown templates

/// Default scratchpad template for new sessions
pub const DEFAULT_TEMPLATE: &str = r#"# Current Task

## Objective
[No active task]

## Plan
- [ ] ...

## Working State


## Notes


---
_Last updated: _
_Session: _
"#;

/// Get the scratchpad template, checking override first
pub fn get_template(
    overrides: &crate::config::prompts_override::PromptsOverride,
) -> &str {
    overrides
        .scratchpad_template()
        .unwrap_or(DEFAULT_TEMPLATE)
}

/// Generate a scratchpad with populated metadata
pub fn generate_scratchpad(objective: Option<&str>, session_id: &str) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let obj = objective.unwrap_or("[No active task]");

    format!(
        r#"# Current Task

## Objective
{}

## Plan
- [ ] ...

## Working State


## Notes


---
_Last updated: {}_
_Session: {}_
"#,
        obj, now, session_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_template_has_sections() {
        assert!(DEFAULT_TEMPLATE.contains("## Objective"));
        assert!(DEFAULT_TEMPLATE.contains("## Plan"));
        assert!(DEFAULT_TEMPLATE.contains("## Working State"));
        assert!(DEFAULT_TEMPLATE.contains("## Notes"));
    }

    #[test]
    fn test_generate_scratchpad_with_objective() {
        let result = generate_scratchpad(Some("Build auth module"), "sess-123");
        assert!(result.contains("Build auth module"));
        assert!(result.contains("sess-123"));
    }
}
