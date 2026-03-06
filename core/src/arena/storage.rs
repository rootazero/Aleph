//! SQLite persistence for SharedArena state.
//!
//! Provides CRUD functions for persisting arenas, slots, and artifacts
//! to the StateDatabase.

use crate::resilience::database::StateDatabase;
use super::types::{ArtifactId, ArtifactKind, ArenaId, ArenaManifest, ArenaStatus};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};

// =============================================================================
// Helpers
// =============================================================================

/// Convert ArenaStatus to its lowercase string representation for storage.
fn status_to_str(status: &ArenaStatus) -> &'static str {
    match status {
        ArenaStatus::Created => "created",
        ArenaStatus::Active => "active",
        ArenaStatus::Settling => "settling",
        ArenaStatus::Archived => "archived",
    }
}

/// Convert ArtifactKind to its lowercase string representation for storage.
fn kind_to_str(kind: &ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Text => "text",
        ArtifactKind::Code => "code",
        ArtifactKind::File => "file",
        ArtifactKind::StructuredData => "structured_data",
    }
}

// =============================================================================
// Arena CRUD
// =============================================================================

/// Save a new arena to the database.
pub fn save_arena(
    db: &StateDatabase,
    arena_id: &ArenaId,
    manifest: &ArenaManifest,
    status: &ArenaStatus,
) -> Result<(), String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());

    let strategy_json =
        serde_json::to_string(&manifest.strategy).map_err(|e| format!("serialize strategy: {}", e))?;
    let participants_json =
        serde_json::to_string(&manifest.participants).map_err(|e| format!("serialize participants: {}", e))?;

    conn.execute(
        r#"
        INSERT OR REPLACE INTO arenas (id, goal, strategy, participants, created_by, status, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
        params![
            arena_id.as_str(),
            manifest.goal,
            strategy_json,
            participants_json,
            manifest.created_by,
            status_to_str(status),
            manifest.created_at.to_rfc3339(),
        ],
    )
    .map_err(|e| format!("save_arena: {}", e))?;

    Ok(())
}

/// Update the status (and optionally settled_at) of an existing arena.
pub fn update_arena_status(
    db: &StateDatabase,
    arena_id: &ArenaId,
    status: &ArenaStatus,
) -> Result<(), String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());

    let settled_at = if *status == ArenaStatus::Archived {
        Some(Utc::now().to_rfc3339())
    } else {
        None
    };

    conn.execute(
        r#"
        UPDATE arenas SET status = ?1, settled_at = ?2 WHERE id = ?3
        "#,
        params![status_to_str(status), settled_at, arena_id.as_str()],
    )
    .map_err(|e| format!("update_arena_status: {}", e))?;

    Ok(())
}

/// Load an arena's goal and status by ID.
///
/// Returns `Ok(None)` if the arena does not exist.
pub fn load_arena(
    db: &StateDatabase,
    arena_id: &ArenaId,
) -> Result<Option<(String, String)>, String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());

    let result = conn
        .query_row(
            "SELECT goal, status FROM arenas WHERE id = ?1",
            params![arena_id.as_str()],
            |row| {
                let goal: String = row.get(0)?;
                let status: String = row.get(1)?;
                Ok((goal, status))
            },
        )
        .optional()
        .map_err(|e| format!("load_arena: {}", e))?;

    Ok(result)
}

// =============================================================================
// Artifact CRUD
// =============================================================================

/// Save an artifact to the database.
pub fn save_artifact(
    db: &StateDatabase,
    artifact_id: &ArtifactId,
    arena_id: &ArenaId,
    agent_id: &str,
    kind: &ArtifactKind,
    content: Option<&str>,
    reference: Option<&str>,
) -> Result<(), String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());

    conn.execute(
        r#"
        INSERT OR REPLACE INTO arena_artifacts (id, arena_id, agent_id, kind, content, reference, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
        params![
            artifact_id.as_str(),
            arena_id.as_str(),
            agent_id,
            kind_to_str(kind),
            content,
            reference,
            Utc::now().to_rfc3339(),
        ],
    )
    .map_err(|e| format!("save_artifact: {}", e))?;

    Ok(())
}

/// Load artifacts for a given arena and agent.
///
/// Returns a list of `(artifact_id, content)` tuples.
/// Artifacts without inline content return an empty string for `content`.
pub fn load_artifacts(
    db: &StateDatabase,
    arena_id: &ArenaId,
    agent_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());

    let mut stmt = conn
        .prepare(
            "SELECT id, COALESCE(content, '') FROM arena_artifacts WHERE arena_id = ?1 AND agent_id = ?2",
        )
        .map_err(|e| format!("load_artifacts prepare: {}", e))?;

    let rows = stmt
        .query_map(params![arena_id.as_str(), agent_id], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            Ok((id, content))
        })
        .map_err(|e| format!("load_artifacts query: {}", e))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| format!("load_artifacts row: {}", e))?);
    }

    Ok(results)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::types::{
        ArenaManifest, ArenaPermissions, ArtifactKind, CoordinationStrategy, Participant,
        ParticipantRole,
    };
    use chrono::Utc;

    /// Helper: create a test manifest.
    fn test_manifest() -> ArenaManifest {
        ArenaManifest {
            goal: "Build a widget".to_string(),
            strategy: CoordinationStrategy::Peer {
                coordinator: "agent-a".to_string(),
            },
            participants: vec![Participant {
                agent_id: "agent-a".to_string(),
                role: ParticipantRole::Coordinator,
                permissions: ArenaPermissions::from_role(ParticipantRole::Coordinator),
            }],
            created_by: "agent-a".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn save_and_load_arena() {
        let db = StateDatabase::in_memory().expect("in-memory db");
        let arena_id = ArenaId::from_string("arena-1");
        let manifest = test_manifest();

        save_arena(&db, &arena_id, &manifest, &ArenaStatus::Created).unwrap();

        let loaded = load_arena(&db, &arena_id).unwrap();
        assert!(loaded.is_some());
        let (goal, status) = loaded.unwrap();
        assert_eq!(goal, "Build a widget");
        assert_eq!(status, "created");
    }

    #[test]
    fn update_arena_status_changes_value() {
        let db = StateDatabase::in_memory().expect("in-memory db");
        let arena_id = ArenaId::from_string("arena-2");
        let manifest = test_manifest();

        save_arena(&db, &arena_id, &manifest, &ArenaStatus::Active).unwrap();

        // Verify initial status
        let (_, status) = load_arena(&db, &arena_id).unwrap().unwrap();
        assert_eq!(status, "active");

        // Update to Archived
        update_arena_status(&db, &arena_id, &ArenaStatus::Archived).unwrap();

        let (_, status) = load_arena(&db, &arena_id).unwrap().unwrap();
        assert_eq!(status, "archived");
    }

    #[test]
    fn save_and_load_artifact() {
        let db = StateDatabase::in_memory().expect("in-memory db");
        let arena_id = ArenaId::from_string("arena-3");
        let manifest = test_manifest();

        // Must save arena first (for referential integrity)
        save_arena(&db, &arena_id, &manifest, &ArenaStatus::Active).unwrap();

        let artifact_id = ArtifactId::from_string("art-1");
        save_artifact(
            &db,
            &artifact_id,
            &arena_id,
            "agent-a",
            &ArtifactKind::Text,
            Some("Hello, world!"),
            None,
        )
        .unwrap();

        let artifacts = load_artifacts(&db, &arena_id, "agent-a").unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].0, "art-1");
        assert_eq!(artifacts[0].1, "Hello, world!");
    }
}
