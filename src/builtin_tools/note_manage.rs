//! `note_manage` — unified LLM tool for CRUD operations on all note categories.
//!
//! Replaces `wiki_manage` and extends coverage to all note categories:
//! preference, plan, learning, project, personal, tool, lesson, skill, reference,
//! transcript, other, and the subagent-* family.

use crate::sync_primitives::Arc;
use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use crate::error::{AlephError, Result};
use crate::memory::context::NoteType;
use crate::memory::events::handler::MemoryCommandHandler;
use crate::memory::events::EventActor;
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

/// Actions supported by the `note_manage` tool.
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
    /// Read materialized graph-health insights (knowledge gaps, bridges,
    /// surprising connections). Read-only.
    Insights,
    /// Read the memory-evolution gate state: recent dream cycles' health
    /// score (before/after), best-ever score, accepted/rejected verdict,
    /// merges the gate rejected, and any churn-pathology cooldown. Lets the
    /// model explain *why* memory changed (or didn't) last night. Read-only.
    Evolution,
}

/// Arguments for the `note_manage` tool.
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

/// Result of a `note_manage` operation.
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
    /// Related existing notes surfaced after a create, so the model can
    /// weave the new note into the wiki (via `links`) instead of leaving an
    /// orphan island.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_notes: Option<Vec<NoteListEntry>>,
}

// =============================================================================
// Tool struct
// =============================================================================

/// Unified tool for managing knowledge notes across all categories.
#[derive(Clone)]
pub struct NoteManageTool {
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
    /// Optional event-sourcing handler. When present, note create/update/
    /// delete actions append a lifecycle event to the per-note event log so
    /// the `memory_timeline` tool can show a note's history. `None` is a
    /// graceful no-op.
    command_handler: Option<Arc<MemoryCommandHandler>>,
    /// When true, notes written/read by this tool are partitioned by the
    /// active project directory (Claude-Code-style workspaces) via
    /// [`crate::memory::project_scope`]. Mirrors `MemoryConfig.project_scoped`;
    /// `false` (the default) is byte-for-byte the single-namespace behaviour.
    project_scoped: bool,
}

impl NoteManageTool {
    pub fn new(memory_dir: PathBuf, store: Arc<SqliteMemoryBackend>) -> Self {
        Self {
            indexer: Arc::new(NoteIndexer::new(memory_dir, store)),
            command_handler: None,
            project_scoped: false,
        }
    }

    /// Enable per-project memory namespacing for this tool. Wired from
    /// `MemoryConfig.project_scoped` at construction; default-off otherwise.
    #[must_use]
    pub const fn with_project_scoping(mut self, enabled: bool) -> Self {
        self.project_scoped = enabled;
        self
    }

    /// Attach an event-sourcing handler so note mutations are recorded in the
    /// per-note event log that the `memory_timeline` tool reads.
    #[must_use]
    pub fn with_command_handler(mut self, handler: Arc<MemoryCommandHandler>) -> Self {
        self.command_handler = Some(handler);
        self
    }

    /// Append a lifecycle event for a completed write action to the per-note
    /// event log. Best-effort: the note write has already succeeded, so a
    /// failure here is logged and swallowed rather than surfaced to the LLM.
    async fn record_lifecycle_event(&self, args: &NoteManageArgs, result: &NoteManageResult) {
        let Some(handler) = &self.command_handler else {
            return;
        };
        let Some(note_path) = result.note_path.as_deref() else {
            return;
        };
        let agent = self.resolve_agent_id(args);
        let outcome = match &args.action {
            NoteManageAction::Create => {
                let note_type = args
                    .category
                    .as_deref()
                    .map(NoteType::from_str_or_other)
                    .unwrap_or_default();
                handler
                    .log_note_created(
                        note_path,
                        args.content.clone().unwrap_or_default(),
                        agent,
                        note_type,
                        EventActor::Agent,
                    )
                    .await
            }
            NoteManageAction::Update => {
                handler
                    .log_note_updated(
                        note_path,
                        args.content.clone().unwrap_or_default(),
                        "note_manage update".to_string(),
                        EventActor::Agent,
                    )
                    .await
            }
            NoteManageAction::Append => {
                let appended = args.facts.clone().unwrap_or_default().join("\n");
                handler
                    .log_note_updated(
                        note_path,
                        appended,
                        "note_manage append".to_string(),
                        EventActor::Agent,
                    )
                    .await
            }
            NoteManageAction::Delete => {
                handler
                    .log_note_deleted(
                        note_path,
                        "note_manage delete".to_string(),
                        EventActor::Agent,
                    )
                    .await
            }
            NoteManageAction::Query
            | NoteManageAction::List
            | NoteManageAction::Insights
            | NoteManageAction::Evolution => return,
        };
        if let Err(e) = outcome {
            warn!(path = %note_path, error = %e, "note_manage: failed to record lifecycle event");
        }
    }

