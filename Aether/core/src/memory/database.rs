/// Vector database wrapper using SQLite + sqlite-vec
///
/// This module provides storage and retrieval functionality for memory embeddings
/// using SQLite as the backend with vector similarity search capabilities.
use crate::error::AetherError;
use crate::memory::context::{
    CompressionSession, ContextAnchor, FactStats, FactType, MemoryEntry, MemoryFact,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Vector database for storing and searching memory embeddings
pub struct VectorDatabase {
    conn: Arc<Mutex<Connection>>,
    db_path: PathBuf,
}

/// Current embedding dimension (bge-small-zh-v1.5)
const CURRENT_EMBEDDING_DIM: u32 = 512;

impl VectorDatabase {
    /// Initialize vector database with schema
    ///
    /// Includes migration logic for embedding dimension changes.
    /// When embedding dimension changes (e.g., 384 -> 512), old data is cleared.
    pub fn new(db_path: PathBuf) -> Result<Self, AetherError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AetherError::config(format!("Failed to create database directory: {}", e))
            })?;
        }

        let conn = Connection::open(&db_path)
            .map_err(|e| AetherError::config(format!("Failed to open database: {}", e)))?;

        // Check if migration is needed (dimension change)
        let needs_migration = Self::check_needs_migration(&conn)?;

        if needs_migration {
            // Drop old memories table for dimension migration
            conn.execute_batch("DROP TABLE IF EXISTS memories;")
                .map_err(|e| AetherError::config(format!("Failed to drop old table: {}", e)))?;

            tracing::info!(
                old_dim = 384,
                new_dim = CURRENT_EMBEDDING_DIM,
                "Cleared memories table for embedding dimension migration"
            );
        }

        // Create schema with version metadata
        conn.execute_batch(
            r#"
            -- Metadata table for schema versioning
            CREATE TABLE IF NOT EXISTS schema_info (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Main memories table
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                app_bundle_id TEXT NOT NULL,
                window_title TEXT NOT NULL,
                user_input TEXT NOT NULL,
                ai_output TEXT NOT NULL,
                embedding BLOB NOT NULL,
                timestamp INTEGER NOT NULL
            );

            -- Index for fast context-based filtering
            CREATE INDEX IF NOT EXISTS idx_context ON memories(app_bundle_id, window_title);

            -- Index for timestamp-based queries (retention policy)
            CREATE INDEX IF NOT EXISTS idx_timestamp ON memories(timestamp);

            -- ================================================================
            -- Memory Compression: Fact Storage Tables
            -- ================================================================

            -- Compressed memory facts table
            CREATE TABLE IF NOT EXISTS memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                fact_type TEXT NOT NULL DEFAULT 'other',
                embedding BLOB,
                source_memory_ids TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                confidence REAL NOT NULL DEFAULT 1.0,
                is_valid INTEGER NOT NULL DEFAULT 1,
                invalidation_reason TEXT
            );

            -- Index for fact type queries
            CREATE INDEX IF NOT EXISTS idx_facts_type ON memory_facts(fact_type);

            -- Index for valid facts queries
            CREATE INDEX IF NOT EXISTS idx_facts_valid ON memory_facts(is_valid);

            -- Index for timestamp-based queries
            CREATE INDEX IF NOT EXISTS idx_facts_updated ON memory_facts(updated_at);

            -- Compression session audit table
            CREATE TABLE IF NOT EXISTS compression_sessions (
                id TEXT PRIMARY KEY,
                source_memory_ids TEXT NOT NULL,
                extracted_fact_ids TEXT NOT NULL,
                compressed_at INTEGER NOT NULL,
                provider_used TEXT NOT NULL,
                duration_ms INTEGER NOT NULL
            );

            -- Index for compression history queries
            CREATE INDEX IF NOT EXISTS idx_compression_time ON compression_sessions(compressed_at);
            "#,
        )
        .map_err(|e| AetherError::config(format!("Failed to create schema: {}", e)))?;

        // Update embedding dimension in schema_info
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('embedding_dimension', ?1)",
            params![CURRENT_EMBEDDING_DIM.to_string()],
        )
        .map_err(|e| AetherError::config(format!("Failed to update schema_info: {}", e)))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    /// Check if database needs migration due to dimension change
    fn check_needs_migration(conn: &Connection) -> Result<bool, AetherError> {
        // Check if schema_info table exists
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_info'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !table_exists {
            // Check if memories table exists (old database without schema_info)
            let memories_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memories'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);

            // If old memories table exists but no schema_info, needs migration
            return Ok(memories_exists);
        }

        // Check current dimension in schema_info
        let current_dimension: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        match current_dimension {
            Some(dim) if dim == CURRENT_EMBEDDING_DIM.to_string() => Ok(false), // Already at current dimension
            Some(dim) => {
                tracing::info!(
                    stored_dim = %dim,
                    current_dim = CURRENT_EMBEDDING_DIM,
                    "Embedding dimension mismatch detected"
                );
                Ok(true) // Needs migration (different dimension)
            }
            None => Ok(true), // No dimension stored, needs migration
        }
    }

    /// Insert memory entry into database
    pub async fn insert_memory(&self, memory: MemoryEntry) -> Result<(), AetherError> {
        let embedding = memory
            .embedding
            .ok_or_else(|| AetherError::config("Cannot insert memory without embedding"))?;

        // Serialize embedding to bytes
        let embedding_bytes = Self::serialize_embedding(&embedding);

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO memories (id, app_bundle_id, window_title, user_input, ai_output, embedding, timestamp)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                memory.id,
                memory.context.app_bundle_id,
                memory.context.window_title,
                memory.user_input,
                memory.ai_output,
                embedding_bytes,
                memory.context.timestamp,
            ],
        )
        .map_err(|e| AetherError::config(format!("Failed to insert memory: {}", e)))?;

        Ok(())
    }

    /// Search memories by context and embedding similarity
    pub async fn search_memories(
        &self,
        app_bundle_id: &str,
        window_title: &str,
        query_embedding: &[f32],
        limit: u32,
    ) -> Result<Vec<MemoryEntry>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Query memories matching the context
        // If app_bundle_id or window_title is empty, treat it as "any value"
        let mut stmt = conn
            .prepare(
                r#"
            SELECT id, app_bundle_id, window_title, user_input, ai_output, embedding, timestamp
            FROM memories
            WHERE (?1 = '' OR app_bundle_id = ?1)
              AND (?2 = '' OR window_title = ?2)
            ORDER BY timestamp DESC
            LIMIT ?3
            "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let memories = stmt
            .query_map(params![app_bundle_id, window_title, limit], |row| {
                let id: String = row.get(0)?;
                let app_id: String = row.get(1)?;
                let window: String = row.get(2)?;
                let user_input: String = row.get(3)?;
                let ai_output: String = row.get(4)?;
                let embedding_bytes: Vec<u8> = row.get(5)?;
                let timestamp: i64 = row.get(6)?;

                let embedding = Self::deserialize_embedding(&embedding_bytes);

                Ok(MemoryEntry {
                    id,
                    context: ContextAnchor {
                        app_bundle_id: app_id,
                        window_title: window,
                        timestamp,
                    },
                    user_input,
                    ai_output,
                    embedding: Some(embedding),
                    similarity_score: None,
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query memories: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse memory rows: {}", e)))?;

        // Calculate similarity scores and sort by score
        let mut scored_memories: Vec<MemoryEntry> = memories
            .into_iter()
            .filter_map(|mut memory| {
                if let Some(ref emb) = memory.embedding {
                    let score = Self::cosine_similarity(query_embedding, emb);
                    memory.similarity_score = Some(score);
                    Some(memory)
                } else {
                    None
                }
            })
            .collect();

        // Sort by similarity score (descending)
        scored_memories.sort_by(|a, b| {
            b.similarity_score
                .partial_cmp(&a.similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top K results
        scored_memories.truncate(limit as usize);

        Ok(scored_memories)
    }

    /// Get recent memories without embedding similarity search
    ///
    /// Used for AI-based memory retrieval where the AI selects relevant memories
    /// instead of using vector similarity. Optionally filters out specified user inputs
    /// (for deduplication with current conversation session).
    pub async fn get_recent_memories(
        &self,
        app_bundle_id: &str,
        window_title: &str,
        limit: u32,
        exclude_user_inputs: &[String],
    ) -> Result<Vec<MemoryEntry>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Query recent memories matching context
        let mut stmt = conn
            .prepare(
                r#"
            SELECT id, app_bundle_id, window_title, user_input, ai_output, embedding, timestamp
            FROM memories
            WHERE (?1 = '' OR app_bundle_id = ?1)
              AND (?2 = '' OR window_title = ?2)
            ORDER BY timestamp DESC
            LIMIT ?3
            "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let memories = stmt
            .query_map(params![app_bundle_id, window_title, limit * 2], |row| {
                // Fetch more than limit to account for filtering
                let id: String = row.get(0)?;
                let app_id: String = row.get(1)?;
                let window: String = row.get(2)?;
                let user_input: String = row.get(3)?;
                let ai_output: String = row.get(4)?;
                let embedding_bytes: Vec<u8> = row.get(5)?;
                let timestamp: i64 = row.get(6)?;

                let embedding = Self::deserialize_embedding(&embedding_bytes);

                Ok(MemoryEntry {
                    id,
                    context: ContextAnchor {
                        app_bundle_id: app_id,
                        window_title: window,
                        timestamp,
                    },
                    user_input,
                    ai_output,
                    embedding: Some(embedding),
                    similarity_score: None,
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query memories: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse memory rows: {}", e)))?;

        // Filter out excluded user inputs (deduplication)
        let filtered: Vec<MemoryEntry> = if exclude_user_inputs.is_empty() {
            memories
        } else {
            memories
                .into_iter()
                .filter(|m| {
                    !exclude_user_inputs
                        .iter()
                        .any(|ex| m.user_input.contains(ex))
                })
                .collect()
        };

        // Take only up to limit after filtering
        Ok(filtered.into_iter().take(limit as usize).collect())
    }

    /// Delete memory by ID
    pub async fn delete_memory(&self, id: &str) -> Result<(), AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(|e| AetherError::config(format!("Failed to delete memory: {}", e)))?;

        if rows_affected == 0 {
            return Err(AetherError::config(format!("Memory not found: {}", id)));
        }

        Ok(())
    }

    /// Clear memories with optional filters
    pub async fn clear_memories(
        &self,
        app_bundle_id: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<u64, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let (query, params_vec): (String, Vec<&str>) = match (app_bundle_id, window_title) {
            (Some(app), Some(window)) => (
                "DELETE FROM memories WHERE app_bundle_id = ?1 AND window_title = ?2".to_string(),
                vec![app, window],
            ),
            (Some(app), None) => (
                "DELETE FROM memories WHERE app_bundle_id = ?1".to_string(),
                vec![app],
            ),
            (None, Some(window)) => (
                "DELETE FROM memories WHERE window_title = ?1".to_string(),
                vec![window],
            ),
            (None, None) => ("DELETE FROM memories".to_string(), vec![]),
        };

        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();

        let rows_affected = conn
            .execute(&query, params_refs.as_slice())
            .map_err(|e| AetherError::config(format!("Failed to clear memories: {}", e)))?;

        Ok(rows_affected as u64)
    }

    /// Get database statistics
    pub async fn get_stats(&self) -> Result<MemoryStats, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Count total memories
        let total_memories: u64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .unwrap_or(0);

        // Count distinct apps
        let total_apps: u64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT app_bundle_id) FROM memories",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Get oldest and newest timestamps
        let oldest_memory_timestamp: i64 = conn
            .query_row("SELECT MIN(timestamp) FROM memories", [], |row| row.get(0))
            .optional()
            .unwrap_or(None)
            .unwrap_or(0);

        let newest_memory_timestamp: i64 = conn
            .query_row("SELECT MAX(timestamp) FROM memories", [], |row| row.get(0))
            .optional()
            .unwrap_or(None)
            .unwrap_or(0);

        // Calculate database size
        let database_size_mb = std::fs::metadata(&self.db_path)
            .map(|m| m.len() as f64 / (1024.0 * 1024.0))
            .unwrap_or(0.0);

        Ok(MemoryStats {
            total_memories,
            total_apps,
            database_size_mb,
            oldest_memory_timestamp,
            newest_memory_timestamp,
        })
    }

    /// Delete memories older than timestamp (for retention policy)
    pub async fn delete_older_than(&self, cutoff_timestamp: i64) -> Result<u64, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute(
                "DELETE FROM memories WHERE timestamp < ?1",
                params![cutoff_timestamp],
            )
            .map_err(|e| AetherError::config(format!("Failed to delete old memories: {}", e)))?;

        Ok(rows_affected as u64)
    }

    /// Get list of unique app bundle IDs with memory counts
    pub async fn get_app_list(&self) -> Result<Vec<(String, u64)>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT app_bundle_id, COUNT(*) as count
                FROM memories
                GROUP BY app_bundle_id
                ORDER BY count DESC, app_bundle_id ASC
                "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let apps = stmt
            .query_map([], |row| {
                let app_bundle_id: String = row.get(0)?;
                let count: u64 = row.get(1)?;
                Ok((app_bundle_id, count))
            })
            .map_err(|e| AetherError::config(format!("Failed to query app list: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse app rows: {}", e)))?;

        Ok(apps)
    }

    // ========================================================================
    // Memory Compression: Fact Operations
    // ========================================================================

    /// Insert a memory fact into the database
    pub async fn insert_fact(&self, fact: MemoryFact) -> Result<(), AetherError> {
        let embedding_bytes = fact
            .embedding
            .as_ref()
            .map(|e| Self::serialize_embedding(e));

        let source_ids_json = serde_json::to_string(&fact.source_memory_ids)
            .map_err(|e| AetherError::config(format!("Failed to serialize source_ids: {}", e)))?;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO memory_facts (
                id, content, fact_type, embedding, source_memory_ids,
                created_at, updated_at, confidence, is_valid, invalidation_reason
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                fact.id,
                fact.content,
                fact.fact_type.as_str(),
                embedding_bytes,
                source_ids_json,
                fact.created_at,
                fact.updated_at,
                fact.confidence,
                fact.is_valid as i32,
                fact.invalidation_reason,
            ],
        )
        .map_err(|e| AetherError::config(format!("Failed to insert fact: {}", e)))?;

        Ok(())
    }

    /// Insert multiple facts in a batch
    pub async fn insert_facts(&self, facts: Vec<MemoryFact>) -> Result<(), AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        for fact in facts {
            let embedding_bytes = fact
                .embedding
                .as_ref()
                .map(|e| Self::serialize_embedding(e));

            let source_ids_json = serde_json::to_string(&fact.source_memory_ids)
                .map_err(|e| AetherError::config(format!("Failed to serialize source_ids: {}", e)))?;

            conn.execute(
                r#"
                INSERT INTO memory_facts (
                    id, content, fact_type, embedding, source_memory_ids,
                    created_at, updated_at, confidence, is_valid, invalidation_reason
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                "#,
                params![
                    fact.id,
                    fact.content,
                    fact.fact_type.as_str(),
                    embedding_bytes,
                    source_ids_json,
                    fact.created_at,
                    fact.updated_at,
                    fact.confidence,
                    fact.is_valid as i32,
                    fact.invalidation_reason,
                ],
            )
            .map_err(|e| AetherError::config(format!("Failed to insert fact: {}", e)))?;
        }

        Ok(())
    }

    /// Search facts by vector similarity
    pub async fn search_facts(
        &self,
        query_embedding: &[f32],
        limit: u32,
        include_invalid: bool,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let query = if include_invalid {
            r#"
            SELECT id, content, fact_type, embedding, source_memory_ids,
                   created_at, updated_at, confidence, is_valid, invalidation_reason
            FROM memory_facts
            WHERE embedding IS NOT NULL
            "#
        } else {
            r#"
            SELECT id, content, fact_type, embedding, source_memory_ids,
                   created_at, updated_at, confidence, is_valid, invalidation_reason
            FROM memory_facts
            WHERE embedding IS NOT NULL AND is_valid = 1
            "#
        };

        let mut stmt = conn
            .prepare(query)
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let facts = stmt
            .query_map([], |row| {
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

                let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));
                let source_memory_ids: Vec<String> =
                    serde_json::from_str(&source_ids_json).unwrap_or_default();

                Ok(MemoryFact {
                    id,
                    content,
                    fact_type: FactType::from_str(&fact_type_str),
                    embedding,
                    source_memory_ids,
                    created_at,
                    updated_at,
                    confidence,
                    is_valid: is_valid != 0,
                    invalidation_reason,
                    similarity_score: None,
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse fact rows: {}", e)))?;

        // Calculate similarity scores and sort
        let mut scored_facts: Vec<MemoryFact> = facts
            .into_iter()
            .filter_map(|mut fact| {
                if let Some(ref emb) = fact.embedding {
                    let score = Self::cosine_similarity(query_embedding, emb);
                    fact.similarity_score = Some(score);
                    Some(fact)
                } else {
                    None
                }
            })
            .collect();

        scored_facts.sort_by(|a, b| {
            b.similarity_score
                .partial_cmp(&a.similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        scored_facts.truncate(limit as usize);

        Ok(scored_facts)
    }

    /// Get facts by type
    pub async fn get_facts_by_type(
        &self,
        fact_type: FactType,
        limit: u32,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, content, fact_type, embedding, source_memory_ids,
                       created_at, updated_at, confidence, is_valid, invalidation_reason
                FROM memory_facts
                WHERE fact_type = ?1 AND is_valid = 1
                ORDER BY updated_at DESC
                LIMIT ?2
                "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let facts = stmt
            .query_map(params![fact_type.as_str(), limit], |row| {
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

                let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));
                let source_memory_ids: Vec<String> =
                    serde_json::from_str(&source_ids_json).unwrap_or_default();

                Ok(MemoryFact {
                    id,
                    content,
                    fact_type: FactType::from_str(&fact_type_str),
                    embedding,
                    source_memory_ids,
                    created_at,
                    updated_at,
                    confidence,
                    is_valid: is_valid != 0,
                    invalidation_reason,
                    similarity_score: None,
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse fact rows: {}", e)))?;

        Ok(facts)
    }

    /// Invalidate a fact (soft delete)
    pub async fn invalidate_fact(&self, fact_id: &str, reason: &str) -> Result<(), AetherError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute(
                r#"
                UPDATE memory_facts
                SET is_valid = 0, invalidation_reason = ?1, updated_at = ?2
                WHERE id = ?3
                "#,
                params![reason, now, fact_id],
            )
            .map_err(|e| AetherError::config(format!("Failed to invalidate fact: {}", e)))?;

        if rows_affected == 0 {
            return Err(AetherError::config(format!("Fact not found: {}", fact_id)));
        }

        Ok(())
    }

    /// Find similar facts for conflict detection
    pub async fn find_similar_facts(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        exclude_id: Option<&str>,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, content, fact_type, embedding, source_memory_ids,
                       created_at, updated_at, confidence, is_valid, invalidation_reason
                FROM memory_facts
                WHERE embedding IS NOT NULL AND is_valid = 1
                "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let facts = stmt
            .query_map([], |row| {
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

                let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));
                let source_memory_ids: Vec<String> =
                    serde_json::from_str(&source_ids_json).unwrap_or_default();

                Ok(MemoryFact {
                    id,
                    content,
                    fact_type: FactType::from_str(&fact_type_str),
                    embedding,
                    source_memory_ids,
                    created_at,
                    updated_at,
                    confidence,
                    is_valid: is_valid != 0,
                    invalidation_reason,
                    similarity_score: None,
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse fact rows: {}", e)))?;

        // Filter by similarity threshold and exclude_id
        let similar_facts: Vec<MemoryFact> = facts
            .into_iter()
            .filter_map(|mut fact| {
                // Exclude specified ID
                if let Some(ex_id) = exclude_id {
                    if fact.id == ex_id {
                        return None;
                    }
                }

                if let Some(ref emb) = fact.embedding {
                    let score = Self::cosine_similarity(query_embedding, emb);
                    if score >= threshold {
                        fact.similarity_score = Some(score);
                        return Some(fact);
                    }
                }
                None
            })
            .collect();

        Ok(similar_facts)
    }

    /// Get uncompressed memories (since last compression)
    pub async fn get_uncompressed_memories(
        &self,
        since_timestamp: i64,
        limit: u32,
    ) -> Result<Vec<MemoryEntry>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, app_bundle_id, window_title, user_input, ai_output, embedding, timestamp
                FROM memories
                WHERE timestamp > ?1
                ORDER BY timestamp ASC
                LIMIT ?2
                "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let memories = stmt
            .query_map(params![since_timestamp, limit], |row| {
                let id: String = row.get(0)?;
                let app_id: String = row.get(1)?;
                let window: String = row.get(2)?;
                let user_input: String = row.get(3)?;
                let ai_output: String = row.get(4)?;
                let embedding_bytes: Vec<u8> = row.get(5)?;
                let timestamp: i64 = row.get(6)?;

                let embedding = Self::deserialize_embedding(&embedding_bytes);

                Ok(MemoryEntry {
                    id,
                    context: ContextAnchor {
                        app_bundle_id: app_id,
                        window_title: window,
                        timestamp,
                    },
                    user_input,
                    ai_output,
                    embedding: Some(embedding),
                    similarity_score: None,
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query memories: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse memory rows: {}", e)))?;

        Ok(memories)
    }

    /// Set the last compression timestamp
    pub async fn set_last_compression_timestamp(&self, timestamp: i64) -> Result<(), AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('last_compression_timestamp', ?1)",
            params![timestamp.to_string()],
        )
        .map_err(|e| AetherError::config(format!("Failed to update compression timestamp: {}", e)))?;
        Ok(())
    }

    /// Get the last compression timestamp
    pub async fn get_last_compression_timestamp(&self) -> Result<Option<i64>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let timestamp: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'last_compression_timestamp'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        Ok(timestamp.and_then(|t| t.parse::<i64>().ok()))
    }

    /// Record a compression session
    pub async fn record_compression_session(
        &self,
        session: CompressionSession,
    ) -> Result<(), AetherError> {
        let source_ids_json = serde_json::to_string(&session.source_memory_ids)
            .map_err(|e| AetherError::config(format!("Failed to serialize source_ids: {}", e)))?;
        let extracted_ids_json = serde_json::to_string(&session.extracted_fact_ids)
            .map_err(|e| AetherError::config(format!("Failed to serialize extracted_ids: {}", e)))?;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO compression_sessions (
                id, source_memory_ids, extracted_fact_ids,
                compressed_at, provider_used, duration_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                session.id,
                source_ids_json,
                extracted_ids_json,
                session.compressed_at,
                session.provider_used,
                session.duration_ms as i64,
            ],
        )
        .map_err(|e| AetherError::config(format!("Failed to record compression session: {}", e)))?;

        Ok(())
    }

    /// Get fact statistics
    pub async fn get_fact_stats(&self) -> Result<FactStats, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Total facts
        let total_facts: u64 = conn
            .query_row("SELECT COUNT(*) FROM memory_facts", [], |row| row.get(0))
            .unwrap_or(0);

        // Valid facts
        let valid_facts: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Facts by type
        let mut facts_by_type = std::collections::HashMap::new();
        let mut stmt = conn
            .prepare(
                "SELECT fact_type, COUNT(*) FROM memory_facts WHERE is_valid = 1 GROUP BY fact_type",
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                let fact_type: String = row.get(0)?;
                let count: u64 = row.get(1)?;
                Ok((fact_type, count))
            })
            .map_err(|e| AetherError::config(format!("Failed to query fact types: {}", e)))?;

        for row in rows {
            if let Ok((fact_type, count)) = row {
                facts_by_type.insert(fact_type, count);
            }
        }

        // Timestamps
        let oldest_fact_timestamp: Option<i64> = conn
            .query_row(
                "SELECT MIN(created_at) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        let newest_fact_timestamp: Option<i64> = conn
            .query_row(
                "SELECT MAX(created_at) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        Ok(FactStats {
            total_facts,
            valid_facts,
            facts_by_type,
            oldest_fact_timestamp,
            newest_fact_timestamp,
        })
    }

    /// Delete old facts based on retention policy
    pub async fn delete_old_facts(&self, cutoff_timestamp: i64) -> Result<u64, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute(
                "DELETE FROM memory_facts WHERE created_at < ?1",
                params![cutoff_timestamp],
            )
            .map_err(|e| AetherError::config(format!("Failed to delete old facts: {}", e)))?;

        Ok(rows_affected as u64)
    }

    /// Clear all facts (for testing or reset)
    pub async fn clear_facts(&self) -> Result<u64, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute("DELETE FROM memory_facts", [])
            .map_err(|e| AetherError::config(format!("Failed to clear facts: {}", e)))?;

        Ok(rows_affected as u64)
    }

    /// Serialize embedding vector to bytes (f32 array -> bytes)
    fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Deserialize embedding from bytes
    fn deserialize_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    /// Calculate cosine similarity between two vectors using SIMD optimization.
    ///
    /// Delegates to the SIMD module which automatically selects the best
    /// implementation for the current platform (NEON on Apple Silicon,
    /// AVX/SSE on x86_64).
    #[inline]
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        super::simd::cosine_similarity(a, b)
    }
}

/// Memory database statistics
#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    pub total_memories: u64,
    pub total_apps: u64,
    pub database_size_mb: f64,
    pub oldest_memory_timestamp: i64,
    pub newest_memory_timestamp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db() -> VectorDatabase {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_memory_{}.db", uuid::Uuid::new_v4()));
        VectorDatabase::new(db_path).unwrap()
    }

    fn create_test_memory(id: &str, app: &str, window: &str, embedding: Vec<f32>) -> MemoryEntry {
        MemoryEntry::with_embedding(
            id.to_string(),
            ContextAnchor::now(app.to_string(), window.to_string()),
            "test user input".to_string(),
            "test ai output".to_string(),
            embedding,
        )
    }

    #[tokio::test]
    async fn test_database_creation() {
        let db = create_test_db();
        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 0);
    }

    #[tokio::test]
    async fn test_insert_and_retrieve() {
        let db = create_test_db();
        let embedding = vec![0.1, 0.2, 0.3, 0.4];
        let memory =
            create_test_memory("test-id", "com.apple.Notes", "Test.txt", embedding.clone());

        db.insert_memory(memory).await.unwrap();

        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 1);
        assert_eq!(stats.total_apps, 1);
    }

    #[tokio::test]
    async fn test_search_memories_by_context() {
        let db = create_test_db();
        let embedding1 = vec![1.0, 0.0, 0.0, 0.0];
        let embedding2 = vec![0.0, 1.0, 0.0, 0.0];

        let memory1 = create_test_memory("id1", "com.apple.Notes", "Doc1.txt", embedding1.clone());
        let memory2 = create_test_memory("id2", "com.apple.Notes", "Doc1.txt", embedding2.clone());

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();

        // Search with query similar to embedding1
        let query = vec![0.9, 0.1, 0.0, 0.0];
        let results = db
            .search_memories("com.apple.Notes", "Doc1.txt", &query, 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        // First result should have higher similarity to embedding1
        assert!(results[0].similarity_score.unwrap() > results[1].similarity_score.unwrap());
    }

    #[tokio::test]
    async fn test_context_isolation() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];

        let memory1 = create_test_memory("id1", "com.apple.Notes", "Doc1.txt", embedding.clone());
        let memory2 = create_test_memory("id2", "com.apple.Notes", "Doc2.txt", embedding.clone());
        let memory3 =
            create_test_memory("id3", "com.apple.TextEdit", "Doc1.txt", embedding.clone());

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();
        db.insert_memory(memory3).await.unwrap();

        // Should only return memories from Notes + Doc1.txt
        let results = db
            .search_memories("com.apple.Notes", "Doc1.txt", &embedding, 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "id1");
    }

    #[tokio::test]
    async fn test_delete_memory() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];
        let memory = create_test_memory("test-id", "com.apple.Notes", "Test.txt", embedding);

        db.insert_memory(memory).await.unwrap();
        assert_eq!(db.get_stats().await.unwrap().total_memories, 1);

        db.delete_memory("test-id").await.unwrap();
        assert_eq!(db.get_stats().await.unwrap().total_memories, 0);
    }

    #[tokio::test]
    async fn test_clear_memories_all() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];

        for i in 0..5 {
            let memory = create_test_memory(
                &format!("id{}", i),
                "com.apple.Notes",
                "Test.txt",
                embedding.clone(),
            );
            db.insert_memory(memory).await.unwrap();
        }

        assert_eq!(db.get_stats().await.unwrap().total_memories, 5);

        let deleted = db.clear_memories(None, None).await.unwrap();
        assert_eq!(deleted, 5);
        assert_eq!(db.get_stats().await.unwrap().total_memories, 0);
    }

    #[tokio::test]
    async fn test_clear_memories_by_app() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];

        let memory1 = create_test_memory("id1", "com.apple.Notes", "Test.txt", embedding.clone());
        let memory2 =
            create_test_memory("id2", "com.apple.TextEdit", "Test.txt", embedding.clone());

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();

        let deleted = db
            .clear_memories(Some("com.apple.Notes"), None)
            .await
            .unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(db.get_stats().await.unwrap().total_memories, 1);
    }

    #[tokio::test]
    async fn test_delete_older_than() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];

        // Create memory with old timestamp
        let old_memory = MemoryEntry::with_embedding(
            "old-id".to_string(),
            ContextAnchor::with_timestamp(
                "com.apple.Notes".to_string(),
                "Test.txt".to_string(),
                1000000,
            ),
            "old input".to_string(),
            "old output".to_string(),
            embedding.clone(),
        );

        // Create memory with recent timestamp
        let new_memory = create_test_memory("new-id", "com.apple.Notes", "Test.txt", embedding);

        db.insert_memory(old_memory).await.unwrap();
        db.insert_memory(new_memory).await.unwrap();

        // Delete memories older than 2000000
        let deleted = db.delete_older_than(2000000).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(db.get_stats().await.unwrap().total_memories, 1);
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((VectorDatabase::cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![1.0, 0.0, 0.0];
        let d = vec![0.0, 1.0, 0.0];
        assert!((VectorDatabase::cosine_similarity(&c, &d) - 0.0).abs() < 0.001);

        let e = vec![1.0, 1.0, 0.0];
        let f = vec![1.0, 0.0, 0.0];
        let similarity = VectorDatabase::cosine_similarity(&e, &f);
        assert!(similarity > 0.7 && similarity < 0.8); // ~0.707
    }

    #[test]
    fn test_embedding_serialization() {
        let embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let bytes = VectorDatabase::serialize_embedding(&embedding);
        let deserialized = VectorDatabase::deserialize_embedding(&bytes);

        assert_eq!(embedding.len(), deserialized.len());
        for (a, b) in embedding.iter().zip(deserialized.iter()) {
            assert!((a - b).abs() < 0.0001);
        }
    }

    #[tokio::test]
    async fn test_error_handling_invalid_memory_id() {
        let db = create_test_db();

        // Try to delete non-existent memory
        let result = db.delete_memory("non-existent-id").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Memory not found"));
    }

    #[tokio::test]
    async fn test_search_memories_with_empty_embedding() {
        let db = create_test_db();

        // Search with empty embedding should return empty results
        let results = db
            .search_memories("com.apple.Notes", "Test.txt", &Vec::new(), 5)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_memories_zero_limit() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];
        let memory = create_test_memory("id1", "com.apple.Notes", "Test.txt", embedding.clone());

        db.insert_memory(memory).await.unwrap();

        let results = db
            .search_memories("com.apple.Notes", "Test.txt", &embedding, 0)
            .await
            .unwrap();

        // Zero limit should return no results
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_get_stats_empty_database() {
        let db = create_test_db();
        let stats = db.get_stats().await.unwrap();

        assert_eq!(stats.total_memories, 0);
        assert_eq!(stats.total_apps, 0);
        assert_eq!(stats.oldest_memory_timestamp, 0);
        assert_eq!(stats.newest_memory_timestamp, 0);
    }

    #[tokio::test]
    async fn test_get_stats_multiple_apps() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];

        // Insert memories for different apps
        let memory1 = create_test_memory("id1", "com.apple.Notes", "Test.txt", embedding.clone());
        let memory2 = create_test_memory("id2", "com.apple.TextEdit", "Doc.txt", embedding.clone());
        let memory3 = create_test_memory("id3", "com.google.Chrome", "Page.html", embedding);

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();
        db.insert_memory(memory3).await.unwrap();

        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 3);
        assert_eq!(stats.total_apps, 3);
    }

    #[tokio::test]
    async fn test_clear_memories_by_window_title() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];

        let memory1 = create_test_memory("id1", "com.apple.Notes", "Doc1.txt", embedding.clone());
        let memory2 = create_test_memory("id2", "com.apple.Notes", "Doc2.txt", embedding);

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();

        // Clear only Doc1.txt memories
        let deleted = db
            .clear_memories(Some("com.apple.Notes"), Some("Doc1.txt"))
            .await
            .unwrap();

        assert_eq!(deleted, 1);
        assert_eq!(db.get_stats().await.unwrap().total_memories, 1);
    }

    #[tokio::test]
    async fn test_cosine_similarity_edge_cases() {
        // Test zero vector
        let zero = vec![0.0, 0.0, 0.0];
        let non_zero = vec![1.0, 1.0, 1.0];
        let similarity = VectorDatabase::cosine_similarity(&zero, &non_zero);
        assert_eq!(similarity, 0.0);

        // Test negative values
        let a = vec![1.0, -1.0, 0.0];
        let b = vec![-1.0, 1.0, 0.0];
        let similarity = VectorDatabase::cosine_similarity(&a, &b);
        assert!(similarity < 0.0); // Opposite direction

        // Test identical negative vectors
        let c = vec![-1.0, -1.0, -1.0];
        let d = vec![-1.0, -1.0, -1.0];
        let similarity = VectorDatabase::cosine_similarity(&c, &d);
        assert!((similarity - 1.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_insert_memory_with_special_characters() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];

        let memory = MemoryEntry::with_embedding(
            "special-id".to_string(),
            ContextAnchor::now(
                "com.app.test".to_string(),
                "File's Name \"quoted\".txt".to_string(),
            ),
            "Input with 'quotes' and \"double quotes\"".to_string(),
            "Output with <tags> & ampersands".to_string(),
            embedding,
        );

        db.insert_memory(memory).await.unwrap();

        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 1);
    }

    #[tokio::test]
    async fn test_search_memories_returns_exact_match() {
        let db = create_test_db();
        let embedding = vec![1.0, 0.0, 0.0, 0.0];
        let memory = create_test_memory("id1", "com.apple.Notes", "Test.txt", embedding.clone());

        db.insert_memory(memory).await.unwrap();

        // Search with exact same embedding should return the memory
        let results = db
            .search_memories("com.apple.Notes", "Test.txt", &embedding, 5)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "id1");
    }

    #[tokio::test]
    async fn test_embedding_serialization_large_vectors() {
        // Test with 512-dimensional vector (real embedding size for bge-small-zh-v1.5)
        let embedding: Vec<f32> = (0..512).map(|i| (i as f32) * 0.001).collect();
        let bytes = VectorDatabase::serialize_embedding(&embedding);
        let deserialized = VectorDatabase::deserialize_embedding(&bytes);

        assert_eq!(embedding.len(), deserialized.len());
        for (a, b) in embedding.iter().zip(deserialized.iter()) {
            assert!((a - b).abs() < 0.0001);
        }
    }

    #[tokio::test]
    async fn test_database_file_creation() {
        use std::fs;

        let temp_dir = std::env::temp_dir().join(format!(
            "aether_test_perms_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let _db = VectorDatabase::new(temp_dir.clone()).unwrap();

        // Verify database directory was created
        assert!(temp_dir.exists());

        // Cleanup
        let _ = fs::remove_dir_all(temp_dir);
    }
}
