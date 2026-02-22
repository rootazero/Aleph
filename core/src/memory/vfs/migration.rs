//! Migration logic for assigning paths to existing facts

use crate::error::AlephError;
use crate::memory::VectorDatabase;
use crate::memory::context::{FactType, compute_parent_path};
use tracing::{info, warn};

/// Migrate existing facts to have aleph:// paths based on their FactType.
/// Idempotent: only updates facts with empty path.
/// Returns the number of facts migrated.
pub async fn migrate_existing_facts_to_paths(database: &VectorDatabase) -> Result<usize, AlephError> {
    let conn = database.conn.lock().unwrap_or_else(|e| e.into_inner());

    // Count facts needing migration
    let count: usize = conn.query_row(
        "SELECT COUNT(*) FROM memory_facts WHERE path = '' AND is_valid = 1",
        [],
        |row| row.get(0),
    ).map_err(|e| AlephError::config(format!("Failed to count unmigrated facts: {}", e)))?;

    if count == 0 {
        info!("No facts need path migration");
        return Ok(0);
    }

    info!(count = count, "Migrating existing facts to VFS paths");

    // Fetch all valid facts with empty path
    let mut stmt = conn.prepare(
        "SELECT id, fact_type FROM memory_facts WHERE path = '' AND is_valid = 1"
    ).map_err(|e| AlephError::config(format!("Failed to prepare migration query: {}", e)))?;

    let rows: Vec<(String, String)> = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .map_err(|e| AlephError::config(format!("Failed to query facts: {}", e)))?
    .filter_map(|r| r.ok())
    .collect();

    let mut migrated = 0;
    for (id, fact_type_str) in &rows {
        let fact_type = FactType::from_str_or_other(fact_type_str);
        let path = fact_type.default_path().to_string();
        let parent = compute_parent_path(&path);

        conn.execute(
            "UPDATE memory_facts SET path = ?1, parent_path = ?2 WHERE id = ?3",
            rusqlite::params![path, parent, id],
        ).map_err(|e| {
            warn!(id = %id, error = %e, "Failed to migrate fact path");
            AlephError::config(format!("Failed to update fact {}: {}", id, e))
        })?;

        migrated += 1;
    }

    info!(migrated = migrated, "Facts path migration completed");
    Ok(migrated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::MemoryFact;

    fn create_test_db() -> VectorDatabase {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_vfs_migration_{}.db", uuid::Uuid::new_v4()));
        VectorDatabase::new(db_path).unwrap()
    }

    #[tokio::test]
    async fn test_migrate_existing_facts() {
        let db = create_test_db();

        // Insert facts with empty path (simulating pre-migration state)
        let fact1 = MemoryFact::new("Prefers Rust".into(), FactType::Preference, vec![]);
        let fact2 = MemoryFact::new("Learning WASM".into(), FactType::Learning, vec![]);
        db.insert_fact(fact1).await.unwrap();
        db.insert_fact(fact2).await.unwrap();

        // Clear paths to simulate old data
        {
            let conn = db.conn.lock().unwrap();
            conn.execute("UPDATE memory_facts SET path = '', parent_path = ''", []).unwrap();
        }

        let migrated = migrate_existing_facts_to_paths(&db).await.unwrap();
        assert_eq!(migrated, 2);

        // Verify paths were assigned
        let prefs = db.get_facts_by_path_prefix("aleph://user/preferences/").await.unwrap();
        assert_eq!(prefs.len(), 1);

        let learning = db.get_facts_by_path_prefix("aleph://knowledge/learning/").await.unwrap();
        assert_eq!(learning.len(), 1);
    }

    #[tokio::test]
    async fn test_migrate_idempotent() {
        let db = create_test_db();

        let fact = MemoryFact::new("Test".into(), FactType::Preference, vec![]);
        db.insert_fact(fact).await.unwrap();

        // Clear paths
        {
            let conn = db.conn.lock().unwrap();
            conn.execute("UPDATE memory_facts SET path = '', parent_path = ''", []).unwrap();
        }

        let first = migrate_existing_facts_to_paths(&db).await.unwrap();
        assert_eq!(first, 1);

        // Second run should be no-op
        let second = migrate_existing_facts_to_paths(&db).await.unwrap();
        assert_eq!(second, 0);
    }
}
