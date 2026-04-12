//! Tool Index Coordinator - synchronizes tools with Memory system
//!
//! The coordinator is responsible for:
//! - Adding/updating tools as notes in the `tool/` category
//! - Removing tools by deleting their note files and index entries
//! - Bulk synchronization of tools
//! - Retrieving all valid tool notes

use std::path::PathBuf;

use super::inference::SemanticPurposeInferrer;
use crate::error::AlephError;
use crate::mcp::manager::{McpManagerEvent, McpManagerHandle};
use crate::memory::context::{
    FactSource, FactSpecificity, NoteType, MemoryCategory, MemoryFact, MemoryLayer, TemporalScope,
};
use crate::memory::notes::{KnowledgeNote, NoteIndexer};
use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryBackend;
use crate::skill::{SkillSystem, SkillSystemEvent};
use crate::sync_primitives::Arc;
use tokio::sync::broadcast;

/// Metadata for a tool to be indexed
#[derive(Debug, Clone)]
pub struct ToolMeta {
    /// Tool name (e.g., "read_file", "search_code")
    pub name: String,
    /// Tool's existing description
    pub description: Option<String>,
    /// Tool category (e.g., "file", "search", "code")
    pub category: Option<String>,
    /// Curated semantic metadata (highest quality source)
    pub structured_meta: Option<String>,
    /// Pre-computed embedding vector
    pub embedding: Option<Vec<f32>>,
}

impl ToolMeta {
    /// Create a new ToolMeta with just a name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            category: None,
            structured_meta: None,
            embedding: None,
        }
    }

    /// Set the description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the category
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Set the structured metadata
    pub fn with_structured_meta(mut self, meta: impl Into<String>) -> Self {
        self.structured_meta = Some(meta.into());
        self
    }

    /// Set the embedding
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }
}

// Agent ID used for all tool notes — tools are system-level, owner-scoped.
const TOOL_AGENT_ID: &str = "owner";

// Note category under which tool descriptors are stored.
const TOOL_CATEGORY: &str = "tool";

/// Coordinates tool synchronization with Memory system
///
/// Stores tools as notes in the `tool/` category (markdown files + notes_index).
/// Uses SemanticPurposeInferrer to generate rich content descriptions.
pub struct ToolIndexCoordinator {
    db: MemoryBackend,
    indexer: NoteIndexer<crate::memory::store::SqliteMemoryBackend>,
    inferrer: Arc<SemanticPurposeInferrer>,
}

impl ToolIndexCoordinator {
    /// Create a new coordinator with a database reference and memory directory.
    ///
    /// `memory_dir` is the root notes directory (parent of all agent directories),
    /// typically `~/.aleph/data/memory/note/`.
    pub fn new(db: MemoryBackend, memory_dir: PathBuf) -> Self {
        Self {
            db: db.clone(),
            indexer: NoteIndexer::new(memory_dir, db),
            inferrer: Arc::new(SemanticPurposeInferrer::new()),
        }
    }

    /// Create a new coordinator with LLM support for L2 optimization.
    pub fn with_llm(
        db: MemoryBackend,
        memory_dir: PathBuf,
        llm_provider: Arc<dyn crate::providers::AiProvider>,
    ) -> Self {
        Self {
            db: db.clone(),
            indexer: NoteIndexer::new(memory_dir, db),
            inferrer: Arc::new(SemanticPurposeInferrer::with_llm(llm_provider)),
        }
    }

    /// Generate a stable note path for a tool: `"tool/{tool_name}"`.
    fn tool_note_path(name: &str) -> String {
        format!("{TOOL_CATEGORY}/{name}")
    }

    /// Get current timestamp in Unix seconds
    fn now_timestamp() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    /// Reconstruct a `MemoryFact`-compatible struct from a note.
    ///
    /// Used to preserve the public API shape that callers (tests, ToolRetrieval) depend on.
    fn note_to_fact(note: &KnowledgeNote, confidence: f32) -> MemoryFact {
        let now = Self::now_timestamp();
        let content = note.facts.join("\n");
        MemoryFact {
            id: format!("tool:{}", note.title),
            content: content.clone(),
            note_type: NoteType::Tool,
            embedding: None,
            source_memory_ids: vec![],
            created_at: if note.created_at > 0 { note.created_at } else { now },
            updated_at: if note.updated_at > 0 { note.updated_at } else { now },
            confidence,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::Principle,
            temporal_scope: TemporalScope::Permanent,
            similarity_score: None,
            path: String::new(),
            layer: MemoryLayer::L2Detail,
            category: MemoryCategory::Patterns,
            fact_source: FactSource::Extracted,
            content_hash: note.content_hash.clone(),
            parent_path: String::new(),
            embedding_model: String::new(),
            namespace: "owner".to_string(),
            agent: TOOL_AGENT_ID.to_string(),
            tier: crate::memory::context::MemoryTier::ShortTerm,
            scope: crate::memory::context::MemoryScope::Global,
            persona_id: None,
            strength: 1.0,
            access_count: 0,
            last_accessed_at: None,
            valid_from: None,
            valid_to: None,
        }
    }

