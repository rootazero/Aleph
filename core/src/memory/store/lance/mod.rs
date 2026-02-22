//! LanceDB-backed storage implementation.
//!
//! Implements `MemoryStore`, `GraphStore`, and `SessionStore` traits
//! using LanceDB as the underlying vector database engine.

use std::path::Path;
use std::sync::Arc;

use lancedb::connection::Connection;
use lancedb::Table;

use crate::error::AlephError;

pub mod arrow_convert;
pub mod facts;
pub mod graph;
pub mod schema;
pub mod sessions;

/// LanceDB-backed memory storage backend.
///
/// Manages four tables:
/// - `facts`       — extracted knowledge facts with multi-dimensional embeddings
/// - `graph_nodes` — knowledge graph entity nodes
/// - `graph_edges` — knowledge graph relationships
/// - `memories`    — raw conversation memory entries
pub struct LanceMemoryBackend {
    pub(crate) db: Connection,
    pub(crate) facts_table: Table,
    pub(crate) nodes_table: Table,
    pub(crate) edges_table: Table,
    pub(crate) memories_table: Table,
}

impl LanceMemoryBackend {
    /// Open an existing LanceDB memory database or create a new one.
    ///
    /// The database is stored at `<data_dir>/memory.lance`.
    /// All four tables are created if they do not already exist.
    pub async fn open_or_create(data_dir: &Path) -> Result<Self, AlephError> {
        let db_path = data_dir.join("memory.lance");
        let db = lancedb::connect(db_path.to_str().unwrap())
            .execute()
            .await
            .map_err(|e| AlephError::config(format!("LanceDB connect failed: {}", e)))?;

        let facts_table =
            Self::ensure_table(&db, "facts", schema::facts_schema()).await?;
        let nodes_table =
            Self::ensure_table(&db, "graph_nodes", schema::graph_nodes_schema()).await?;
        let edges_table =
            Self::ensure_table(&db, "graph_edges", schema::graph_edges_schema()).await?;
        let memories_table =
            Self::ensure_table(&db, "memories", schema::memories_schema()).await?;

        Ok(Self {
            db,
            facts_table,
            nodes_table,
            edges_table,
            memories_table,
        })
    }

    /// Ensure a table exists — open if it already exists, create empty if not.
    async fn ensure_table(
        db: &Connection,
        name: &str,
        schema: Arc<arrow_schema::Schema>,
    ) -> Result<Table, AlephError> {
        match db.open_table(name).execute().await {
            Ok(table) => Ok(table),
            Err(_) => db
                .create_empty_table(name, schema)
                .execute()
                .await
                .map_err(|e| {
                    AlephError::config(format!("Failed to create table '{}': {}", name, e))
                }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_open_or_create_new_database() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();

        // Verify all tables were created
        let tables = backend.db.table_names().execute().await.unwrap();
        assert!(tables.contains(&"facts".to_string()));
        assert!(tables.contains(&"graph_nodes".to_string()));
        assert!(tables.contains(&"graph_edges".to_string()));
        assert!(tables.contains(&"memories".to_string()));
    }

    #[tokio::test]
    async fn test_open_existing_database() {
        let tmp = tempfile::tempdir().unwrap();

        // Create database
        let _backend1 = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();
        drop(_backend1);

        // Re-open — should succeed without creating new tables
        let backend2 = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();
        let tables = backend2.db.table_names().execute().await.unwrap();
        assert_eq!(tables.len(), 4);
    }
}
