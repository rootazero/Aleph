//! Search operations for memory facts

use crate::error::AlephError;
use crate::memory::context::{FactSource, FactSpecificity, FactType, MemoryFact, TemporalScope};
use crate::memory::database::core::VectorDatabase;
use crate::memory::NamespaceScope;

impl VectorDatabase {
    /// Search facts by vector similarity using sqlite-vec
    pub async fn search_facts(
        &self,
        query_embedding: &[f32],
        scope: NamespaceScope,
        limit: u32,
        include_invalid: bool,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let query_bytes = Self::serialize_embedding(query_embedding);

        // Get namespace filter
        let (namespace_filter, namespace_params) = scope.to_sql_filter();

        let query = if include_invalid {
            format!(
                r#"
            WITH vec_matches AS (
                SELECT rowid, distance
                FROM facts_vec
                WHERE embedding MATCH ?1
                ORDER BY distance
                LIMIT ?2
            )
            SELECT
                f.id, f.content, f.fact_type, f.embedding, f.source_memory_ids,
                f.created_at, f.updated_at, f.confidence, f.is_valid, f.invalidation_reason,
                f.specificity, f.temporal_scope,
                f.path, f.fact_source, f.content_hash, f.parent_path,
                1.0 / (1.0 + vm.distance) as score
            FROM memory_facts f
            INNER JOIN vec_matches vm ON f.rowid = vm.rowid
            WHERE {}
            ORDER BY vm.distance
            "#,
                namespace_filter
            )
        } else {
            format!(
                r#"
            WITH vec_matches AS (
                SELECT rowid, distance
                FROM facts_vec
                WHERE embedding MATCH ?1
                ORDER BY distance
                LIMIT ?2
            )
            SELECT
                f.id, f.content, f.fact_type, f.embedding, f.source_memory_ids,
                f.created_at, f.updated_at, f.confidence, f.is_valid, f.invalidation_reason,
                f.specificity, f.temporal_scope,
                f.path, f.fact_source, f.content_hash, f.parent_path,
                1.0 / (1.0 + vm.distance) as score
            FROM memory_facts f
            INNER JOIN vec_matches vm ON f.rowid = vm.rowid
            WHERE f.is_valid = 1 AND {}
            ORDER BY vm.distance
            "#,
                namespace_filter
            )
        };

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        // Build params: query_bytes, limit, namespace_params
        let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(query_bytes),
            Box::new(limit),
        ];
        for np in namespace_params {
            param_values.push(Box::new(np));
        }
        let params_refs: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let facts = stmt
            .query_map(params_refs.as_slice(), |row| {
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
                let path: String = row.get(12)?;
                let fact_source_str: String = row.get(13)?;
                let content_hash: String = row.get(14)?;
                let parent_path: String = row.get(15)?;
                let score: f64 = row.get(16)?;

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
                    decay_invalidated_at: None,
                    specificity: FactSpecificity::from_str_or_default(&specificity_str),
                    temporal_scope: TemporalScope::from_str_or_default(&temporal_scope_str),
                    similarity_score: Some(score as f32),
                    path,
                    fact_source: FactSource::from_str_or_default(&fact_source_str),
                    content_hash,
                    parent_path,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse fact rows: {}", e)))?;

        Ok(facts)
    }

    /// Get facts by type with namespace isolation
    pub async fn get_facts_by_type(
        &self,
        fact_type: FactType,
        scope: NamespaceScope,
        limit: u32,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Get namespace filter
        let (namespace_filter, namespace_params) = scope.to_sql_filter();

        // Build the WHERE clause with proper parameter numbering
        // ?1 = fact_type, ?2 = limit, ?3+ = namespace params (if any)
        let where_clause = if namespace_params.is_empty() {
            // Owner scope: no namespace filter needed
            format!("fact_type = ?1 AND is_valid = 1 AND {}", namespace_filter)
        } else {
            // Guest/Shared scope: namespace filter with parameter
            "fact_type = ?1 AND is_valid = 1 AND namespace = ?3".to_string()
        };

        let query = format!(
            r#"
                SELECT id, content, fact_type, embedding, source_memory_ids,
                       created_at, updated_at, confidence, is_valid, invalidation_reason,
                       specificity, temporal_scope,
                       path, fact_source, content_hash, parent_path
                FROM memory_facts
                WHERE {}
                ORDER BY updated_at DESC
                LIMIT ?2
                "#,
            where_clause
        );

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        // Build params: fact_type, limit, namespace_params
        let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(fact_type.as_str().to_string()),
            Box::new(limit),
        ];
        for np in namespace_params {
            param_values.push(Box::new(np));
        }
        let params_refs: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let facts = stmt
            .query_map(params_refs.as_slice(), |row| {
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
                let path: String = row.get(12)?;
                let fact_source_str: String = row.get(13)?;
                let content_hash: String = row.get(14)?;
                let parent_path: String = row.get(15)?;

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
                    decay_invalidated_at: None,
                    specificity: FactSpecificity::from_str_or_default(&specificity_str),
                    temporal_scope: TemporalScope::from_str_or_default(&temporal_scope_str),
                    similarity_score: None,
                    path,
                    fact_source: FactSource::from_str_or_default(&fact_source_str),
                    content_hash,
                    parent_path,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse fact rows: {}", e)))?;

        Ok(facts)
    }

    /// Find similar facts for conflict detection using sqlite-vec
    pub async fn find_similar_facts(
        &self,
        query_embedding: &[f32],
        scope: NamespaceScope,
        threshold: f32,
        exclude_id: Option<&str>,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let query_bytes = Self::serialize_embedding(query_embedding);

        // Fetch more candidates than needed, filter by threshold after
        let limit = 50u32;

        // Get namespace filter
        let (namespace_filter, namespace_params) = scope.to_sql_filter();

        let query = format!(
            r#"
                WITH vec_matches AS (
                    SELECT rowid, distance
                    FROM facts_vec
                    WHERE embedding MATCH ?1
                    ORDER BY distance
                    LIMIT ?2
                )
                SELECT
                    f.id, f.content, f.fact_type, f.embedding, f.source_memory_ids,
                    f.created_at, f.updated_at, f.confidence, f.is_valid, f.invalidation_reason,
                    f.specificity, f.temporal_scope,
                    f.path, f.fact_source, f.content_hash, f.parent_path,
                    1.0 / (1.0 + vm.distance) as score
                FROM memory_facts f
                INNER JOIN vec_matches vm ON f.rowid = vm.rowid
                WHERE f.is_valid = 1 AND {}
                ORDER BY vm.distance
                "#,
            namespace_filter
        );

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        // Build params: query_bytes, limit, namespace_params
        let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(query_bytes),
            Box::new(limit),
        ];
        for np in namespace_params {
            param_values.push(Box::new(np));
        }
        let params_refs: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let facts = stmt
            .query_map(params_refs.as_slice(), |row| {
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
                let path: String = row.get(12)?;
                let fact_source_str: String = row.get(13)?;
                let content_hash: String = row.get(14)?;
                let parent_path: String = row.get(15)?;
                let score: f64 = row.get(16)?;

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
                    decay_invalidated_at: None,
                    specificity: FactSpecificity::from_str_or_default(&specificity_str),
                    temporal_scope: TemporalScope::from_str_or_default(&temporal_scope_str),
                    similarity_score: Some(score as f32),
                    path,
                    fact_source: FactSource::from_str_or_default(&fact_source_str),
                    content_hash,
                    parent_path,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query similar facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse fact rows: {}", e)))?;

        // Filter by threshold and exclude_id
        let similar_facts: Vec<MemoryFact> = facts
            .into_iter()
            .filter(|fact| {
                if let Some(exclude) = exclude_id {
                    if fact.id == exclude {
                        return false;
                    }
                }
                fact.similarity_score.unwrap_or(0.0) >= threshold
            })
            .collect();

        Ok(similar_facts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::database::core::VectorDatabase;
    use crate::memory::NamespaceScope;
    use tempfile::TempDir;

    /// Helper to create a test database
    async fn create_test_db() -> (VectorDatabase, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();
        (db, temp_dir)
    }

    /// Helper to insert a test fact with namespace
    async fn insert_fact_with_namespace(
        db: &VectorDatabase,
        content: &str,
        fact_type: FactType,
        namespace: &str,
    ) -> String {
        let conn = db.conn.lock().unwrap();
        let fact_id = uuid::Uuid::new_v4().to_string();
        let source_ids_json = serde_json::to_string(&Vec::<String>::new()).unwrap();

        conn.execute(
            r#"
            INSERT INTO memory_facts (
                id, content, fact_type, source_memory_ids, created_at, updated_at,
                confidence, is_valid, specificity, temporal_scope, namespace
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#,
            rusqlite::params![
                fact_id,
                content,
                fact_type.as_str(),
                source_ids_json,
                chrono::Utc::now().timestamp(),
                chrono::Utc::now().timestamp(),
                1.0,
                1,
                "general",
                "permanent",
                namespace,
            ],
        ).unwrap();

        fact_id
    }

    #[tokio::test]
    async fn test_get_facts_by_type_owner_sees_all() {
        let (db, _temp_dir) = create_test_db().await;

        // Insert facts in different namespaces
        insert_fact_with_namespace(&db, "Owner fact", FactType::Preference, "owner").await;
        insert_fact_with_namespace(&db, "Guest1 fact", FactType::Preference, "guest:guest1").await;
        insert_fact_with_namespace(&db, "Guest2 fact", FactType::Preference, "guest:guest2").await;

        // Owner should see all facts (no namespace filter)
        let facts = db
            .get_facts_by_type(FactType::Preference, NamespaceScope::Owner, 100)
            .await
            .unwrap();

        assert_eq!(facts.len(), 3, "Owner should see all 3 facts");
    }

    #[tokio::test]
    async fn test_get_facts_by_type_guest_isolation() {
        let (db, _temp_dir) = create_test_db().await;

        // Insert facts in different namespaces
        insert_fact_with_namespace(&db, "Owner fact", FactType::Preference, "owner").await;
        insert_fact_with_namespace(&db, "Guest1 fact", FactType::Preference, "guest:guest1").await;
        insert_fact_with_namespace(&db, "Guest2 fact", FactType::Preference, "guest:guest2").await;

        // Guest1 should only see their own facts
        let guest1_facts = db
            .get_facts_by_type(
                FactType::Preference,
                NamespaceScope::Guest("guest1".to_string()),
                100,
            )
            .await
            .unwrap();

        assert_eq!(guest1_facts.len(), 1, "Guest1 should only see 1 fact");
        assert!(
            guest1_facts[0].content.contains("Guest1"),
            "Guest1 should only see their own fact"
        );

        // Guest2 should only see their own facts
        let guest2_facts = db
            .get_facts_by_type(
                FactType::Preference,
                NamespaceScope::Guest("guest2".to_string()),
                100,
            )
            .await
            .unwrap();

        assert_eq!(guest2_facts.len(), 1, "Guest2 should only see 1 fact");
        assert!(
            guest2_facts[0].content.contains("Guest2"),
            "Guest2 should only see their own fact"
        );
    }

    #[tokio::test]
    async fn test_get_facts_by_type_guest_cannot_see_owner() {
        let (db, _temp_dir) = create_test_db().await;

        // Insert owner fact
        insert_fact_with_namespace(&db, "Owner secret", FactType::Preference, "owner").await;

        // Guest should not see owner's fact
        let guest_facts = db
            .get_facts_by_type(
                FactType::Preference,
                NamespaceScope::Guest("guest1".to_string()),
                100,
            )
            .await
            .unwrap();

        assert_eq!(guest_facts.len(), 0, "Guest should not see owner's facts");
    }

    #[tokio::test]
    async fn test_get_facts_by_type_shared_namespace() {
        let (db, _temp_dir) = create_test_db().await;

        // Insert facts in different namespaces
        insert_fact_with_namespace(&db, "Owner fact", FactType::Preference, "owner").await;
        insert_fact_with_namespace(&db, "Shared fact", FactType::Preference, "shared").await;
        insert_fact_with_namespace(&db, "Guest fact", FactType::Preference, "guest:guest1").await;

        // Shared scope should only see shared facts
        let shared_facts = db
            .get_facts_by_type(FactType::Preference, NamespaceScope::Shared, 100)
            .await
            .unwrap();

        assert_eq!(shared_facts.len(), 1, "Shared scope should only see 1 fact");
        assert!(
            shared_facts[0].content.contains("Shared"),
            "Should only see shared fact"
        );
    }
}