    /// Sync a single tool to Memory as a note in `tool/` category.
    ///
    /// Creates or overwrites the tool note with inferred semantic purpose.
    /// Returns the stable note path (`"tool:{name}"`) on success.
    ///
    /// # Arguments
    /// * `name` - Tool name
    /// * `description` - Tool's existing description
    /// * `category` - Tool category
    /// * `structured_meta` - Curated semantic metadata
    /// * `embedding` - Pre-computed embedding vector
    pub async fn sync_tool(
        &self,
        name: &str,
        description: Option<&str>,
        category: Option<&str>,
        structured_meta: Option<&str>,
        embedding: Option<Vec<f32>>,
    ) -> Result<String, AlephError> {
        // Infer semantic purpose using ranked strategy (L0 -> L1)
        let inferred = self
            .inferrer
            .infer(name, description, category, structured_meta);

        // Log optimization level for observability
        tracing::info!(
            tool_name = %name,
            optimization_level = %format!("L{}", inferred.level),
            confidence = %inferred.confidence,
            "Tool indexed with semantic inference"
        );

        // Build content: inferred purpose + original description for context
        let content = if let Some(desc) = description {
            if !desc.is_empty() {
                format!("{}\n\nOriginal: {}", inferred.description, desc)
            } else {
                inferred.description.clone()
            }
        } else {
            inferred.description.clone()
        };

        let now = Self::now_timestamp();

        // Build the note (Option A: full content as single fact for clean roundtrip)
        let note = KnowledgeNote {
            title: name.to_string(),
            category: TOOL_CATEGORY.to_string(),
            tags: vec!["tool".to_string()],
            facts: vec![content],
            links: vec![],
            created_at: now,
            updated_at: now,
            content_hash: String::new(), // will be computed on write
        };

        // Write note file + update notes_index
        self.indexer.write_note(TOOL_AGENT_ID, TOOL_CATEGORY, &note).await?;

        // Re-index from disk so content_hash and index are consistent
        let note_path = self
            .indexer
            .memory_dir()
            .join(TOOL_AGENT_ID)
            .join(TOOL_CATEGORY)
            .join(format!("{name}.md"));
        self.indexer.index_file(TOOL_AGENT_ID, TOOL_CATEGORY, &note_path).await?;

        // Store embedding in notes vector table if provided.
        // ToolRetrieval will be switched to query notes in Task 8.
        if let Some(emb) = embedding {
            let note_path = Self::tool_note_path(name);
            let dim = emb.len() as u32;
            self.indexer
                .store()
                .upsert_embedding(&note_path, TOOL_AGENT_ID, &emb, dim)
                .await?;
        }

        // Schedule L2 optimization if needed (async, non-blocking)
        if self.inferrer.should_trigger_l2(&inferred) {
            tracing::debug!(
                tool_name = %name,
                "Scheduling L2 async optimization"
            );

            let indexer_store = Arc::clone(self.indexer.store());
            let memory_dir = self.indexer.memory_dir().to_path_buf();
            let db_clone = self.db.clone();
            let inferrer = Arc::clone(&self.inferrer);
            let tool_name = name.to_string();
            let tool_desc = description.map(|s| s.to_string());
            let tool_cat = category.map(|s| s.to_string());
            let note_id = format!("tool:{name}");

            tokio::spawn(async move {
                match inferrer
                    .enhance_with_llm(
                        &note_id,
                        &tool_name,
                        tool_desc.as_deref(),
                        tool_cat.as_deref(),
                    )
                    .await
                {
                    Ok(l2_result) => {
                        tracing::info!(
                            tool_name = %tool_name,
                            optimization_level = "L2",
                            confidence = %l2_result.confidence,
                            "L2 optimization completed"
                        );

                        // Update note on disk with L2-enhanced content
                        let file_path = std::path::Path::new(&memory_dir)
                            .join(TOOL_AGENT_ID)
                            .join(TOOL_CATEGORY)
                            .join(format!("{tool_name}.md"));

                        // Read existing note, replace the facts content
                        if let Ok(existing_md) = tokio::fs::read_to_string(&file_path).await {
                            if let Ok(mut existing_note) =
                                KnowledgeNote::from_markdown(&tool_name, &existing_md)
                            {
                                existing_note.facts = vec![l2_result.description.clone()];
                                existing_note.updated_at = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs() as i64;

                                let updated_md = existing_note.to_markdown();
                                if let Err(e) = tokio::fs::write(&file_path, &updated_md).await {
                                    tracing::warn!(
                                        tool_name = %tool_name,
                                        error = %e,
                                        "Failed to write L2-updated note to disk"
                                    );
                                } else {
                                    // Re-index the updated file
                                    let indexer =
                                        NoteIndexer::new(memory_dir, Arc::clone(&indexer_store));
                                    let _ = indexer
                                        .index_file(TOOL_AGENT_ID, TOOL_CATEGORY, &file_path)
                                        .await;
                                }
                            }
                        }

                        let _ = &db_clone;
                        let _ = &note_id;
                    }
                    Err(e) => {
                        tracing::warn!(
                            tool_name = %tool_name,
                            error = %e,
                            "L2 optimization failed, keeping L1 description"
                        );
                    }
                }
            });
        }

        Ok(format!("tool:{name}"))
    }

