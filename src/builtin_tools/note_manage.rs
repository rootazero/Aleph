//! note_manage — unified LLM tool for CRUD operations on all note categories.
//!
//! Replaces `wiki_manage` and extends coverage to all note categories:
//! preference, plan, learning, project, personal, tool, lesson, skill, reference,
//! transcript, other, and the subagent-* family.

use std::path::PathBuf;
use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::{AlephError, Result};
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{sanitize_title, KnowledgeNote, NoteIndexer};
use crate::memory::store::SqliteMemoryBackend;
use crate::tools::AlephTool;

/// Valid note categories (mirrors `CATEGORY_DIRS` in indexer.rs).
const VALID_CATEGORIES: &[&str] = &[
    "preference",
    "plan",
    "learning",
    "project",
    "personal",
    "tool",
    "lesson",
    "skill",
    "reference",
    "transcript",
    "other",
    "subagent-run",
    "subagent-session",
    "subagent-checkpoint",
    "subagent-transcript",
    "contradiction",
];

// =============================================================================
// Args / Output types
// =============================================================================

/// Actions supported by the note_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NoteManageAction {
    /// Create a new note (fails if filename already exists).
    Create,
    /// Replace the body content of an existing note.
    Update,
    /// Append bullet-point facts (and optional links) to an existing or new note.
    Append,
    /// Full-text search across all indexed notes.
    Query,
    /// List all notes, optionally filtered by category.
    List,
    /// Delete a note file and remove it from the index.
    Delete,
}

/// Arguments for the note_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NoteManageArgs {
    /// Action to perform: create, update, append, query, list, delete.
    pub action: NoteManageAction,

    /// Note category: preference, plan, learning, project, personal, tool,
    /// lesson, skill, reference, transcript, other, subagent-run, subagent-session,
    /// subagent-checkpoint, subagent-transcript.
    /// Required for create/update/append/delete; optional filter for list.
    #[serde(default)]
    pub category: Option<String>,

    /// Note filename in kebab-case or title-case, without the `.md` suffix.
    /// Required for create, update, append (as target), delete.
    #[serde(default)]
    pub filename: Option<String>,

    /// Note title displayed in the frontmatter (required for create).
    #[serde(default)]
    pub title: Option<String>,

    /// Markdown body content — the full body text for create/update.
    #[serde(default)]
    pub content: Option<String>,

    /// Bullet-point facts to append (for `append` action).
    #[serde(default)]
    pub facts: Option<Vec<String>>,

    /// Wikilinks to related notes (e.g. ["Rust Learning", "Dev Environment"]).
    #[serde(default)]
    pub links: Option<Vec<String>>,

    /// Tags to attach to the note (used on create).
    #[serde(default)]
    pub tags: Option<Vec<String>>,

    /// Search query text (required for `query` action).
    #[serde(default)]
    pub query: Option<String>,

    /// Maximum number of results for query/list (default: 20).
    #[serde(default)]
    pub limit: Option<usize>,

    /// Agent ID to scope the note operation to. If absent, defaults to "default".
    /// When the calling system prompt declares the active agent's id, prefer
    /// passing it here so the note lands in the caller's per-agent vault rather
    /// than the global "default" namespace.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// A lightweight note entry returned by list/query.
#[derive(Debug, Clone, Serialize)]
pub struct NoteListEntry {
    /// Relative path within the agent directory: "{category}/{filename}".
    pub path: String,
    pub category: String,
    pub filename: String,
    pub tags: Vec<String>,
}

/// Result of a note_manage operation.
#[derive(Debug, Clone, Serialize)]
pub struct NoteManageResult {
    pub success: bool,
    pub message: String,
    /// VFS path of the note affected (create/update/append/delete).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_path: Option<String>,
    /// File content (query action returns matching note bodies).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Notes returned by list/query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<NoteListEntry>>,
}

// =============================================================================
// Tool struct
// =============================================================================

/// Unified tool for managing knowledge notes across all categories.
#[derive(Clone)]
pub struct NoteManageTool {
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
}

impl NoteManageTool {
    pub fn new(memory_dir: PathBuf, store: Arc<SqliteMemoryBackend>) -> Self {
        Self {
            indexer: Arc::new(NoteIndexer::new(memory_dir, store)),
        }
    }

    /// Default agent ID (used when args.agent_id is absent).
    fn agent_id(&self) -> &str {
        "default"
    }

    /// Resolve the effective agent_id for this invocation: prefer args.agent_id,
    /// fall back to the tool's default. This is the only path callers should use
    /// when they need an agent-scoped operation.
    fn resolve_agent_id<'a>(&'a self, args: &'a NoteManageArgs) -> &'a str {
        args.agent_id.as_deref().unwrap_or_else(|| self.agent_id())
    }

