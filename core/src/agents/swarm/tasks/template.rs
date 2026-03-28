//! Task template parsing and instantiation (Task 5)
//!
//! Provides types and functions for loading team templates from Markdown files
//! with YAML frontmatter. Templates define a team's members, task graph, and
//! the Markdown body used to construct each agent's system prompt.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::error::{AlephError, Result};

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Full team template deserialized from YAML frontmatter.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamTemplate {
    pub name: String,
    pub description: String,
    /// Name of the member who is the team leader.
    pub leader: String,
    pub members: Vec<TemplateMember>,
    #[serde(default)]
    pub tasks: Vec<TemplateTask>,
    #[serde(default)]
    pub variables: Vec<TemplateVariable>,
}

/// A member entry inside a team template.
#[derive(Debug, Clone, Deserialize)]
pub struct TemplateMember {
    pub name: String,
    pub role: String,
    #[serde(default)]
    pub model: Option<String>,
}

/// A task entry inside a team template.
#[derive(Debug, Clone, Deserialize)]
pub struct TemplateTask {
    /// Short unique key used for `blocked_by` cross-references.
    pub key: String,
    pub subject: String,
    /// Name of the member who owns this task.
    pub owner: String,
    #[serde(default)]
    pub priority: Option<String>,
    /// Keys of tasks that must complete before this task can start.
    #[serde(default)]
    pub blocked_by: Vec<String>,
}

/// A variable declaration inside a team template.
#[derive(Debug, Clone, Deserialize)]
pub struct TemplateVariable {
    pub name: String,
    pub description: String,
}

/// Result of parsing a template file: YAML frontmatter + Markdown body.
#[derive(Debug)]
pub struct ParsedTemplate {
    pub frontmatter: TeamTemplate,
    /// Markdown body that follows the closing `---` fence.
    pub body: String,
}

// ---------------------------------------------------------------------------
// parse_template
// ---------------------------------------------------------------------------

/// Parse a Markdown+YAML frontmatter template file.
///
/// The file must begin with `---`, followed by YAML, then another `---` line.
/// Everything after the second `---` is the Markdown body.
pub fn parse_template(content: &str) -> Result<ParsedTemplate> {
    let content = content.trim_start();

    if !content.starts_with("---") {
        return Err(AlephError::Other {
            message: "Template must start with YAML frontmatter (---)".to_string(),
            suggestion: Some("Add --- at the beginning of the file".to_string()),
        });
    }

    // Skip the opening `---` (3 bytes, always ASCII)
    let after_open = &content[3..];

    // Find the closing `---` delimiter on its own line
    let end_pos = after_open.find("\n---").ok_or_else(|| AlephError::Other {
        message: "Template frontmatter must be closed with --- on a new line".to_string(),
        suggestion: None,
    })?;

    let yaml_str = after_open[..end_pos].trim();

    // Body starts after the closing `---` line (skip "\n---" plus optional "\n")
    let after_close = &after_open[end_pos + 4..]; // skip "\n---"
    let body = after_close.strip_prefix('\n').unwrap_or(after_close).to_string();

    let frontmatter: TeamTemplate = serde_yaml::from_str(yaml_str).map_err(|e| AlephError::Other {
        message: format!("Failed to parse template YAML frontmatter: {}", e),
        suggestion: Some("Check the frontmatter for YAML syntax errors".to_string()),
    })?;

    Ok(ParsedTemplate { frontmatter, body })
}

// ---------------------------------------------------------------------------
// substitute_variables
// ---------------------------------------------------------------------------

