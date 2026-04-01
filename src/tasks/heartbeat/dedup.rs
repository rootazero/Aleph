//! Deduplication Engine
//!
//! Prevents redundant notifications by comparing new probe outputs
//! against recent history using cosine similarity on embeddings.
//!
//! The `DedupEngine` stores embedding vectors in the heartbeat SQLite DB
//! and compares them against recent history within a configurable time window.

use std::sync::Arc;

use rusqlite::{params, Connection};
use tokio::sync::Mutex;

use crate::memory::EmbeddingProvider;
use crate::tasks::heartbeat::config::DedupConfig;

// ── cosine_similarity ─────────────────────────────────────────────────────────

/// Compute cosine similarity between two equal-length float vectors.
///
/// Returns 0.0 if either vector is a zero vector.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

// ── DedupEngine ───────────────────────────────────────────────────────────────

/// Deduplication engine for heartbeat outputs.
///
/// Uses semantic embedding similarity to suppress redundant notifications.
/// When no embedding provider is configured, all calls degrade gracefully
/// to no-ops (is_duplicate returns false, record is a no-op).
pub struct DedupEngine {
    config: DedupConfig,
    conn: Arc<Mutex<Connection>>,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
}

impl DedupEngine {
    /// Create a real DedupEngine with a DB connection and optional embedding provider.
    ///
    /// Initializes the dedup schema on the given connection.
    /// If `embedding_provider` is None, all dedup operations are no-ops.
    pub fn new(
        config: DedupConfig,
        conn: Arc<Mutex<Connection>>,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Self {
        Self {
            config,
            conn,
            embedding_provider,
        }
    }

    /// Create a no-op DedupEngine backed by an in-memory connection.
    ///
    /// Used in tests and as a fallback when no DB is available.
    pub fn noop(config: DedupConfig) -> Self {
        let conn = Connection::open_in_memory()
            .expect("failed to open in-memory connection for noop DedupEngine");
        // Initialize schema so the connection is valid, errors are ignored.
        let _ = init_dedup_schema(&conn);
        Self {
            config,
            conn: Arc::new(Mutex::new(conn)),
            embedding_provider: None,
        }
    }

    /// Check whether the given output is a semantic duplicate of recent outputs.
    ///
    /// Returns false when:
    /// - No embedding provider is configured
    /// - Embedding generation fails
    /// - No similar output found in history within the window
    pub async fn is_duplicate(&self, task_id: &str, output: &str) -> bool {
        let provider = match &self.embedding_provider {
            Some(p) => p,
            None => return false, // No embedding provider = no dedup
        };

        // Compute embedding for current output
        let embedding = match provider.embed(output).await {
            Ok(e) => e,
            Err(_) => return false, // Embedding failure = pass through
        };

        // Load history from DB (lock only for DB read, release before comparison)
        let history = {
            let conn = self.conn.lock().await;
            let cutoff = chrono::Utc::now().timestamp_millis() - self.config.window_ms as i64;
            let model_name = provider.model_name().to_string();
            load_dedup_history(&conn, task_id, cutoff, &model_name, self.config.max_history)
                .unwrap_or_default()
        };
        // Lock released here

        // Compare with history entries
        for (_, hist_embedding) in &history {
            let sim = cosine_similarity(&embedding, hist_embedding);
            if sim >= self.config.similarity_threshold {
                return true; // Duplicate found
            }
        }

        false
    }

    /// Record an output embedding after successful delivery.
    ///
    /// No-op when embedding provider is not configured.
    pub async fn record(&self, task_id: &str, output: &str) {
        let provider = match &self.embedding_provider {
            Some(p) => p,
            None => return,
        };

        let embedding = match provider.embed(output).await {
            Ok(e) => e,
            Err(_) => return,
        };

        let model_name = provider.model_name().to_string();
        let conn = self.conn.lock().await;
        let _ = insert_dedup_record(&conn, task_id, output, &embedding, &model_name);
        let _ = prune_dedup_records(&conn, task_id, self.config.max_history);
    }

    /// Remove dedup records older than `retention_ms` milliseconds.
    pub async fn cleanup(&self, retention_ms: u64) {
        let conn = self.conn.lock().await;
        let cutoff = chrono::Utc::now().timestamp_millis() - retention_ms as i64;
        let _ = cleanup_dedup_records(&conn, cutoff);
    }
}

// ── Schema ─────────────────────────────────────────────────────────────────────

/// Initialize the heartbeat_dedup table schema.
///
/// Safe to call multiple times (uses IF NOT EXISTS).
pub fn init_dedup_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS heartbeat_dedup (
            id          TEXT PRIMARY KEY,
            task_id     TEXT NOT NULL,
            output_text TEXT NOT NULL,
            embedding   BLOB NOT NULL,
            model       TEXT NOT NULL,
            created_at  INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_heartbeat_dedup_task_id
            ON heartbeat_dedup(task_id);",
    )
    .map_err(|e| format!("failed to create heartbeat_dedup schema: {e}"))
}