    // -------------------------------------------------------------------------
    // Action handlers
    // -------------------------------------------------------------------------

    async fn handle_create(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id = self.resolve_agent_id(args);

        let category = args
            .category
            .as_deref()
            .ok_or_else(|| AlephError::tool("category is required for create"))?;
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for create"))?;
        let _title = args
            .title
            .as_deref()
            .ok_or_else(|| AlephError::tool("title is required for create"))?;

        validate_category(category)?;

        // Ensure directory exists
        let safe_filename = sanitize_title(filename)?;
        let note_dir = self.indexer.memory_dir().join(agent_id).join(category);
        tokio::fs::create_dir_all(&note_dir)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to create category dir: {e}")))?;

        let file_path = note_dir.join(format!("{safe_filename}.md"));
        if file_path.exists() {
            return Err(AlephError::tool(format!(
                "Note '{filename}' in '{category}' already exists. Use 'update' action instead."
            )));
        }

        let tags = args.tags.clone().unwrap_or_default();
        let now = chrono::Utc::now().timestamp();
        let mut note = KnowledgeNote {
            title: safe_filename.clone(),
            category: category.to_string(),
            tags: tags.clone(),
            facts: vec![],
            links: args.links.clone().unwrap_or_default(),
            created_at: now,
            updated_at: now,
            content_hash: String::new(),
            ..Default::default()
        };

        // If content is provided, treat each line starting with "- " as a fact,
        // and the rest as additional markdown body appended below facts.
        // For simplicity, store content as the facts list parsed from the body.
        if let Some(content) = &args.content {
            note.facts = content
                .lines()
                .filter_map(|l| {
                    let t = l.trim();
                    if let Some(rest) = t.strip_prefix("- ") {
                        Some(rest.to_string())
                    } else if !t.is_empty() {
                        // Non-bullet lines stored verbatim as facts
                        Some(t.to_string())
                    } else {
                        None
                    }
                })
                .collect();
        }

        // Atomic write — open with create_new to avoid TOCTOU race
        let md = note.to_markdown();
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file_path)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(md.as_bytes())
            })
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    AlephError::tool(format!(
                        "Note '{filename}' already exists. Use 'update' action instead."
                    ))
                } else {
                    AlephError::tool(format!("Failed to write note: {e}"))
                }
            })?;

        // Index the new file
        self.indexer
            .index_file(agent_id, category, &file_path)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to index note: {e}")))?;

        let note_path = format!("{category}/{safe_filename}");
        info!(path = %note_path, "Note created");

        Ok(NoteManageResult {
            success: true,
            message: format!("Created note '{safe_filename}' in '{category}'"),
            note_path: Some(note_path),
            content: None,
            notes: None,
        })
    }

    async fn handle_update(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id = self.resolve_agent_id(args);

        let category = args
            .category
            .as_deref()
            .ok_or_else(|| AlephError::tool("category is required for update"))?;
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for update"))?;
        let content = args
            .content
            .as_deref()
            .ok_or_else(|| AlephError::tool("content is required for update"))?;

        validate_category(category)?;

        let safe_filename = sanitize_title(filename)?;
        let file_path = self
            .indexer
            .memory_dir()
            .join(agent_id)
            .join(category)
            .join(format!("{safe_filename}.md"));

        if !file_path.exists() {
            return Err(AlephError::tool(format!(
                "Note '{filename}' in '{category}' does not exist. Use 'create' action first."
            )));
        }

        // Read existing note, preserve metadata, replace facts/body
        let existing = tokio::fs::read_to_string(&file_path)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to read note: {e}")))?;

        let mut note = KnowledgeNote::from_markdown(&safe_filename, &existing)
            .map_err(|e| AlephError::tool(format!("Failed to parse existing note: {e}")))?;

        // Replace facts from new content
        note.facts = content
            .lines()
            .filter_map(|l| {
                let t = l.trim();
                if let Some(rest) = t.strip_prefix("- ") {
                    Some(rest.to_string())
                } else if !t.is_empty() {
                    Some(t.to_string())
                } else {
                    None
                }
            })
            .collect();

        // Apply optional field updates
        if let Some(tags) = &args.tags {
            note.tags = tags.clone();
        }
        if let Some(links) = &args.links {
            note.links = links.clone();
        }
        note.updated_at = chrono::Utc::now().timestamp();

        // Write updated file
        let md = note.to_markdown();
        tokio::fs::write(&file_path, &md)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to write note: {e}")))?;

        // Re-index
        self.indexer
            .index_file(agent_id, category, &file_path)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to re-index note: {e}")))?;

        let note_path = format!("{category}/{safe_filename}");
        info!(path = %note_path, "Note updated");

        Ok(NoteManageResult {
            success: true,
            message: format!("Updated note '{safe_filename}' in '{category}'"),
            note_path: Some(note_path),
            content: None,
            notes: None,
        })
    }

    async fn handle_append(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id = self.resolve_agent_id(args);

        let category = args
            .category
            .as_deref()
            .ok_or_else(|| AlephError::tool("category is required for append"))?;
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for append"))?;

        validate_category(category)?;

        let safe_filename = sanitize_title(filename)?;
        let note_path = format!("{category}/{safe_filename}");

        let new_facts = args.facts.clone().unwrap_or_default();
        let new_links = args.links.clone().unwrap_or_default();

        if new_facts.is_empty() && new_links.is_empty() {
            return Err(AlephError::tool(
                "At least one fact or link is required for append",
            ));
        }

        self.indexer
            .append_to_note(agent_id, &note_path, &new_facts, &new_links)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to append to note: {e}")))?;

        info!(path = %note_path, facts = new_facts.len(), "Note appended");

        Ok(NoteManageResult {
            success: true,
            message: format!(
                "Appended {} fact(s) to '{safe_filename}' in '{category}'",
                new_facts.len()
            ),
            note_path: Some(note_path),
            content: None,
            notes: None,
        })
    }

    async fn handle_query(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id = self.resolve_agent_id(args);

        let query = args
            .query
            .as_deref()
            .ok_or_else(|| AlephError::tool("query is required for query action"))?;

        let limit = args.limit.unwrap_or(20);

        let results = self
            .indexer
            .store()
            .search_notes_fts(query, agent_id, limit)
            .await
            .map_err(|e| AlephError::tool(format!("Note search failed: {e}")))?;

        if results.is_empty() {
            return Ok(NoteManageResult {
                success: true,
                message: format!("No notes found matching '{query}'"),
                note_path: None,
                content: None,
                notes: Some(vec![]),
            });
        }

        let mut notes = Vec::new();
        let mut combined_content = String::new();

        for entry in &results {
            let file_path = self
                .indexer
                .memory_dir()
                .join(agent_id)
                .join(&entry.category)
                .join(format!("{}.md", entry.filename));

            let file_content = tokio::fs::read_to_string(&file_path)
                .await
                .unwrap_or_default();

            combined_content.push_str(&format!(
                "## {} ({})\n\n{}\n\n---\n\n",
                entry.filename, entry.path, file_content
            ));

            notes.push(NoteListEntry {
                path: entry.path.clone(),
                category: entry.category.clone(),
                filename: entry.filename.clone(),
                tags: entry.tags.clone(),
            });
        }

        Ok(NoteManageResult {
            success: true,
            message: format!("Found {} note(s) matching '{query}'", notes.len()),
            note_path: None,
            content: Some(combined_content),
            notes: Some(notes),
        })
    }

    async fn handle_list(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id = self.resolve_agent_id(args);
        let limit = args.limit.unwrap_or(100);

        let all_entries = self
            .indexer
            .store()
            .list_notes(agent_id)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to list notes: {e}")))?;

        let entries: Vec<NoteListEntry> = all_entries
            .into_iter()
            .filter(|e| {
                // Optional category filter
                if let Some(cat) = args.category.as_deref() {
                    e.category == cat
                } else {
                    true
                }
            })
            .take(limit)
            .map(|e| NoteListEntry {
                path: e.path.clone(),
                category: e.category.clone(),
                filename: e.filename.clone(),
                tags: e.tags.clone(),
            })
            .collect();

        let category_label = args
            .category
            .as_deref()
            .map(|c| format!(" in '{c}'"))
            .unwrap_or_default();

        Ok(NoteManageResult {
            success: true,
            message: format!("{} note(s){category_label}", entries.len()),
            note_path: None,
            content: None,
            notes: Some(entries),
        })
    }

    async fn handle_delete(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id = self.resolve_agent_id(args);

        let category = args
            .category
            .as_deref()
            .ok_or_else(|| AlephError::tool("category is required for delete"))?;
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for delete"))?;

        validate_category(category)?;

        let safe_filename = sanitize_title(filename)?;
        let file_path = self
            .indexer
            .memory_dir()
            .join(agent_id)
            .join(category)
            .join(format!("{safe_filename}.md"));

        if !file_path.exists() {
            return Err(AlephError::tool(format!(
                "Note '{filename}' in '{category}' does not exist"
            )));
        }

        let note_path = format!("{category}/{safe_filename}");

        // Remove from index first (recoverable), then delete file
        if let Err(e) = self
            .indexer
            .store()
            .remove_note_index(&note_path, agent_id)
            .await
        {
            warn!(path = %note_path, error = %e, "Failed to remove note from index");
        }

        tokio::fs::remove_file(&file_path)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to delete note file: {e}")))?;

        info!(path = %note_path, "Note deleted");

        Ok(NoteManageResult {
            success: true,
            message: format!("Deleted note '{safe_filename}' from '{category}'"),
            note_path: Some(note_path),
            content: None,
            notes: None,
        })
    }
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for NoteManageTool {
    const NAME: &'static str = "note_manage";
    const DESCRIPTION: &'static str =
        "Create, update, append, query, list, or delete personal knowledge notes. \
         Notes are markdown files organized by category (preference, plan, learning, \
         project, personal, tool, lesson, skill, reference, transcript, other). \
         Use this tool to store and retrieve long-term knowledge and preferences.";

    type Args = NoteManageArgs;
    type Output = NoteManageResult;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "note_manage(action='create', category='preference', filename='editor-prefs', title='Editor Preferences', content='- Prefers Vim\\n- Uses LazyVim', tags=['editor'])".to_string(),
            "note_manage(action='update', category='preference', filename='editor-prefs', content='- Prefers Neovim\\n- Uses LazyVim config')".to_string(),
            "note_manage(action='append', category='skill', filename='rust-skills', facts=['Learned async/await patterns'], links=['Tokio'])".to_string(),
            "note_manage(action='query', query='vim editor preferences', limit=5)".to_string(),
            "note_manage(action='list', category='reference')".to_string(),
            "note_manage(action='delete', category='plan', filename='old-plan')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            NoteManageAction::Create => self.handle_create(&args).await,
            NoteManageAction::Update => self.handle_update(&args).await,
            NoteManageAction::Append => self.handle_append(&args).await,
            NoteManageAction::Query => self.handle_query(&args).await,
            NoteManageAction::List => self.handle_list(&args).await,
            NoteManageAction::Delete => self.handle_delete(&args).await,
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Build a category-specific YAML frontmatter block.
///
/// Used by tests to verify template output.
pub fn frontmatter_template(category: &str, title: &str, tags: &[String]) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d").to_string();
    let tags_str = serde_json::to_string(tags).unwrap_or_else(|_| "[]".into());
    match category {
        "reference" => format!(
            "---\ntitle: {title}\naliases: []\ntags: {tags_str}\nsources: []\ncreated: \"{now}\"\nupdated: \"{now}\"\n---"
        ),
        "skill" => format!(
            "---\ntitle: {title}\nscope: persona\ntags: {tags_str}\ncreated: \"{now}\"\nupdated: \"{now}\"\n---"
        ),
        _ => format!(
            "---\ncategory: {category}\ntags: {tags_str}\ncreated: \"{now}\"\nupdated: \"{now}\"\n---"
        ),
    }
}

