//! SkillManageTool — create, patch, delete, list learned skills.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::context::{
    FactSource, NoteType, MemoryCategory, MemoryFact, MemoryLayer,
};
use crate::skill::{is_valid_category, is_valid_skill_name};

/// Actions supported by the skill_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SkillAction {
    Create,
    Patch,
    Delete,
    List,
}

/// Arguments for the skill_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillManageArgs {
    /// Action to perform.
    pub action: SkillAction,
    /// Skill name (kebab-case). Required for create, patch, delete.
    pub name: Option<String>,
    /// Skill category. Required for create.
    pub category: Option<String>,
    /// Skill scope: "global" or "persona" (default: persona).
    pub scope: Option<String>,
    /// One-line description. Required for create.
    pub description: Option<String>,
    /// Full skill markdown content. Required for create.
    pub content: Option<String>,
    /// Old text to find (for patch action).
    pub old_text: Option<String>,
    /// New text to replace with (for patch action).
    pub new_text: Option<String>,
}

/// Result of a skill_manage operation.
#[derive(Debug, Clone, Serialize)]
pub struct SkillManageResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillListEntry>>,
}

/// Entry in skill list output.
#[derive(Debug, Clone, Serialize)]
pub struct SkillListEntry {
    pub name: String,
    pub category: String,
    pub description: String,
    pub path: String,
}

/// Validate arguments for create action.
pub fn validate_args_for_create(
    name: &str,
    category: &str,
    description: &str,
    content: &str,
) -> Result<(), String> {
    if !is_valid_skill_name(name) {
        return Err(format!(
            "Invalid skill name '{}': must be non-empty kebab-case (lowercase ASCII + hyphens)",
            name
        ));
    }
    if !is_valid_category(category) {
        return Err(format!(
            "Invalid category '{}': must be one of: coding, debugging, workflow, knowledge, communication",
            category
        ));
    }
    if description.is_empty() {
        return Err("Description cannot be empty".to_string());
    }
    if content.is_empty() {
        return Err("Content cannot be empty".to_string());
    }
    Ok(())
}

/// Build a MemoryFact from skill creation arguments.
pub fn build_skill_fact(
    name: &str,
    category: &str,
    description: &str,
    content: &str,
) -> MemoryFact {
    let path = format!("aleph://skills/{}/{}/", category, name);
    let full_content = format!("{}\n\n{}", description, content);

    MemoryFact::new(full_content, NoteType::Skill, Vec::new())
        .with_layer(MemoryLayer::L1Overview)
        .with_category(MemoryCategory::Patterns)
        .with_path(path)
        .with_fact_source(FactSource::Extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_skill_name() {
        assert!(validate_args_for_create("rust-debug", "coding", "desc", "content").is_ok());
        assert!(validate_args_for_create("", "coding", "desc", "content").is_err());
        assert!(validate_args_for_create("Has Spaces", "coding", "desc", "content").is_err());
    }

    #[test]
    fn validates_category() {
        assert!(validate_args_for_create("name", "coding", "desc", "content").is_ok());
        assert!(validate_args_for_create("name", "invalid", "desc", "content").is_err());
    }

    #[test]
    fn builds_fact_from_args() {
        let fact = build_skill_fact(
            "rust-debug",
            "coding",
            "Debug Rust errors",
            "# Steps\n1. Read error",
        );
        assert_eq!(fact.note_type, NoteType::Skill);
        assert_eq!(fact.path, "aleph://skills/coding/rust-debug/");
        assert!(fact.content.contains("Debug Rust errors"));
        assert!(fact.content.contains("Read error"));
    }
}