    /// Default agent ID (used when `args.agent_id` is absent).
    const fn agent_id(&self) -> &str {
        "default"
    }

    /// Resolve the effective `agent_id` (storage partition key) for this
    /// invocation: prefer `args.agent_id`, fall back to the tool's default.
    ///
    /// When `project_scoped` is enabled and a project root is active for the
    /// run, the base id is composed with the project namespace so notes are
    /// isolated per project directory (the existing `note/{agent_id}/…` layout
    /// + `(agent_id, …)` table keys do the partitioning, no schema change).
    ///   Outside a project — or with the feature off — the base id is returned
    ///   unchanged. This is the only path callers should use when they need an
    ///   agent-scoped operation.
    fn resolve_agent_id(&self, args: &NoteManageArgs) -> String {
        let base = args.agent_id.as_deref().unwrap_or_else(|| self.agent_id());
        crate::memory::project_scope::scoped_or_base(
            base,
            self.project_scoped,
            crate::projects::current_project_root().as_deref(),
        )
    }

    // -------------------------------------------------------------------------
    // Action handlers
    // -------------------------------------------------------------------------

    async fn handle_create(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args);
        let agent_id = agent_id_owned.as_str();

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
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file_path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    AlephError::tool(format!(
                        "Note '{filename}' already exists. Use 'update' action instead."
                    ))
                } else {
                    AlephError::tool(format!("Failed to write note: {e}"))
                }
            })?;
        file.write_all(md.as_bytes())
            .await
            .map_err(|e| AlephError::tool(format!("Failed to write note: {e}")))?;

        // Index the new file
        self.indexer
            .index_file(agent_id, category, &file_path)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to index note: {e}")))?;

        let note_path = format!("{category}/{safe_filename}");
        info!(path = %note_path, "Note created");

        // Surface related existing notes (best-effort, FTS-only — this tool
        // has no embedder) so the model can weave the new note into the wiki
        // instead of leaving an orphan island. `search_notes_fts` treats its
        // whole query as ONE FTS5 phrase, so a multi-word title+content blob
        // would require an exact phrase hit and never match — search per
        // significant keyword instead and merge by path. Search failure must
        // never fail the create.
        let query_text = format!(
            "{} {}",
            args.title.as_deref().unwrap_or(&safe_filename),
            args.content.as_deref().unwrap_or("")
        );
        let mut rel: Vec<NoteListEntry> = Vec::new();
        for kw in related_keywords(&query_text) {
            match self
                .indexer
                .store()
                .search_notes_fts(&kw, agent_id, 3)
                .await
            {
                Ok(hits) => {
                    for e in hits {
                        if e.path == note_path || rel.iter().any(|r| r.path == e.path) {
                            continue;
                        }
                        rel.push(NoteListEntry {
                            path: e.path,
                            category: e.category,
                            filename: e.filename,
                            tags: e.tags,
                        });
                    }
                }
                Err(e) => {
                    warn!(error = %e, keyword = %kw, "note_manage create: related-note search failed");
                }
            }
            if rel.len() >= 5 {
                break;
            }
        }
        rel.truncate(5);
        let related_notes = (!rel.is_empty()).then_some(rel);
        let message = match &related_notes {
            Some(rel) => format!(
                "Created note '{safe_filename}' in '{category}'. Found {} related note(s) — consider linking them (append with links=[...]) so this note is not an orphan: {}",
                rel.len(),
                rel.iter()
                    .map(|r| r.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!("Created note '{safe_filename}' in '{category}'"),
        };

        Ok(NoteManageResult {
            related_notes,
            success: true,
            message,
            note_path: Some(note_path),
            content: None,
            notes: None,
        })
    }

    async fn handle_update(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args);
        let agent_id = agent_id_owned.as_str();

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
            related_notes: None,
            success: true,
            message: format!("Updated note '{safe_filename}' in '{category}'"),
            note_path: Some(note_path),
            content: None,
            notes: None,
        })
    }

    async fn handle_append(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args);
        let agent_id = agent_id_owned.as_str();

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
            related_notes: None,
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
        let agent_id_owned = self.resolve_agent_id(args);
        let agent_id = agent_id_owned.as_str();

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
                related_notes: None,
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
            related_notes: None,
            success: true,
            message: format!("Found {} note(s) matching '{query}'", notes.len()),
            note_path: None,
            content: Some(combined_content),
            notes: Some(notes),
        })
    }

    async fn handle_list(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args);
        let agent_id = agent_id_owned.as_str();
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
            related_notes: None,
            success: true,
            message: format!("{} note(s){category_label}", entries.len()),
            note_path: None,
            content: None,
            notes: Some(entries),
        })
    }

    async fn handle_delete(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args);
        let agent_id = agent_id_owned.as_str();

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
            related_notes: None,
            success: true,
            message: format!("Deleted note '{safe_filename}' from '{category}'"),
            note_path: Some(note_path),
            content: None,
            notes: None,
        })
    }

    /// Read materialized knowledge-graph health insights for the agent: knowledge
    /// gaps (isolated notes), sparse communities, bridge notes, and surprising
    /// cross-community connections. Read-only — the insights are materialized by
    /// `GraphRecomputeStage` during dreaming, so an empty result simply means the
    /// graph has not been recomputed yet rather than an error.
    async fn handle_insights(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args);
        let agent_id = agent_id_owned.as_str();
        let rows = self
            .indexer
            .store()
            .read_graph_insights(agent_id, None)
            .await
            .map_err(|e| AlephError::tool(format!("read insights failed: {e}")))?;
        let mut content = String::from("# Knowledge Graph Insights\n\n");
        if rows.is_empty() {
            content.push_str(
                "_No materialized insights yet (graph recompute runs during dreaming)._\n",
            );
        } else {
            for (kind, payload) in &rows {
                content.push_str(&format!("## {kind}\n```json\n{payload}\n```\n\n"));
            }
        }
        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: format!("Graph insights ({} kinds)", rows.len()),
            note_path: None,
            content: Some(content),
            notes: None,
        })
    }

    /// Read-only: summarize the memory-evolution gate from the last few dream
    /// cycles' event log. Surfaces the health score trend, best-ever score, the
    /// gate verdict, rejected merges, and any churn-pathology cooldown.
    async fn handle_evolution(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        use crate::memory::dreaming::evolution::GateOutcome;
        use crate::memory::dreaming::{EventLog, GateDecision};

        let agent_id_owned = self.resolve_agent_id(args);
        let agent_id = agent_id_owned.as_str();
        let agent_dir = self.indexer.memory_dir().join(agent_id);
        let events = EventLog::new(&agent_dir).read_last(5).await.unwrap_or_default();

        let mut content = String::from("# Memory Evolution Gate\n\n");
        if events.is_empty() {
            content.push_str("_No dream cycles recorded yet — the evolution gate runs nightly during memory consolidation._\n");
            return Ok(NoteManageResult {
                related_notes: None,
                success: true,
                message: "No dream cycles recorded yet".to_string(),
                note_path: None,
                content: Some(content),
                notes: None,
            });
        }

        for ev in events.iter().rev() {
            content.push_str(&format!(
                "## Cycle {} · strategy `{}`\n",
                ev.cycle, ev.strategy
            ));
            match &ev.report.evolution {
                Some(e) => {
                    let verdict = match e.outcome {
                        GateOutcome::AcceptNewBest => "✅ accepted (new best)",
                        GateOutcome::Accept => "✅ accepted",
                        GateOutcome::Reject => "⛔ rejected (no improvement)",
                    };
                    content.push_str(&format!(
                        "- health: {:.3} → {:.3} (best {:.3}) — {verdict}\n",
                        e.baseline, e.candidate, e.best
                    ));
                    if e.merges_rejected > 0 {
                        content.push_str(&format!(
                            "- {} proposed merge(s) rejected by the gate (would fuse distinct knowledge)\n",
                            e.merges_rejected
                        ));
                    }
                }
                None => content.push_str("- (no evolution score for this cycle)\n"),
            }
            if let GateDecision::Conserve {
                reason,
                cooldown_remaining,
            } = &ev.gate_decision
            {
                content.push_str(&format!(
                    "- ⚠️ churn pathology: {reason} (cooldown {cooldown_remaining})\n"
                ));
            }
            content.push('\n');
        }

        let latest = events.last();
        let msg = latest
            .and_then(|e| e.report.evolution.as_ref())
            .map_or_else(
                || "Evolution gate state (no score)".to_string(),
                |e| format!("Evolution gate: health {:.3} (best {:.3})", e.candidate, e.best),
            );

        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: msg,
            note_path: None,
            content: Some(content),
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
         Use this tool to store and retrieve long-term knowledge and preferences. \
         This is the DURABLE tier — searchable and recalled on relevance, not always \
         in-prompt. If a fact is identity-level and worth re-reading EVERY session \
         regardless of topic (a core preference, standing correction), also pin it to \
         the hot zone with `remember`. \
         IMPORTANT: notes form a wiki — when creating a note, ALWAYS connect it to \
         related notes via the `links` parameter; linkless notes become orphan \
         islands and are archived early. The create result returns `related_notes` \
         candidates — link the relevant ones with a follow-up append.";

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
        let result = match args.action {
            NoteManageAction::Create => self.handle_create(&args).await,
            NoteManageAction::Update => self.handle_update(&args).await,
            NoteManageAction::Append => self.handle_append(&args).await,
            NoteManageAction::Query => self.handle_query(&args).await,
            NoteManageAction::List => self.handle_list(&args).await,
            NoteManageAction::Delete => self.handle_delete(&args).await,
            NoteManageAction::Insights => self.handle_insights(&args).await,
            NoteManageAction::Evolution => self.handle_evolution(&args).await,
        }?;
        // Best-effort audit trail for the memory_timeline tool.
        self.record_lifecycle_event(&args, &result).await;
        Ok(result)
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Extract up to 4 significant keywords (length >= 4, lowercased, deduped,
/// input order preserved) from a note's title+content for the per-keyword
/// related-note FTS search after `create`.
fn related_keywords(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if word.chars().count() < 4 {
            continue;
        }
        let lower = word.to_lowercase();
        if !out.contains(&lower) {
            out.push(lower);
            if out.len() >= 4 {
                break;
            }
        }
    }
    out
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
    fn test_valid_category_check() {
        assert!(validate_category("reference").is_ok());
        assert!(validate_category("preference").is_ok());
        assert!(validate_category("subagent-run").is_ok());
        assert!(validate_category("unknown-cat").is_err());
        assert!(validate_category("").is_err());
    }

    use crate::memory::store::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn mk_tool() -> (tempfile::TempDir, NoteManageTool) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let tool = NoteManageTool::new(dir.path().join("note"), backend);
        (dir, tool)
    }

    fn create_args(filename: &str, content: &str) -> NoteManageArgs {
        NoteManageArgs {
            action: NoteManageAction::Create,
            category: Some("learning".into()),
            filename: Some(filename.into()),
            title: Some(filename.into()),
            content: Some(content.into()),
            facts: None,
            links: None,
            tags: None,
            query: None,
            limit: None,
            agent_id: None,
        }
    }

    #[tokio::test]
    async fn create_surfaces_related_notes() {
        let (_d, tool) = mk_tool();
        let r1 = tool
            .call(create_args(
                "tokio-basics",
                "- tokioruntime event loop basics",
            ))
            .await
            .unwrap();
        assert!(r1.success);
        // Second note is highly related to the first -> related_notes must
        // surface the first one.
        let r2 = tool
            .call(create_args(
                "tokio-advanced",
                "- advanced tokioruntime scheduling patterns",
            ))
            .await
            .unwrap();
        assert!(r2.success);
        let related = r2.related_notes.expect("related notes should surface");
        assert!(
            related.iter().any(|n| n.path == "learning/tokio-basics"),
            "expected learning/tokio-basics in {related:?}"
        );
        // The just-created note never appears in its own candidates.
        assert!(related.iter().all(|n| n.path != "learning/tokio-advanced"));
        // The message carries the linking nudge.
        assert!(r2.message.contains("consider linking"));
    }

    #[tokio::test]
    async fn insights_action_returns_ok_on_empty_graph() {
        let (_d, tool) = mk_tool();
        let args = NoteManageArgs {
            action: NoteManageAction::Insights,
            category: None,
            filename: None,
            title: None,
            content: None,
            facts: None,
            links: None,
            tags: None,
            query: None,
            limit: None,
            agent_id: None,
        };
        let r = tool.call(args).await.unwrap();
        assert!(r.success);
    }

    #[tokio::test]
    async fn create_with_no_related_notes_omits_field() {
        let (_d, tool) = mk_tool();
        let r = tool
            .call(create_args(
                "zzz-unique",
                "- completely unrelated xyzzy fact",
            ))
            .await
            .unwrap();
        assert!(r.success);
        assert!(r.related_notes.is_none());
    }
}