    /// Remove a tool from Memory by deleting its note file and index entry.
    pub async fn remove_tool(&self, name: &str) -> Result<(), AlephError> {
        let note_path = Self::tool_note_path(name);

        // Delete the markdown file
        let file_path = self
            .indexer
            .memory_dir()
            .join(TOOL_AGENT_ID)
            .join(TOOL_CATEGORY)
            .join(format!("{name}.md"));
        tokio::fs::remove_file(&file_path).await.ok();

        // Remove from notes index
        self.indexer
            .store()
            .remove_note_index(&note_path, TOOL_AGENT_ID)
            .await?;

        Ok(())
    }

    /// Sync multiple tools in bulk.
    ///
    /// Returns the list of note paths that were created/updated.
    pub async fn sync_all(&self, tools: Vec<ToolMeta>) -> Result<Vec<String>, AlephError> {
        let mut note_ids = Vec::with_capacity(tools.len());

        for tool in tools {
            let note_id = self
                .sync_tool(
                    &tool.name,
                    tool.description.as_deref(),
                    tool.category.as_deref(),
                    tool.structured_meta.as_deref(),
                    tool.embedding,
                )
                .await?;
            note_ids.push(note_id);
        }

        Ok(note_ids)
    }

    /// Get all valid tool notes from Memory as `MemoryFact`s.
    ///
    /// Returns entries ordered by updated_at descending.
    pub async fn get_tool_facts(&self) -> Result<Vec<MemoryFact>, AlephError> {

        let entries = self
            .indexer
            .store()
            .get_notes_by_category(TOOL_AGENT_ID, TOOL_CATEGORY, 1000)
            .await?;

        let mut facts = Vec::with_capacity(entries.len());
        for entry in entries {
            // Read the markdown file to reconstruct content
            let file_path = self
                .indexer
                .memory_dir()
                .join(TOOL_AGENT_ID)
                .join(TOOL_CATEGORY)
                .join(format!("{}.md", entry.filename));

            let note = if let Ok(md) = tokio::fs::read_to_string(&file_path).await {
                KnowledgeNote::from_markdown(&entry.filename, &md).unwrap_or_else(|_| {
                    KnowledgeNote {
                        title: entry.filename.clone(),
                        category: TOOL_CATEGORY.to_string(),
                        tags: vec![],
                        facts: vec![],
                        links: vec![],
                        created_at: entry.created_at,
                        updated_at: entry.updated_at,
                        content_hash: entry.content_hash.clone(),
                    }
                })
            } else {
                KnowledgeNote {
                    title: entry.filename.clone(),
                    category: TOOL_CATEGORY.to_string(),
                    tags: vec![],
                    facts: vec![],
                    links: vec![],
                    created_at: entry.created_at,
                    updated_at: entry.updated_at,
                    content_hash: entry.content_hash.clone(),
                }
            };

            // Default confidence: assume L1 (0.75) unless we can parse it from content
            let fact = Self::note_to_fact(&note, 0.75);
            facts.push(fact);
        }

        Ok(facts)
    }

