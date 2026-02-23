//! Workspace CRUD operations backed by MemoryFact storage.
//!
//! Workspaces are persisted as special Facts under the
//! `aleph://system/workspaces/` VFS path prefix. This allows the memory
//! system to manage workspace metadata without introducing a separate
//! persistence layer.

use crate::error::AlephError;
use crate::memory::context::{
    FactSource, FactSpecificity, FactType, MemoryCategory, MemoryFact, MemoryLayer, TemporalScope,
};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::MemoryBackend;
use crate::memory::store::MemoryStore;
use crate::memory::workspace::{Workspace, DEFAULT_WORKSPACE};

/// VFS path prefix under which workspace definition facts are stored.
const WORKSPACE_PATH_PREFIX: &str = "aleph://system/workspaces/";

/// Create a new workspace by storing its definition as a Fact.
pub async fn create_workspace(db: &MemoryBackend, ws: &Workspace) -> Result<(), AlephError> {
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
/// them back into `Workspace` structs.
pub async fn list_workspaces(db: &MemoryBackend) -> Result<Vec<Workspace>, AlephError> {
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
            if let Ok(ws) = serde_json::from_str::<Workspace>(&fact.content) {
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
) -> Result<Option<Workspace>, AlephError> {
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
