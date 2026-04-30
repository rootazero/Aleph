//! SQLite persistence for SharedArena state.
//!
//! Provides CRUD functions for persisting arenas, slots, and artifacts
//! to the StateDatabase.

use super::types::{ArenaId, ArenaManifest, ArenaStatus, ArtifactId, ArtifactKind};
use crate::resilience::database::StateDatabase;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArenaStorageError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("arena not found: {0}")]
    NotFound(String),
}

fn status_to_str(status: &ArenaStatus) -> &'static str {
    match status {
        ArenaStatus::Created => "created",
        ArenaStatus::Active => "active",
        ArenaStatus::Settling => "settling",
        ArenaStatus::Archived => "archived",
    }
}

fn kind_to_str(kind: &ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Text => "text",
        ArtifactKind::Code => "code",
        ArtifactKind::File => "file",
        ArtifactKind::StructuredData => "structured_data",
    }
}

pub fn save_arena(
    db: &StateDatabase,
    arena_id: &ArenaId,
    manifest: &ArenaManifest,
    status: &ArenaStatus,
) -> Result<(), ArenaStorageError> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());

    let strategy_json = serde_json::to_string(&manifest.strategy)
        .map_err(|e| ArenaStorageError::Serialization(format!("strategy: {}", e)))?;
    let participants_json = serde_json::to_string(&manifest.participants)
        .map_err(|e| ArenaStorageError::Serialization(format!("participants: {}", e)))?;

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
    .map_err(ArenaStorageError::Database)?;

    Ok(())
}

pub fn update_arena_status(
    db: &StateDatabase,
    arena_id: &ArenaId,
    status: &ArenaStatus,
) -> Result<(), ArenaStorageError> {
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
    .map_err(ArenaStorageError::Database)?;

    Ok(())
}

pub fn load_arena(
    db: &StateDatabase,
    arena_id: &ArenaId,
) -> Result<Option<(String, String)>, ArenaStorageError> {
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
        .map_err(ArenaStorageError::Database)?;

    Ok(result)
}

pub fn save_artifact(
    db: &StateDatabase,
    artifact_id: &ArtifactId,
    arena_id: &ArenaId,
    agent_id: &str,
    kind: &ArtifactKind,
    content: Option<&str>,
    reference: Option<&str>,
) -> Result<(), ArenaStorageError> {
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
    .map_err(ArenaStorageError::Database)?;

    Ok(())
}

pub fn load_artifacts(
    db: &StateDatabase,
    arena_id: &ArenaId,
    agent_id: &str,
) -> Result<Vec<(String, String)>, ArenaStorageError> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());

    let mut stmt = conn.prepare(
        "SELECT id, COALESCE(content, '') FROM arena_artifacts WHERE arena_id = ?1 AND agent_id = ?2",
    )?;

    let rows = stmt.query_map(params![arena_id.as_str(), agent_id], |row| {
        let id: String = row.get(0)?;
        let content: String = row.get(1)?;
        Ok((id, content))
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::types::{
        ArenaManifest, ArenaPermissions, ArtifactKind, CoordinationStrategy, Participant,
        ParticipantRole,
    };
    use chrono::Utc;

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

        let (_, status) = load_arena(&db, &arena_id).unwrap().unwrap();
        assert_eq!(status, "active");

        update_arena_status(&db, &arena_id, &ArenaStatus::Archived).unwrap();

        let (_, status) = load_arena(&db, &arena_id).unwrap().unwrap();
        assert_eq!(status, "archived");
    }

    #[test]
    fn save_and_load_artifact() {
        let db = StateDatabase::in_memory().expect("in-memory db");
        let arena_id = ArenaId::from_string("arena-3");
        let manifest = test_manifest();

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