/// Replace `{key}` placeholders in `text` with values from `variables`.
///
/// The caller is responsible for supplying built-in variables such as
/// `agent_name`, `agent_role`, `team_name`, and `leader_name` alongside any
/// user-defined variables.
pub fn substitute_variables(text: &str, variables: &HashMap<String, String>) -> String {
    let mut result = text.to_string();
    // Sort keys for deterministic replacement order (HashMap iteration is non-deterministic)
    let mut sorted_vars: Vec<(&String, &String)> = variables.iter().collect();
    sorted_vars.sort_by_key(|(k, _)| k.as_str());
    for (key, value) in sorted_vars {
        let placeholder = format!("{{{}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
}

// ---------------------------------------------------------------------------
// find_template
// ---------------------------------------------------------------------------

/// Locate and read a template file by name.
///
/// Search order:
/// 1. `~/.aleph/templates/{name}.md`
/// 2. Namespaced `plugin:template` — not yet implemented (returns an error).
pub fn find_template(name: &str) -> Result<String> {
    // Reject plugin-namespaced names for now
    if name.contains(':') {
        return Err(AlephError::Other {
            message: format!(
                "Plugin-namespaced template '{}' is not yet supported",
                name
            ),
            suggestion: Some(
                "Use a plain template name from ~/.aleph/templates/".to_string(),
            ),
        });
    }

    let home = dirs::home_dir().ok_or_else(|| AlephError::Other {
        message: "Cannot determine home directory".to_string(),
        suggestion: None,
    })?;

    let path = home.join(".aleph").join("templates").join(format!("{}.md", name));

    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| {
            AlephError::IoError(format!("Failed to read template '{}': {}", name, e))
        })
    } else {
        Err(AlephError::NotFound(format!(
            "Template '{}' not found at {}",
            name,
            path.display()
        )))
    }
}

// ---------------------------------------------------------------------------
// validate_template
// ---------------------------------------------------------------------------