// ── SQLite Helpers ─────────────────────────────────────────────────────────────

/// Load recent dedup records for a task within the time window.
///
/// Only returns records that match the given model name, so different embedding
/// models do not cross-contaminate dedup history.
fn load_dedup_history(
    conn: &Connection,
    task_id: &str,
    cutoff_ms: i64,
    model: &str,
    limit: usize,
) -> Result<Vec<(String, Vec<f32>)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, embedding
             FROM heartbeat_dedup
             WHERE task_id = ?1 AND model = ?2 AND created_at > ?3
             ORDER BY created_at DESC
             LIMIT ?4",
        )
        .map_err(|e| format!("failed to prepare dedup history query: {e}"))?;

    let rows = stmt
        .query_map(params![task_id, model, cutoff_ms, limit as i64], |row| {
            let id: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            let embedding = bytes_to_f32_vec(&blob);
            Ok((id, embedding))
        })
        .map_err(|e| format!("failed to query dedup history: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to read dedup row: {e}"))
}

/// Insert a new dedup record into the database.
fn insert_dedup_record(
    conn: &Connection,
    task_id: &str,
    output: &str,
    embedding: &[f32],
    model: &str,
) -> Result<(), String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let blob = f32_vec_to_bytes(embedding);

    conn.execute(
        "INSERT INTO heartbeat_dedup (id, task_id, output_text, embedding, model, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, task_id, output, blob, model, now],
    )
    .map_err(|e| format!("failed to insert dedup record: {e}"))?;

    Ok(())
}

/// Delete the oldest records beyond `max_history` for a given task.
fn prune_dedup_records(conn: &Connection, task_id: &str, max_history: usize) -> Result<(), String> {
    conn.execute(
        "DELETE FROM heartbeat_dedup
         WHERE task_id = ?1
           AND id NOT IN (
               SELECT id FROM heartbeat_dedup
               WHERE task_id = ?1
               ORDER BY created_at DESC
               LIMIT ?2
           )",
        params![task_id, max_history as i64],
    )
    .map_err(|e| format!("failed to prune dedup records: {e}"))?;

    Ok(())
}

/// Delete all dedup records older than `cutoff_ms`.
fn cleanup_dedup_records(conn: &Connection, cutoff_ms: i64) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM heartbeat_dedup WHERE created_at < ?1",
        params![cutoff_ms],
    )
    .map_err(|e| format!("failed to cleanup dedup records: {e}"))
}

// ── Serialization Helpers ──────────────────────────────────────────────────────

