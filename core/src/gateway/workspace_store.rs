//! Legacy workspace CRUD operations backed by MemoryFact storage.
//!
//! **Deprecated**: This module contains the legacy workspace persistence layer
//! that stores workspace definitions as MemoryFacts under a VFS path prefix.
//! It will be replaced by `WorkspaceManager` (SQLite-backed) in T6.
//!
//! The `LegacyWorkspace` type here is the old memory-centric Workspace struct.
//! New code should use `crate::gateway::workspace::Workspace` instead.

use serde::{Deserialize, Serialize};

use crate::error::AlephError;
use crate::memory::context::{
    FactSource, FactSpecificity, FactType, MemoryCategory, MemoryFact, MemoryLayer, TemporalScope,
};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::MemoryBackend;
use crate::memory::store::MemoryStore;

use crate::gateway::workspace::DEFAULT_WORKSPACE;

// =============================================================================
// Legacy Workspace types (formerly memory::workspace)
// =============================================================================

/// A workspace represents an isolated memory context with its own configuration.
///
/// **Deprecated**: This is the legacy memory-backed workspace definition.
/// New code should use `crate::gateway::workspace::Workspace`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyWorkspace {
    /// Unique workspace identifier (URL-safe slug)
    pub id: String,

    /// Human-readable display name
    pub name: String,

    /// Optional description of the workspace purpose
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional emoji or icon identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Workspace-specific configuration overrides
    #[serde(default)]
    pub config: LegacyWorkspaceConfig,

    /// Whether this is the default workspace
    #[serde(default)]
    pub is_default: bool,

    /// Whether this workspace is archived (soft-deleted)
    #[serde(default)]
    pub is_archived: bool,

    /// Creation timestamp (unix seconds)
    pub created_at: i64,

    /// Last update timestamp (unix seconds)
    pub updated_at: i64,
}

impl LegacyWorkspace {
    /// Create a new workspace with the given id and name.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            icon: None,
            config: LegacyWorkspaceConfig::default(),
            is_default: false,
            is_archived: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create the default workspace instance.
    pub fn default_workspace() -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: DEFAULT_WORKSPACE.to_string(),
            name: "Default".to_string(),
            description: Some("Default workspace for all memories".to_string()),
            icon: None,
            config: LegacyWorkspaceConfig::default(),
            is_default: true,
            is_archived: false,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Configuration overrides for a legacy workspace.
///
/// All fields are optional; when `None`, the global default applies.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LegacyWorkspaceConfig {
    /// Memory decay rate override (0.0 = no decay, 1.0 = maximum decay)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_rate: Option<f64>,

    /// Fact types that should never decay in this workspace
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permanent_fact_types: Vec<FactType>,

    /// Default AI provider override (e.g., "anthropic", "openai")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,

    /// Default model override (e.g., "claude-sonnet-4-20250514")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// System prompt override for this workspace
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,

    /// Allowlist of tool names available in this workspace (empty = all tools)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
}

// =============================================================================
// CRUD operations
// =============================================================================

/// VFS path prefix under which workspace definition facts are stored.
const WORKSPACE_PATH_PREFIX: &str = "aleph://system/workspaces/";

/// Create a new workspace by storing its definition as a Fact.
pub async fn create_workspace(db: &MemoryBackend, ws: &LegacyWorkspace) -> Result<(), AlephError> {
    let content = serde_json::to_string(ws)
        .map_err(|e| AlephError::config(format!("Failed to serialize workspace: {}", e)))?;
    let now = chrono::Utc::now().timestamp();
    let path = format!("{}{}", WORKSPACE_PATH_PREFIX, ws.id);
    let parent_path = WORKSPACE_PATH_PREFIX.to_string();

    let fact = MemoryFact {
        id: format!("ws-{}", ws.id),
        content,
        fact_type: FactType::Other,
        embedding: None,
        source_memory_ids: vec![],
        created_at: now,
        updated_at: now,
        confidence: 1.0,
        is_valid: true,
        invalidation_reason: None,
        decay_invalidated_at: None,
        specificity: FactSpecificity::Principle,
        temporal_scope: TemporalScope::Permanent,
        similarity_score: None,
        path,
        layer: MemoryLayer::L2Detail,
        category: MemoryCategory::Entities,
        fact_source: FactSource::Manual,
        content_hash: String::new(),
        parent_path,
        embedding_model: String::new(),
        namespace: "owner".to_string(),
        workspace: DEFAULT_WORKSPACE.to_string(),
        tier: crate::memory::context::MemoryTier::ShortTerm,
        scope: crate::memory::context::MemoryScope::Global,
        persona_id: None,
        strength: 1.0,
        access_count: 0,
        last_accessed_at: None,
    };

    db.insert_fact(&fact).await
}

/// List all workspaces.
///
/// Retrieves all facts under the workspace path prefix and deserializes
/// them back into `LegacyWorkspace` structs.
pub async fn list_workspaces(db: &MemoryBackend) -> Result<Vec<LegacyWorkspace>, AlephError> {
    let facts = db
        .list_by_path(
            WORKSPACE_PATH_PREFIX,
            &NamespaceScope::Owner,
            DEFAULT_WORKSPACE,
        )
        .await?;

    let mut workspaces = Vec::new();
    for entry in facts {
        if let Some(fact) = db
            .get_by_path(&entry.path, &NamespaceScope::Owner, DEFAULT_WORKSPACE)
            .await?
        {
            if let Ok(ws) = serde_json::from_str::<LegacyWorkspace>(&fact.content) {
                workspaces.push(ws);
            }
        }
    }
    Ok(workspaces)
}

/// Get a workspace by ID.
pub async fn get_workspace(
    db: &MemoryBackend,
    id: &str,
) -> Result<Option<LegacyWorkspace>, AlephError> {
    let path = format!("{}{}", WORKSPACE_PATH_PREFIX, id);
    if let Some(fact) = db
        .get_by_path(&path, &NamespaceScope::Owner, DEFAULT_WORKSPACE)
        .await?
    {
        Ok(serde_json::from_str(&fact.content).ok())
    } else {
        Ok(None)
    }
}

/// Archive (soft-delete) a workspace.
///
/// The default workspace cannot be archived.
pub async fn archive_workspace(db: &MemoryBackend, id: &str) -> Result<(), AlephError> {
    if let Some(mut ws) = get_workspace(db, id).await? {
        if ws.is_default {
            return Err(AlephError::config(
                "Cannot archive default workspace".to_string(),
            ));
        }
        ws.is_archived = true;
        ws.updated_at = chrono::Utc::now().timestamp();
        // Delete old fact and create updated one
        db.delete_fact(&format!("ws-{}", id)).await?;
        create_workspace(db, &ws).await
    } else {
        Err(AlephError::config(format!(
            "Workspace '{}' not found",
            id
        )))
    }
}
