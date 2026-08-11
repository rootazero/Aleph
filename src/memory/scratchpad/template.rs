// core/src/memory/scratchpad/template.rs

//! Scratchpad Markdown templates
//!
//! There is no user-facing template override. `~/.aleph/prompts.toml` and its
//! `PromptsOverride` loader were removed on 2026-08-08: the only accessor left
//! (`get_template`) had zero call sites, because `ScratchpadManager::initialize`
//! builds its scratchpad from [`generate_scratchpad`]. Restoring an override
//! means wiring it into that constructor in the same commit — a config surface
//! that parses cleanly and changes nothing is worse than no surface at all.

/// Default scratchpad template for new sessions.
///
/// The three sections here are exactly the three a writing surface can reach:
/// `## Objective` (`set_objective` / `set_plan`), `## Plan` (`set_plan` /
/// `start_item` / `complete_item`), `## Notes` (`append_note`). A fourth,
/// `## Working State`, used to be stamped into every scratchpad — no tool
/// action could write it and only the (now withdrawn) `has_content` probe read
/// it, so it was a section the model was shown and could never fill. Existing
/// files that still carry it are unaffected: `section_span` locates sections by
/// header, and an unknown one is simply never addressed.
pub const DEFAULT_TEMPLATE: &str = r#"# Current Task

## Objective
[No active task]

## Plan
- [ ] ...

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
        assert!(DEFAULT_TEMPLATE.contains("## Notes"));
    }

    /// Every section the template ships must have a writing surface. A section
    /// nobody can fill is an affordance the model is shown and cannot use — it
    /// is also how `## Working State` survived as dead structure in every
    /// scratchpad on disk. If a new section is added here, add the action that
    /// writes it in the same commit.
    #[test]
    fn every_shipped_section_has_a_writer() {
        let shipped: Vec<&str> = DEFAULT_TEMPLATE
            .lines()
            .filter(|l| l.starts_with("## "))
            .collect();
        assert_eq!(
            shipped,
            vec!["## Objective", "## Plan", "## Notes"],
            "a section with no `scratchpad` action behind it must not ship"
        );
        // The generated form must agree with the constant, or a brand-new
        // scratchpad and a cleared one would have different shapes.
        let generated: Vec<&str> = generate_scratchpad(Some("x"), "s")
            .lines()
            .filter(|l| l.starts_with("## "))
            .map(|l| match l {
                "## Objective" => "## Objective",
                "## Plan" => "## Plan",
                "## Notes" => "## Notes",
                other => panic!("unexpected generated section {other}"),
            })
            .collect();
        assert_eq!(generated, shipped);
    }

    #[test]
    fn test_generate_scratchpad_with_objective() {
        let result = generate_scratchpad(Some("Build auth module"), "sess-123");
        assert!(result.contains("Build auth module"));
        assert!(result.contains("sess-123"));
    }
}