/// Validate a `TeamTemplate` for referential integrity.
///
/// Checks:
/// - Leader name exists in the members list.
/// - All task owners exist in the members list.
/// - All `blocked_by` keys reference existing task keys.
/// - No duplicate task keys.
pub fn validate_template(template: &TeamTemplate) -> Result<()> {
    let member_names: HashSet<&str> = template.members.iter().map(|m| m.name.as_str()).collect();

    // Leader must be a member
    if !member_names.contains(template.leader.as_str()) {
        return Err(AlephError::Other {
            message: format!(
                "Template leader '{}' is not listed in members",
                template.leader
            ),
            suggestion: Some("Add the leader as a member or correct the leader field".to_string()),
        });
    }

    // Collect task keys and check for duplicates
    let mut task_keys: HashSet<&str> = HashSet::new();
    for task in &template.tasks {
        if !task_keys.insert(task.key.as_str()) {
            return Err(AlephError::Other {
                message: format!("Duplicate task key '{}'", task.key),
                suggestion: Some("Each task key must be unique within the template".to_string()),
            });
        }
    }

    // Validate task owners and blocked_by references
    for task in &template.tasks {
        if !member_names.contains(task.owner.as_str()) {
            return Err(AlephError::Other {
                message: format!(
                    "Task '{}' owner '{}' is not listed in members",
                    task.key, task.owner
                ),
                suggestion: None,
            });
        }

        for dep_key in &task.blocked_by {
            if !task_keys.contains(dep_key.as_str()) {
                return Err(AlephError::Other {
                    message: format!(
                        "Task '{}' blocked_by references nonexistent key '{}'",
                        task.key, dep_key
                    ),
                    suggestion: Some(
                        "Check that the referenced task key is defined in the tasks list"
                            .to_string(),
                    ),
                });
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_TEMPLATE: &str = r#"---
name: code-review-team
description: Code review team with architect + reviewers + summarizer
leader: architect
members:
  - name: architect
    role: Architect, assigns review tasks and makes final decisions
    model: claude-sonnet-4-20250514
  - name: reviewer-1
    role: Code reviewer, focuses on logic correctness and security
  - name: summarizer
    role: Summarizes findings into a final report
tasks:
  - key: analyze
    subject: Analyze code change scope
    owner: architect
    priority: high
  - key: review-logic
    subject: Review logic and security
    owner: reviewer-1
    blocked_by: [analyze]
  - key: consolidate
    subject: Consolidate review report
    owner: summarizer
    blocked_by: [review-logic]
variables:
  - name: goal
    description: Review target
---

# Code Review Team

You are {agent_name}, {agent_role}.

## Team Goal

{goal}
"#;

    #[test]
    fn test_parse_template() {
        let parsed = parse_template(EXAMPLE_TEMPLATE).expect("should parse without error");
        let fm = &parsed.frontmatter;

        assert_eq!(fm.name, "code-review-team");
        assert_eq!(fm.description, "Code review team with architect + reviewers + summarizer");
        assert_eq!(fm.leader, "architect");

        assert_eq!(fm.members.len(), 3);
        assert_eq!(fm.members[0].name, "architect");
        assert_eq!(fm.members[0].role, "Architect, assigns review tasks and makes final decisions");
        assert_eq!(fm.members[0].model.as_deref(), Some("claude-sonnet-4-20250514"));
        assert_eq!(fm.members[1].name, "reviewer-1");
        assert!(fm.members[1].model.is_none());

        assert_eq!(fm.tasks.len(), 3);
        assert_eq!(fm.tasks[0].key, "analyze");
        assert_eq!(fm.tasks[0].owner, "architect");
        assert_eq!(fm.tasks[0].priority.as_deref(), Some("high"));
        assert!(fm.tasks[0].blocked_by.is_empty());

        assert_eq!(fm.tasks[1].key, "review-logic");
        assert_eq!(fm.tasks[1].blocked_by, vec!["analyze"]);

        assert_eq!(fm.tasks[2].key, "consolidate");
        assert_eq!(fm.tasks[2].blocked_by, vec!["review-logic"]);

        assert_eq!(fm.variables.len(), 1);
        assert_eq!(fm.variables[0].name, "goal");
        assert_eq!(fm.variables[0].description, "Review target");

        // Body should contain the Markdown content
        assert!(parsed.body.contains("You are {agent_name}"));
        assert!(parsed.body.contains("{goal}"));
    }

    #[test]
    fn test_substitute_variables() {
        let text = "You are {agent_name}, {agent_role}. Goal: {goal}.";
        let mut vars = HashMap::new();
        vars.insert("agent_name".to_string(), "Alice".to_string());
        vars.insert("agent_role".to_string(), "Architect".to_string());
        vars.insert("goal".to_string(), "Review the PR".to_string());

        let result = substitute_variables(text, &vars);
        assert_eq!(result, "You are Alice, Architect. Goal: Review the PR.");
    }

    #[test]
    fn test_substitute_variables_unknown_placeholder_unchanged() {
        let text = "Hello {unknown}!";
        let vars = HashMap::new();
        let result = substitute_variables(text, &vars);
        assert_eq!(result, "Hello {unknown}!");
    }

    #[test]
    fn test_validate_template_valid() {
        let parsed = parse_template(EXAMPLE_TEMPLATE).unwrap();
        validate_template(&parsed.frontmatter).expect("valid template should pass validation");
    }

    #[test]
    fn test_validate_template_invalid_leader() {
        let parsed = parse_template(EXAMPLE_TEMPLATE).unwrap();
        let mut tmpl = parsed.frontmatter;
        tmpl.leader = "nonexistent-leader".to_string();

        let err = validate_template(&tmpl).expect_err("should fail with invalid leader");
        assert!(
            err.to_string().contains("nonexistent-leader"),
            "error message should mention the bad leader name: {}",
            err
        );
    }

    #[test]
    fn test_validate_template_invalid_blocked_by() {
        let parsed = parse_template(EXAMPLE_TEMPLATE).unwrap();
        let mut tmpl = parsed.frontmatter;
        // Make a task block on a key that doesn't exist
        tmpl.tasks[1].blocked_by = vec!["ghost-task".to_string()];

        let err =
            validate_template(&tmpl).expect_err("should fail with invalid blocked_by reference");
        assert!(
            err.to_string().contains("ghost-task"),
            "error message should mention the bad key: {}",
            err
        );
    }

    #[test]
    fn test_validate_template_duplicate_task_key() {
        let parsed = parse_template(EXAMPLE_TEMPLATE).unwrap();
        let mut tmpl = parsed.frontmatter;
        // Duplicate the first task key
        let dup = tmpl.tasks[0].clone();
        tmpl.tasks.push(dup);

        let err = validate_template(&tmpl).expect_err("should fail with duplicate task key");
        assert!(err.to_string().contains("analyze"), "error should name the duplicate: {}", err);
    }

    #[test]
    fn test_validate_template_invalid_task_owner() {
        let parsed = parse_template(EXAMPLE_TEMPLATE).unwrap();
        let mut tmpl = parsed.frontmatter;
        tmpl.tasks[0].owner = "ghost-member".to_string();

        let err = validate_template(&tmpl).expect_err("should fail with unknown task owner");
        assert!(
            err.to_string().contains("ghost-member"),
            "error should name the bad owner: {}",
            err
        );
    }

    #[test]
    fn test_parse_template_missing_frontmatter() {
        let content = "No frontmatter here.\n\nJust body text.";
        let err = parse_template(content).expect_err("should fail without frontmatter");
        assert!(err.to_string().contains("frontmatter"));
    }
}
