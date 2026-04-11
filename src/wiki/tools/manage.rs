//! Wiki fact builder and validation for wiki_manage tool arguments.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::context::{
    FactSource, NoteType, MemoryCategory, MemoryFact, MemoryLayer, MemoryScope, MemoryTier,
};
use crate::wiki::is_valid_page_slug;

/// Actions supported by the wiki_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WikiAction {
    Create,
    Update,
    Query,
    Delete,
    List,
}

/// Arguments for the wiki_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WikiManageArgs {
    /// Action to perform.
    pub action: WikiAction,
    /// Page slug (kebab-case). Required for create, update, delete.
    #[serde(default)]
    pub page_slug: Option<String>,
    /// Page title. Required for create.
    #[serde(default)]
    pub title: Option<String>,
    /// One-line summary of the page. Required for create.
    #[serde(default)]
    pub summary: Option<String>,
    /// Full markdown content of the page. Required for create, optional for update.
    #[serde(default)]
    pub content: Option<String>,
    /// Search query string. Required for query action.
    #[serde(default)]
    pub query: Option<String>,
}

/// Result of a wiki_manage operation.
#[derive(Debug, Clone, Serialize)]
pub struct WikiManageResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<WikiListEntry>>,
}

/// Entry in wiki page list output.
#[derive(Debug, Clone, Serialize)]
pub struct WikiListEntry {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub path: String,
}

/// Validate arguments for create action.
pub fn validate_args_for_create(
    page_slug: &str,
    title: &str,
    summary: &str,
    content: &str,
) -> Result<(), String> {
    if !is_valid_page_slug(page_slug) {
        return Err(format!(
            "Invalid page slug '{}': must be non-empty kebab-case (lowercase ASCII, hyphens, digits)",
            page_slug
        ));
    }
    if title.is_empty() {
        return Err("Title cannot be empty".to_string());
    }
    if summary.is_empty() {
        return Err("Summary cannot be empty".to_string());
    }
    if content.is_empty() {
        return Err("Content cannot be empty".to_string());
    }
    Ok(())
}

/// Build a MemoryFact anchor for a wiki page.
pub fn build_wiki_fact(agent_id: &str, page_slug: &str, summary: &str) -> MemoryFact {
    let path = crate::wiki::wiki_path(agent_id, page_slug);

    MemoryFact::new(summary.to_string(), NoteType::Wiki, Vec::new())
        .with_confidence(0.9)
        .with_tier(MemoryTier::LongTerm)
        .with_scope(MemoryScope::Global)
        .with_layer(MemoryLayer::L2Detail)
        .with_category(MemoryCategory::Patterns)
        .with_path(path)
        .with_fact_source(FactSource::Document)
        .with_agent(agent_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_page_slug() {
        assert!(validate_args_for_create("rust-ownership", "Title", "Summary", "Content").is_ok());
        assert!(validate_args_for_create("", "Title", "Summary", "Content").is_err());
        assert!(validate_args_for_create("Has Spaces", "Title", "Summary", "Content").is_err());
    }

    #[test]
    fn validates_required_fields() {
        assert!(validate_args_for_create("slug", "", "Summary", "Content").is_err());
        assert!(validate_args_for_create("slug", "Title", "", "Content").is_err());
        assert!(validate_args_for_create("slug", "Title", "Summary", "").is_err());
    }

    #[test]
    fn builds_wiki_fact_correctly() {
        let fact = build_wiki_fact("default", "rust-ownership", "Rust ownership and borrowing rules");
        assert_eq!(fact.note_type, NoteType::Wiki);
        assert_eq!(fact.path, "aleph://wiki/default/rust-ownership.md");
        assert_eq!(fact.tier, MemoryTier::LongTerm);
        assert_eq!(fact.scope, MemoryScope::Global);
        assert_eq!(fact.agent, "default");
        assert!(fact.content.contains("Rust ownership"));
        assert!((fact.confidence - 0.9).abs() < f32::EPSILON);
    }
}
