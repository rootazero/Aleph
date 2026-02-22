//! Path-based query operations for the VFS layer

use crate::error::AlephError;
use crate::memory::context::{FactSource, FactSpecificity, FactType, MemoryFact, TemporalScope};
use crate::memory::database::core::VectorDatabase;
use rusqlite::params;

/// A directory entry in the VFS
#[derive(Debug, Clone)]
pub struct PathEntry {
    /// Sub-path name (e.g., "coding/")
    pub name: String,
    /// Full aleph:// path
    pub full_path: String,
    /// Whether this entry has child facts (is a directory)
    pub is_directory: bool,
    /// Number of facts under this path
    pub fact_count: usize,
    /// Whether an L1 Overview exists for this path
    pub has_l1: bool,
    /// One-line summary (<100 tokens)
    pub abstract_line: String,
}

impl VectorDatabase {
    /// List direct children of a path (for `ls` operation)
    ///
    /// Returns distinct child path segments with fact counts.
    pub async fn list_path_children(&self, parent_path: &str) -> Result<Vec<PathEntry>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let parent = parent_path.to_string();

        // Get distinct child paths and their fact counts
        let mut stmt = conn.prepare(
            r#"
            SELECT path, COUNT(*) as cnt, MIN(content) as first_content
            FROM memory_facts
            WHERE parent_path = ?1 AND is_valid = 1 AND fact_source != 'summary'
            GROUP BY path
            ORDER BY path
            "#,
        ).map_err(|e| AlephError::config(format!("Failed to prepare path query: {}", e)))?;

        let entries = stmt.query_map(params![parent], |row| {
            let full_path: String = row.get(0)?;
            let fact_count: usize = row.get(1)?;
            let first_content: String = row.get(2)?;

            // Extract name from full path relative to parent
            let name = full_path.strip_prefix(&parent)
                .unwrap_or(&full_path)
                .to_string();

            Ok(PathEntry {
                name,
                full_path: full_path.clone(),
                is_directory: fact_count > 1,
                fact_count,
                has_l1: false, // Will check separately
                abstract_line: first_content.chars().take(100).collect(),
            })
        }).map_err(|e| AlephError::config(format!("Failed to query paths: {}", e)))?;

        let mut result: Vec<PathEntry> = entries
            .filter_map(|e| e.ok())
            .collect();

        // Check L1 availability for each entry
        for entry in &mut result {
            let has_l1: i64 = conn.query_row(
                "SELECT COUNT(*) FROM memory_facts WHERE path = ?1 AND fact_source = 'summary' AND is_valid = 1",
                params![entry.full_path],
                |row| row.get(0),
            ).unwrap_or(0);
            entry.has_l1 = has_l1 > 0;
        }