/// Validate that the category is one of the known valid values.
fn validate_category(category: &str) -> Result<()> {
    if VALID_CATEGORIES.contains(&category) {
        Ok(())
    } else {
        Err(AlephError::tool(format!(
            "Unknown category '{category}'. Valid categories: {}",
            VALID_CATEGORIES.join(", ")
        )))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_category_accepts_contradiction() {
        assert!(validate_category("contradiction").is_ok());
    }

    #[test]
    fn test_frontmatter_wiki_template() {
        let tags = vec!["rust".to_string(), "memory".to_string()];
        let fm = frontmatter_template("reference", "Rust Ownership", &tags);
        assert!(fm.contains("title: Rust Ownership"));
        assert!(fm.contains("aliases: []"));
        assert!(fm.contains("sources: []"));
        assert!(fm.contains("tags:"));
    }

    #[test]
    fn test_frontmatter_skill_template() {
        let tags = vec!["coding".to_string()];
        let fm = frontmatter_template("skill", "Async Rust", &tags);
        assert!(fm.contains("title: Async Rust"));
        assert!(fm.contains("scope: persona"));
        assert!(fm.contains("tags:"));
    }

    #[test]
    fn test_frontmatter_default_template() {
        let tags: Vec<String> = vec![];
        let fm = frontmatter_template("preference", "Editor Config", &tags);
        assert!(fm.contains("category: preference"));
        assert!(fm.contains("tags: []"));
        assert!(fm.contains("created:"));
        assert!(fm.contains("updated:"));
    }

    #[test]
    fn test_valid_category_check() {
        assert!(validate_category("reference").is_ok());
        assert!(validate_category("preference").is_ok());
        assert!(validate_category("subagent-run").is_ok());
        assert!(validate_category("unknown-cat").is_err());
        assert!(validate_category("").is_err());
    }
}