    /// Get a specific tool fact by name.
    pub async fn get_tool_fact(&self, name: &str) -> Result<Option<MemoryFact>, AlephError> {

        let note_path = Self::tool_note_path(name);
        let entry = self
            .indexer
            .store()
            .get_note_index(&note_path, TOOL_AGENT_ID)
            .await?;

        let Some(entry) = entry else {
            return Ok(None);
        };

        let file_path = self
            .indexer
            .memory_dir()
            .join(TOOL_AGENT_ID)
            .join(TOOL_CATEGORY)
            .join(format!("{}.md", entry.filename));

        let note = if let Ok(md) = tokio::fs::read_to_string(&file_path).await {
            KnowledgeNote::from_markdown(&entry.filename, &md).unwrap_or_else(|_| {
                KnowledgeNote {
                    title: entry.filename.clone(),
                    category: TOOL_CATEGORY.to_string(),
                    tags: vec![],
                    facts: vec![],
                    links: vec![],
                    created_at: entry.created_at,
                    updated_at: entry.updated_at,
                    content_hash: entry.content_hash.clone(),
                }
            })
        } else {
            KnowledgeNote {
                title: entry.filename.clone(),
                category: TOOL_CATEGORY.to_string(),
                tags: vec![],
                facts: vec![],
                links: vec![],
                created_at: entry.created_at,
                updated_at: entry.updated_at,
                content_hash: entry.content_hash.clone(),
            }
        };

        // Infer confidence from content: L0 content contains "sandboxed" style structured meta
        // We use 0.95 as default for L0 (structured_meta), 0.75 for L1.
        // Heuristic: if the note has substantial content (>80 chars), assume L0 confidence.
        let content = note.facts.join("\n");
        let confidence = if content.len() > 80 { 0.95 } else { 0.75 };

        Ok(Some(Self::note_to_fact(&note, confidence)))
    }

    /// Check if a tool note exists.
    pub async fn tool_exists(&self, name: &str) -> Result<bool, AlephError> {

        let note_path = Self::tool_note_path(name);
        let entry = self
            .indexer
            .store()
            .get_note_index(&note_path, TOOL_AGENT_ID)
            .await?;
        Ok(entry.is_some())
    }

    // ========== Event Listeners ==========

