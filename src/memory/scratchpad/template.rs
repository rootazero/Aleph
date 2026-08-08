// core/src/memory/scratchpad/template.rs

//! Scratchpad Markdown templates
//!
//! There is no user-facing template override. `~/.aleph/prompts.toml` and its
//! `PromptsOverride` loader were removed on 2026-08-08: the only accessor left
//! (`get_template`) had zero call sites, because `ScratchpadManager::initialize`
//! builds its scratchpad from [`generate_scratchpad`]. Restoring an override
//! means wiring it into that constructor in the same commit — a config surface
//! that parses cleanly and changes nothing is worse than no surface at all.

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

/// Generate a scratchpad with populated metadata
#[must_use]
pub fn generate_scratchpad(objective: Option<&str>, session_id: &str) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let obj = objective.unwrap_or("[No active task]");

    format!(
        r#"# Current Task

## Objective
{obj}

## Plan
- [ ] ...

## Working State


## Notes


---
_Last updated: {now}_
_Session: {session_id}_
"#
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