/// Serialize a `f32` slice to a little-endian byte vector.
fn f32_vec_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Deserialize a little-endian byte slice to a `Vec<f32>`.
///
/// Ignores trailing bytes that are not a multiple of 4.
fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_zero() {
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn test_cosine_similarity_proportional() {
        // Vectors pointing in the same direction (just scaled differently)
        // should give similarity 1.0
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![2.0, 4.0, 6.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 0.001);
    }

    #[test]
    fn test_f32_roundtrip() {
        let v = vec![1.0f32, 2.5, -3.7, 0.0];
        let bytes = f32_vec_to_bytes(&v);
        let back = bytes_to_f32_vec(&bytes);
        assert_eq!(v.len(), back.len());
        for (a, b) in v.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_f32_roundtrip_empty() {
        let v: Vec<f32> = vec![];
        let bytes = f32_vec_to_bytes(&v);
        let back = bytes_to_f32_vec(&bytes);
        assert!(back.is_empty());
    }

    #[test]
    fn test_bytes_to_f32_ignores_trailing() {
        let v = vec![1.0f32];
        let mut bytes = f32_vec_to_bytes(&v);
        bytes.push(0xFF); // one extra byte — should be ignored
        bytes.push(0xFF);
        bytes.push(0xFF); // only 3 extra, not a full f32
        let back = bytes_to_f32_vec(&bytes);
        assert_eq!(back.len(), 1);
        assert!((back[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_init_dedup_schema_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(init_dedup_schema(&conn).is_ok());
        // Second call should also succeed
        assert!(init_dedup_schema(&conn).is_ok());
    }

    #[test]
    fn test_insert_and_load_dedup_history() {
        let conn = Connection::open_in_memory().unwrap();
        init_dedup_schema(&conn).unwrap();

        let embedding = vec![1.0f32, 0.0, 0.0];
        let now_ms = chrono::Utc::now().timestamp_millis();

        insert_dedup_record(&conn, "task-1", "hello world", &embedding, "model-a").unwrap();

        // Load within window
        let cutoff = now_ms - 1_000; // 1 second ago
        let history = load_dedup_history(&conn, "task-1", cutoff, "model-a", 10).unwrap();
        assert_eq!(history.len(), 1);
        assert!((history[0].1[0] - 1.0).abs() < 1e-6);

        // Different model should return empty
        let history2 = load_dedup_history(&conn, "task-1", cutoff, "model-b", 10).unwrap();
        assert!(history2.is_empty());

        // Different task should return empty
        let history3 = load_dedup_history(&conn, "task-2", cutoff, "model-a", 10).unwrap();
        assert!(history3.is_empty());
    }

    #[test]
    fn test_prune_keeps_only_max_history() {
        let conn = Connection::open_in_memory().unwrap();
        init_dedup_schema(&conn).unwrap();

        // Insert 5 records with staggered timestamps
        for i in 0..5_i64 {
            let id = uuid::Uuid::new_v4().to_string();
            let embedding = vec![i as f32, 0.0];
            let blob = f32_vec_to_bytes(&embedding);
            conn.execute(
                "INSERT INTO heartbeat_dedup (id, task_id, output_text, embedding, model, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, "task-1", "output", blob, "model-a", 1_000_000 + i * 1000],
            ).unwrap();
        }

        // Prune to max_history = 3
        prune_dedup_records(&conn, "task-1", 3).unwrap();

        let cutoff = 0_i64; // include all
        let history = load_dedup_history(&conn, "task-1", cutoff, "model-a", 10).unwrap();
        assert_eq!(history.len(), 3, "should retain only 3 most recent records");
    }

    #[test]
    fn test_cleanup_removes_old_records() {
        let conn = Connection::open_in_memory().unwrap();
        init_dedup_schema(&conn).unwrap();

        let embedding = vec![1.0f32];
        let blob = f32_vec_to_bytes(&embedding);

        // Old record (timestamp 0)
        conn.execute(
            "INSERT INTO heartbeat_dedup (id, task_id, output_text, embedding, model, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["old-id", "task-1", "old output", blob.clone(), "model-a", 0_i64],
        ).unwrap();

        // Recent record
        let now_ms = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO heartbeat_dedup (id, task_id, output_text, embedding, model, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["new-id", "task-1", "new output", blob, "model-a", now_ms],
        ).unwrap();

        let cutoff = now_ms - 1_000; // 1 second ago
        let deleted = cleanup_dedup_records(&conn, cutoff).unwrap();
        assert_eq!(deleted, 1, "should delete only the old record");

        let history = load_dedup_history(&conn, "task-1", 0, "model-a", 10).unwrap();
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn test_noop_engine_never_deduplicates() {
        let engine = DedupEngine::noop(DedupConfig::default());
        assert!(!engine.is_duplicate("task-1", "some output").await);
        engine.record("task-1", "some output").await;
        // Still no dedup since embedding provider not wired
        assert!(!engine.is_duplicate("task-1", "some output").await);
    }

    #[tokio::test]
    async fn test_engine_without_provider_is_noop() {
        let conn = Connection::open_in_memory().unwrap();
        init_dedup_schema(&conn).unwrap();
        let engine = DedupEngine::new(
            DedupConfig::default(),
            Arc::new(Mutex::new(conn)),
            None, // no embedding provider
        );
        assert!(!engine.is_duplicate("task-1", "hello").await);
        engine.record("task-1", "hello").await;
        assert!(!engine.is_duplicate("task-1", "hello").await);
    }
}
