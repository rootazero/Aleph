//! SQLite vector store implementation

use crate::error::{AetherError, Result};
use fastembed::{EmbeddingModel as FastEmbedModel, InitOptions, TextEmbedding};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Memory entry stored in vector database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub user_input: String,
    pub assistant_response: String,
    pub timestamp: i64,
    pub app_context: Option<String>,
}

impl MemoryEntry {
    pub fn text(&self) -> String {
        format!("User: {}\nAssistant: {}", self.user_input, self.assistant_response)
    }
}

/// Memory store using SQLite with vector search
pub struct MemoryStore {
    conn: Arc<RwLock<Connection>>,
    embedding_model: TextEmbedding,
    initialized: bool,
}

impl MemoryStore {
    pub const EMBEDDING_DIM: usize = 512;

    pub async fn new(db_path: &str) -> Result<Self> {
        info!(db_path = %db_path, "Creating MemoryStore");

        if let Some(parent) = Path::new(db_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AetherError::config(format!("Failed to create db directory: {}", e))
                })?;
            }
        }

        let conn = Connection::open(db_path).map_err(|e| {
            AetherError::config(format!("Failed to open database: {}", e))
        })?;

        let embedding_model = TextEmbedding::try_new(
            InitOptions::new(FastEmbedModel::BGESmallZHV15)
                .with_show_download_progress(true),
        ).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?;

        #[allow(clippy::arc_with_non_send_sync)]
        let store = Self {
            conn: Arc::new(RwLock::new(conn)),
            embedding_model,
            initialized: false,
        };

        store.init_schema().await?;

        Ok(Self {
            conn: store.conn,
            embedding_model: store.embedding_model,
            initialized: true,
        })
    }

    async fn init_schema(&self) -> Result<()> {
        let conn = self.conn.write().await;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                user_input TEXT NOT NULL,
                assistant_response TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                app_context TEXT,
                embedding BLOB NOT NULL
            )",
            [],
        ).map_err(|e| AetherError::config(format!("Failed to create table: {}", e)))?;
        debug!("Database schema initialized");
        Ok(())
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub async fn store(&self, entry: MemoryEntry) -> Result<()> {
        let text = entry.text();
        let embedding = self.embed_text(&text)?;
        let embedding_bytes = Self::embedding_to_bytes(&embedding);

        let conn = self.conn.write().await;
        conn.execute(
            "INSERT OR REPLACE INTO memories (id, user_input, assistant_response, timestamp, app_context, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                entry.id,
                entry.user_input,
                entry.assistant_response,
                entry.timestamp,
                entry.app_context,
                embedding_bytes,
            ],
        ).map_err(|e| AetherError::config(format!("Failed to insert memory: {}", e)))?;
        debug!(id = %entry.id, "Memory stored");
        Ok(())
    }

    pub async fn search(&self, query: &str, top_k: usize) -> Result<Vec<(MemoryEntry, f32)>> {
        let query_embedding = self.embed_text(query)?;

        let conn = self.conn.read().await;
        let mut stmt = conn.prepare(
            "SELECT id, user_input, assistant_response, timestamp, app_context, embedding FROM memories"
        ).map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let entries: Vec<(MemoryEntry, Vec<f32>)> = stmt
            .query_map([], |row| {
                let embedding_bytes: Vec<u8> = row.get(5)?;
                Ok((
                    MemoryEntry {
                        id: row.get(0)?,
                        user_input: row.get(1)?,
                        assistant_response: row.get(2)?,
                        timestamp: row.get(3)?,
                        app_context: row.get(4)?,
                    },
                    Self::bytes_to_embedding(&embedding_bytes),
                ))
            })
            .map_err(|e| AetherError::config(format!("Query failed: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();

        let mut scored: Vec<(MemoryEntry, f32)> = entries
            .into_iter()
            .map(|(entry, emb)| {
                let score = Self::cosine_similarity(&query_embedding, &emb);
                (entry, score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    pub async fn clear(&self) -> Result<()> {
        let conn = self.conn.write().await;
        conn.execute("DELETE FROM memories", [])
            .map_err(|e| AetherError::config(format!("Failed to clear memories: {}", e)))?;
        info!("All memories cleared");
        Ok(())
    }

    fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embedding_model
            .embed(vec![text], None)
            .map_err(|e| AetherError::config(format!("Embedding failed: {}", e)))?;
        embeddings.into_iter().next()
            .ok_or_else(|| AetherError::config("No embedding returned"))
    }

    fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes.chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a > 0.0 && norm_b > 0.0 { dot / (norm_a * norm_b) } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_entry_text() {
        let entry = MemoryEntry {
            id: "test-1".to_string(),
            user_input: "Hello".to_string(),
            assistant_response: "Hi there!".to_string(),
            timestamp: 1234567890,
            app_context: None,
        };
        assert!(entry.text().contains("User: Hello"));
        assert!(entry.text().contains("Assistant: Hi there!"));
    }

    #[test]
    fn test_embedding_bytes_roundtrip() {
        let embedding = vec![0.1, 0.2, 0.3, 0.4];
        let bytes = MemoryStore::embedding_to_bytes(&embedding);
        let recovered = MemoryStore::bytes_to_embedding(&bytes);
        for (a, b) in embedding.iter().zip(recovered.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((MemoryStore::cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
        let c = vec![0.0, 1.0, 0.0];
        assert!(MemoryStore::cosine_similarity(&a, &c).abs() < 1e-6);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_memory_store_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_memory.db");
        let store = MemoryStore::new(db_path.to_str().unwrap()).await.unwrap();
        assert!(store.is_initialized());
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_store_and_search() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_memory.db");
        let store = MemoryStore::new(db_path.to_str().unwrap()).await.unwrap();

        let entry = MemoryEntry {
            id: "test-1".to_string(),
            user_input: "What is Rust?".to_string(),
            assistant_response: "Rust is a systems programming language.".to_string(),
            timestamp: 1234567890,
            app_context: None,
        };

        store.store(entry).await.unwrap();
        let results = store.search("programming language", 5).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].0.id, "test-1");
    }
}