        Ok(result)
    }

    /// Get all valid facts under a path (including nested)
    pub async fn get_facts_by_path_prefix(&self, path_prefix: &str) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let prefix = format!("{}%", path_prefix);

        let mut stmt = conn.prepare(
            r#"
            SELECT id, content, fact_type, embedding, source_memory_ids,
                   created_at, updated_at, confidence, is_valid, invalidation_reason,
                   specificity, temporal_scope, decay_invalidated_at,
                   path, fact_source, content_hash, parent_path, embedding_model
            FROM memory_facts
            WHERE path LIKE ?1 AND is_valid = 1
            ORDER BY path, updated_at DESC
            "#,
        ).map_err(|e| AlephError::config(format!("Failed to prepare prefix query: {}", e)))?;

        let facts = stmt.query_map(params![prefix], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let fact_type_str: String = row.get(2)?;
            let embedding_bytes: Option<Vec<u8>> = row.get(3)?;
            let source_ids_json: String = row.get(4)?;
            let created_at: i64 = row.get(5)?;
            let updated_at: i64 = row.get(6)?;
            let confidence: f32 = row.get(7)?;
            let is_valid: i32 = row.get(8)?;
            let invalidation_reason: Option<String> = row.get(9)?;
            let specificity_str: String = row.get(10)?;
            let temporal_scope_str: String = row.get(11)?;
            let decay_invalidated_at: Option<i64> = row.get(12)?;
            let path: String = row.get(13)?;
            let fact_source_str: String = row.get(14)?;
            let content_hash: String = row.get(15)?;
            let parent_path: String = row.get(16)?;
            let embedding_model: String = row.get(17)?;

            let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));
            let source_memory_ids: Vec<String> =
                serde_json::from_str(&source_ids_json).unwrap_or_default();

            Ok(MemoryFact {
                id,
                content,
                fact_type: FactType::from_str_or_other(&fact_type_str),
                embedding,
                source_memory_ids,
                created_at,
                updated_at,
                confidence,
                is_valid: is_valid != 0,
                invalidation_reason,
                decay_invalidated_at,
                specificity: FactSpecificity::from_str_or_default(&specificity_str),
                temporal_scope: TemporalScope::from_str_or_default(&temporal_scope_str),
                similarity_score: None,
                path,
                fact_source: FactSource::from_str_or_default(&fact_source_str),
                content_hash,
                parent_path,
                embedding_model,
            })
        }).map_err(|e| AlephError::config(format!("Failed to query facts by prefix: {}", e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AlephError::config(format!("Failed to parse fact rows: {}", e)))?;

        Ok(facts)
    }

    /// Get the L1 Summary fact for a path (if exists)
    pub async fn get_l1_overview(&self, path: &str) -> Result<Option<MemoryFact>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let result = conn.query_row(
            r#"
            SELECT id, content, fact_type, embedding, source_memory_ids,
                   created_at, updated_at, confidence, is_valid, invalidation_reason,
                   specificity, temporal_scope, decay_invalidated_at,
                   path, fact_source, content_hash, parent_path, embedding_model
            FROM memory_facts
            WHERE path = ?1 AND fact_source = 'summary' AND is_valid = 1
            ORDER BY updated_at DESC
            LIMIT 1
            "#,
            params![path],
            |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let fact_type_str: String = row.get(2)?;
                let embedding_bytes: Option<Vec<u8>> = row.get(3)?;
                let source_ids_json: String = row.get(4)?;
                let created_at: i64 = row.get(5)?;
                let updated_at: i64 = row.get(6)?;
                let confidence: f32 = row.get(7)?;
                let is_valid: i32 = row.get(8)?;
                let invalidation_reason: Option<String> = row.get(9)?;
                let specificity_str: String = row.get(10)?;
                let temporal_scope_str: String = row.get(11)?;
                let decay_invalidated_at: Option<i64> = row.get(12)?;
                let path: String = row.get(13)?;
                let fact_source_str: String = row.get(14)?;
                let content_hash: String = row.get(15)?;
                let parent_path: String = row.get(16)?;
                let embedding_model: String = row.get(17)?;

                let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));
                let source_memory_ids: Vec<String> =
                    serde_json::from_str(&source_ids_json).unwrap_or_default();

                Ok(MemoryFact {
                    id,
                    content,
                    fact_type: FactType::from_str_or_other(&fact_type_str),
                    embedding,
                    source_memory_ids,
                    created_at,
                    updated_at,
                    confidence,
                    is_valid: is_valid != 0,
                    invalidation_reason,
                    decay_invalidated_at,
                    specificity: FactSpecificity::from_str_or_default(&specificity_str),
                    temporal_scope: TemporalScope::from_str_or_default(&temporal_scope_str),
                    similarity_score: None,
                    path,
                    fact_source: FactSource::from_str_or_default(&fact_source_str),
                    content_hash,
                    parent_path,
                    embedding_model,
                })
            },
        );

        match result {
            Ok(fact) => Ok(Some(fact)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::config(format!("Failed to get L1 overview: {}", e))),
        }
    }

    /// Count facts under a path prefix
    pub async fn count_facts_by_path(&self, path_prefix: &str) -> Result<usize, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let prefix = format!("{}%", path_prefix);

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memory_facts WHERE path LIKE ?1 AND is_valid = 1 AND fact_source != 'summary'",
            params![prefix],
            |row| row.get(0),
        ).map_err(|e| AlephError::config(format!("Failed to count facts: {}", e)))?;

        Ok(count as usize)
    }

    /// Update a fact's path
    pub async fn update_fact_path(&self, fact_id: &str, path: &str, parent_path: &str) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let rows = conn.execute(
            "UPDATE memory_facts SET path = ?1, parent_path = ?2 WHERE id = ?3",
            params![path, parent_path, fact_id],
        ).map_err(|e| AlephError::config(format!("Failed to update fact path: {}", e)))?;

        if rows == 0 {
            return Err(AlephError::config(format!("Fact not found: {}", fact_id)));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db() -> VectorDatabase {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_vfs_{}.db", uuid::Uuid::new_v4()));
        VectorDatabase::new(db_path).unwrap()
    }

    #[tokio::test]
    async fn test_list_path_children_empty() {
        let db = create_test_db();
        let entries = db.list_path_children("aleph://user/").await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_list_path_children_with_facts() {
        let db = create_test_db();

        let mut fact1 = MemoryFact::new("Prefers Rust".into(), FactType::Preference, vec![]);
        fact1.path = "aleph://user/preferences/coding/".into();
        fact1.parent_path = "aleph://user/preferences/".into();
        db.insert_fact(fact1).await.unwrap();

        let mut fact2 = MemoryFact::new("Prefers dark theme".into(), FactType::Preference, vec![]);
        fact2.path = "aleph://user/preferences/coding/".into();
        fact2.parent_path = "aleph://user/preferences/".into();
        db.insert_fact(fact2).await.unwrap();

        let mut fact3 = MemoryFact::new("Uses Neovim".into(), FactType::Preference, vec![]);
        fact3.path = "aleph://user/preferences/tools/".into();
        fact3.parent_path = "aleph://user/preferences/".into();
        db.insert_fact(fact3).await.unwrap();

        let entries = db.list_path_children("aleph://user/preferences/").await.unwrap();
        assert_eq!(entries.len(), 2);

        let coding = entries.iter().find(|e| e.name == "coding/").unwrap();
        assert_eq!(coding.fact_count, 2);
        assert!(coding.is_directory);

        let tools = entries.iter().find(|e| e.name == "tools/").unwrap();
        assert_eq!(tools.fact_count, 1);
    }

    #[tokio::test]
    async fn test_get_facts_by_path_prefix() {
        let db = create_test_db();

        let mut fact1 = MemoryFact::new("Fact A".into(), FactType::Preference, vec![]);
        fact1.path = "aleph://user/preferences/coding/".into();
        fact1.parent_path = "aleph://user/preferences/".into();
        db.insert_fact(fact1).await.unwrap();

        let mut fact2 = MemoryFact::new("Fact B".into(), FactType::Personal, vec![]);
        fact2.path = "aleph://user/personal/".into();
        fact2.parent_path = "aleph://user/".into();
        db.insert_fact(fact2).await.unwrap();

        let facts = db.get_facts_by_path_prefix("aleph://user/").await.unwrap();
        assert_eq!(facts.len(), 2);

        let facts = db.get_facts_by_path_prefix("aleph://user/preferences/").await.unwrap();
        assert_eq!(facts.len(), 1);
    }

    #[tokio::test]
    async fn test_get_l1_overview() {
        let db = create_test_db();

        let mut l1 = MemoryFact::new("Overview of preferences".into(), FactType::Other, vec![]);
        l1.path = "aleph://user/preferences/".into();
        l1.parent_path = "aleph://user/".into();
        l1.fact_source = FactSource::Summary;
        db.insert_fact(l1).await.unwrap();

        let overview = db.get_l1_overview("aleph://user/preferences/").await.unwrap();
        assert!(overview.is_some());
        assert_eq!(overview.unwrap().content, "Overview of preferences");

        let none = db.get_l1_overview("aleph://nonexistent/").await.unwrap();
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn test_count_facts_by_path() {
        let db = create_test_db();

        let mut fact1 = MemoryFact::new("A".into(), FactType::Preference, vec![]);
        fact1.path = "aleph://user/preferences/coding/".into();
        fact1.parent_path = "aleph://user/preferences/".into();
        db.insert_fact(fact1).await.unwrap();

        let mut fact2 = MemoryFact::new("B".into(), FactType::Preference, vec![]);
        fact2.path = "aleph://user/preferences/tools/".into();
        fact2.parent_path = "aleph://user/preferences/".into();
        db.insert_fact(fact2).await.unwrap();

        assert_eq!(db.count_facts_by_path("aleph://user/preferences/").await.unwrap(), 2);
        assert_eq!(db.count_facts_by_path("aleph://user/preferences/coding/").await.unwrap(), 1);
        assert_eq!(db.count_facts_by_path("aleph://agent/").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_update_fact_path() {
        let db = create_test_db();

        let fact = MemoryFact::new("Test fact".into(), FactType::Preference, vec![]);
        let fact_id = fact.id.clone();
        db.insert_fact(fact).await.unwrap();

        db.update_fact_path(&fact_id, "aleph://user/preferences/coding/", "aleph://user/preferences/").await.unwrap();

        let facts = db.get_facts_by_path_prefix("aleph://user/preferences/coding/").await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].id, fact_id);
    }
}