    /// Start listening to MCP Manager events
    ///
    /// This spawns a background task that listens for MCP events and
    /// automatically re-syncs tool notes when tools change on MCP servers.
    ///
    /// Events handled:
    /// - `ServerStarted`: Re-sync tools for the started server
    /// - `ToolsChanged`: Re-sync tools for the affected server
    /// - `ServerCrashed`: Remove tools for the crashed server
    ///
    /// # Arguments
    /// * `mcp_handle` - Handle to the MCP Manager
    /// * `tool_provider` - Callback to get tools for a server (server_id -> Vec<ToolMeta>)
    ///
    /// # Returns
    /// A JoinHandle for the spawned task (can be used to abort the listener)
    pub fn start_mcp_listener<F>(
        self: Arc<Self>,
        mcp_handle: McpManagerHandle,
        tool_provider: F,
    ) -> tokio::task::JoinHandle<()>
    where
        F: Fn(String) -> Vec<ToolMeta> + Send + Sync + 'static,
    {
        let mut receiver = mcp_handle.subscribe();
        let coordinator = self;
        let tool_provider = Arc::new(tool_provider);

        tokio::spawn(async move {
            tracing::info!("ToolIndexCoordinator: MCP event listener started");

            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        match &event {
                            McpManagerEvent::ServerStarted {
                                server_id,
                                tool_count,
                                ..
                            } => {
                                tracing::info!(
                                    server_id = %server_id,
                                    tool_count = %tool_count,
                                    "MCP server started, syncing tools"
                                );
                                let tools = tool_provider(server_id.clone());
                                if let Err(e) = coordinator.sync_all(tools).await {
                                    tracing::error!(
                                        error = %e,
                                        server_id = %server_id,
                                        "Failed to sync tools for started MCP server"
                                    );
                                }
                            }
                            McpManagerEvent::ToolsChanged {
                                server_id,
                                tool_count,
                            } => {
                                tracing::info!(
                                    server_id = %server_id,
                                    tool_count = %tool_count,
                                    "MCP tools changed, re-syncing"
                                );
                                let tools = tool_provider(server_id.clone());
                                if let Err(e) = coordinator.sync_all(tools).await {
                                    tracing::error!(
                                        error = %e,
                                        server_id = %server_id,
                                        "Failed to sync tools after MCP tools changed"
                                    );
                                }
                            }
                            McpManagerEvent::ServerCrashed {
                                server_id, error, ..
                            } => {
                                tracing::warn!(
                                    server_id = %server_id,
                                    error = %error,
                                    "MCP server crashed, invalidating tools"
                                );
                                // We could remove tools here, but for now just log
                                // The tools will be re-synced when server restarts
                            }
                            McpManagerEvent::ServerRemoved { server_id, .. } => {
                                tracing::info!(
                                    server_id = %server_id,
                                    "MCP server removed"
                                );
                                // Tools from this server should be removed
                                // but we need to know which tools belong to which server
                                // This would require additional metadata tracking
                            }
                            _ => {
                                // Other events (ManagerReady, ManagerShutdown, etc.)
                                // don't require tool index updates
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("MCP event channel closed, stopping listener");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            lagged = n,
                            "MCP event listener lagged, some events may have been missed"
                        );
                    }
                }
            }
        })
    }

    /// Start listening to Skill Registry events
    ///
    /// This spawns a background task that listens for skill lifecycle events
    /// and automatically re-syncs tool notes when skills are loaded/removed.
    ///
    /// Events handled:
    /// - `AllReloaded`: Re-sync all skill tools
    /// - `SkillLoaded`: Sync the single skill as a tool
    /// - `SkillRemoved`: Remove the skill's tool note
    ///
    /// # Arguments
    /// * `registry` - The SkillsRegistry to listen to
    /// * `skill_to_tool` - Callback to convert skill_id -> ToolMeta
    ///
    /// # Returns
    /// A JoinHandle for the spawned task
    pub fn start_skill_listener<F>(
        self: Arc<Self>,
        system: SkillSystem,
        skill_to_tool: F,
    ) -> tokio::task::JoinHandle<()>
    where
        F: Fn(String) -> Option<ToolMeta> + Send + Sync + 'static,
    {
        let mut receiver = system.subscribe();
        let coordinator = self;
        let skill_to_tool = Arc::new(skill_to_tool);

        tokio::spawn(async move {
            tracing::info!("ToolIndexCoordinator: Skill event listener started");

            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        match &event {
                            SkillSystemEvent::AllReloaded { count, skill_ids } => {
                                tracing::info!(
                                    count = %count,
                                    "Skills reloaded, syncing all skill tools"
                                );

                                let tools: Vec<ToolMeta> = skill_ids
                                    .iter()
                                    .filter_map(|id| skill_to_tool(id.clone()))
                                    .collect();

                                if let Err(e) = coordinator.sync_all(tools).await {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to sync tools after skills reload"
                                    );
                                }
                            }
                            SkillSystemEvent::SkillLoaded {
                                skill_id,
                                skill_name,
                            } => {
                                tracing::info!(
                                    skill_id = %skill_id,
                                    skill_name = %skill_name,
                                    "Skill loaded, syncing as tool"
                                );

                                if let Some(tool) = skill_to_tool(skill_id.clone()) {
                                    if let Err(e) = coordinator.sync_all(vec![tool]).await {
                                        tracing::error!(
                                            error = %e,
                                            skill_id = %skill_id,
                                            "Failed to sync skill as tool"
                                        );
                                    }
                                }
                            }
                            SkillSystemEvent::SkillRemoved { skill_id } => {
                                tracing::info!(
                                    skill_id = %skill_id,
                                    "Skill removed, removing tool note"
                                );

                                // Skill tools use format "skill:{skill_id}" as tool name
                                let tool_name = format!("skill:{}", skill_id);
                                if let Err(e) = coordinator.remove_tool(&tool_name).await {
                                    tracing::error!(
                                        error = %e,
                                        skill_id = %skill_id,
                                        "Failed to remove skill tool note"
                                    );
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("Skill event channel closed, stopping listener");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            lagged = n,
                            "Skill event listener lagged, some events may have been missed"
                        );
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_note_path() {
        assert_eq!(
            ToolIndexCoordinator::tool_note_path("read_file"),
            "tool/read_file"
        );
        assert_eq!(
            ToolIndexCoordinator::tool_note_path("search_code"),
            "tool/search_code"
        );
    }

    #[test]
    fn test_tool_meta_builder() {
        let meta = ToolMeta::new("read_file")
            .with_description("Read file contents")
            .with_category("file")
            .with_structured_meta("Read and retrieve content from local filesystem");

        assert_eq!(meta.name, "read_file");
        assert_eq!(meta.description, Some("Read file contents".to_string()));
        assert_eq!(meta.category, Some("file".to_string()));
        assert!(meta.structured_meta.is_some());
    }

    #[test]
    fn test_now_timestamp() {
        let ts = ToolIndexCoordinator::now_timestamp();
        // Should be a reasonable Unix timestamp (after 2020)
        assert!(ts > 1577836800); // 2020-01-01
    }
}
