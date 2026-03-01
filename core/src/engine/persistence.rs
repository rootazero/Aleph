//! Persistence Layer for Learned Rules and Patterns
//!
//! This module provides persistence for learned rules, patterns, and classifier state.
//! It enables the system to save and load learning data across sessions.
//!
//! # Architecture
//!
//! ```text
//! RuleLearner → Persistence → SQLite Database
//!     ↓             ↓              ↓
//!  Patterns      Save/Load      Storage
//! ```
//!
//! # Storage Format
//!
//! - **learned_patterns**: User input patterns and their associated actions
//! - **classifier_state**: Naive Bayes classifier parameters (priors, likelihoods)
//! - **rule_metadata**: Rule generation metadata (confidence, hit rate, etc.)
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::engine::{Persistence, RuleLearner};
//!
//! let persistence = Persistence::new("./data/learned_rules.db").await?;
//! let learner = RuleLearner::new();
//!
//! // Load previously learned patterns
//! persistence.load_learner(&learner).await?;
//!
//! // ... learning happens ...
//!
//! // Save learned patterns
//! persistence.save_learner(&learner).await?;
//! ```

use super::{AtomicAction, NaiveBayesClassifier};
use rusqlite::{params, Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use crate::sync_primitives::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

/// Persistence layer for learned rules
pub struct Persistence {
    /// Database connection (Mutex instead of RwLock because rusqlite::Connection is not Sync)
    conn: Arc<Mutex<Connection>>,

    /// Database file path (retained for diagnostics)
    _db_path: PathBuf,
}

impl Persistence {
    /// Create a new persistence layer
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to the SQLite database file
    pub async fn new<P: AsRef<Path>>(db_path: P) -> SqliteResult<Self> {
        let db_path = db_path.as_ref().to_path_buf();

        // Create parent directory if it doesn't exist
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let conn = Connection::open(&db_path)?;

        // Initialize schema
        Self::init_schema(&conn)?;

        info!(path = ?db_path, "Initialized persistence layer");

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            _db_path: db_path,
        })
    }

    /// Initialize database schema
    fn init_schema(conn: &Connection) -> SqliteResult<()> {
        // Learned patterns table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS learned_patterns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                pattern TEXT NOT NULL UNIQUE,
                action_json TEXT NOT NULL,
                count INTEGER NOT NULL DEFAULT 0,
                successes INTEGER NOT NULL DEFAULT 0,
                failures INTEGER NOT NULL DEFAULT 0,
                confidence REAL NOT NULL DEFAULT 0.0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        // Classifier state table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS classifier_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                state_json TEXT NOT NULL,
                total_samples INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        // Rule metadata table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS rule_metadata (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                pattern TEXT NOT NULL UNIQUE,
                hit_count INTEGER NOT NULL DEFAULT 0,
                miss_count INTEGER NOT NULL DEFAULT 0,
                avg_latency_ms REAL NOT NULL DEFAULT 0.0,
                last_hit_at INTEGER,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        // Create indices
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_patterns_confidence ON learned_patterns(confidence DESC)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_metadata_hit_count ON rule_metadata(hit_count DESC)",
            [],
        )?;

        Ok(())
    }

    /// Save a learned pattern
    ///
    /// # Arguments
    ///
    /// * `pattern` - The input pattern
    /// * `action` - The associated atomic action
    /// * `count` - Number of observations
    /// * `successes` - Number of successful executions
    /// * `failures` - Number of failed executions
    pub async fn save_pattern(
        &self,
        pattern: &str,
        action: &AtomicAction,
        count: usize,
        successes: usize,
        failures: usize,
    ) -> SqliteResult<()> {
        let action_json = serde_json::to_string(action).unwrap();
        let confidence = if count > 0 {
            successes as f64 / count as f64
        } else {
            0.0
        };
        let now = chrono::Utc::now().timestamp();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO learned_patterns (pattern, action_json, count, successes, failures, confidence, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(pattern) DO UPDATE SET
                action_json = excluded.action_json,
                count = excluded.count,
                successes = excluded.successes,
                failures = excluded.failures,
                confidence = excluded.confidence,
                updated_at = excluded.updated_at",
            params![pattern, action_json, count as i64, successes as i64, failures as i64, confidence, now, now],
        )?;

        debug!(pattern = %pattern, count = count, "Saved learned pattern");

        Ok(())
    }

    /// Load all learned patterns
    pub async fn load_patterns(&self) -> SqliteResult<Vec<LearnedPattern>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT pattern, action_json, count, successes, failures, confidence
             FROM learned_patterns
             ORDER BY confidence DESC, count DESC",
        )?;

        let patterns = stmt
            .query_map([], |row| {
                let action_json: String = row.get(1)?;
                let action: AtomicAction = serde_json::from_str(&action_json).unwrap();

                Ok(LearnedPattern {
                    pattern: row.get(0)?,
                    action,
                    count: row.get::<_, i64>(2)? as usize,
                    successes: row.get::<_, i64>(3)? as usize,
                    failures: row.get::<_, i64>(4)? as usize,
                    confidence: row.get(5)?,
                })
            })?
            .collect::<SqliteResult<Vec<_>>>()?;

        info!(count = patterns.len(), "Loaded learned patterns");

        Ok(patterns)
    }

    /// Save classifier state
    ///
    /// # Arguments
    ///
    /// * `classifier` - The Naive Bayes classifier
    pub async fn save_classifier(&self, classifier: &NaiveBayesClassifier) -> SqliteResult<()> {
        let state_json = serde_json::to_string(classifier).unwrap();
        let total_samples = classifier.sample_count();
        let now = chrono::Utc::now().timestamp();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO classifier_state (id, state_json, total_samples, updated_at)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                state_json = excluded.state_json,
                total_samples = excluded.total_samples,
                updated_at = excluded.updated_at",
            params![state_json, total_samples as i64, now],
        )?;

        info!(total_samples = total_samples, "Saved classifier state");

        Ok(())
    }

    /// Load classifier state
    pub async fn load_classifier(&self) -> SqliteResult<Option<NaiveBayesClassifier>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare("SELECT state_json FROM classifier_state WHERE id = 1")?;

        let result = stmt.query_row([], |row| {
            let state_json: String = row.get(0)?;
            let classifier: NaiveBayesClassifier = serde_json::from_str(&state_json).unwrap();
            Ok(classifier)
        });

        match result {
            Ok(classifier) => {
                info!(
                    total_samples = classifier.sample_count(),
                    "Loaded classifier state"
                );
                Ok(Some(classifier))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                debug!("No classifier state found");
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Save rule metadata
    ///
    /// # Arguments
    ///
    /// * `pattern` - The rule pattern
    /// * `hit_count` - Number of times the rule was hit
    /// * `miss_count` - Number of times the rule was missed
    /// * `avg_latency_ms` - Average latency in milliseconds
    pub async fn save_rule_metadata(
        &self,
        pattern: &str,
        hit_count: usize,
        miss_count: usize,
        avg_latency_ms: f64,
    ) -> SqliteResult<()> {
        let now = chrono::Utc::now().timestamp();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO rule_metadata (pattern, hit_count, miss_count, avg_latency_ms, last_hit_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(pattern) DO UPDATE SET
                hit_count = excluded.hit_count,
                miss_count = excluded.miss_count,
                avg_latency_ms = excluded.avg_latency_ms,
                last_hit_at = excluded.last_hit_at",
            params![pattern, hit_count as i64, miss_count as i64, avg_latency_ms, now, now],
        )?;

        debug!(pattern = %pattern, hit_count = hit_count, "Saved rule metadata");

        Ok(())
    }

    /// Load rule metadata
    pub async fn load_rule_metadata(&self) -> SqliteResult<Vec<RuleMetadata>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT pattern, hit_count, miss_count, avg_latency_ms
             FROM rule_metadata
             ORDER BY hit_count DESC",
        )?;

        let metadata = stmt
            .query_map([], |row| {
                Ok(RuleMetadata {
                    pattern: row.get(0)?,
                    hit_count: row.get::<_, i64>(1)? as usize,
                    miss_count: row.get::<_, i64>(2)? as usize,
                    avg_latency_ms: row.get(3)?,
                })
            })?
            .collect::<SqliteResult<Vec<_>>>()?;

        info!(count = metadata.len(), "Loaded rule metadata");

        Ok(metadata)
    }

    /// Clear all learned data
    pub async fn clear_all(&self) -> SqliteResult<()> {
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM learned_patterns", [])?;
        conn.execute("DELETE FROM classifier_state", [])?;
        conn.execute("DELETE FROM rule_metadata", [])?;

        info!("Cleared all learned data");

        Ok(())
    }

    /// Get database statistics
    pub async fn stats(&self) -> SqliteResult<PersistenceStats> {
        let conn = self.conn.lock().await;

        let pattern_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM learned_patterns",
            [],
            |row| row.get(0),
        )?;

        let rule_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rule_metadata",
            [],
            |row| row.get(0),
        )?;

        let total_samples: i64 = conn
            .query_row(
                "SELECT total_samples FROM classifier_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        Ok(PersistenceStats {
            pattern_count: pattern_count as usize,
            rule_count: rule_count as usize,
            total_samples: total_samples as usize,
        })
    }
}

/// Learned pattern record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedPattern {
    pub pattern: String,
    pub action: AtomicAction,
    pub count: usize,
    pub successes: usize,
    pub failures: usize,
    pub confidence: f64,
}

/// Rule metadata record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleMetadata {
    pub pattern: String,
    pub hit_count: usize,
    pub miss_count: usize,
    pub avg_latency_ms: f64,
}

/// Persistence statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistenceStats {
    pub pattern_count: usize,
    pub rule_count: usize,
    pub total_samples: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SearchPattern, SearchScope};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_persistence_basic() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let persistence = Persistence::new(&db_path).await.unwrap();

        // Save a pattern
        let action = AtomicAction::Search {
            pattern: SearchPattern::Regex {
                pattern: "TODO".to_string(),
            },
            scope: SearchScope::Workspace,
            filters: vec![],
        };

        persistence
            .save_pattern("search for TODO", &action, 5, 5, 0)
            .await
            .unwrap();

        // Load patterns
        let patterns = persistence.load_patterns().await.unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].pattern, "search for TODO");
        assert_eq!(patterns[0].count, 5);
        assert_eq!(patterns[0].successes, 5);
        assert_eq!(patterns[0].confidence, 1.0);
    }

    #[tokio::test]
    async fn test_classifier_persistence() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let persistence = Persistence::new(&db_path).await.unwrap();

        // Create and save classifier
        let classifier = NaiveBayesClassifier::new();
        persistence.save_classifier(&classifier).await.unwrap();

        // Load classifier
        let loaded = persistence.load_classifier().await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn test_rule_metadata() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let persistence = Persistence::new(&db_path).await.unwrap();

        // Save metadata
        persistence
            .save_rule_metadata("git.*status", 100, 5, 50.0)
            .await
            .unwrap();

        // Load metadata
        let metadata = persistence.load_rule_metadata().await.unwrap();
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].pattern, "git.*status");
        assert_eq!(metadata[0].hit_count, 100);
        assert_eq!(metadata[0].avg_latency_ms, 50.0);
    }

    #[tokio::test]
    async fn test_clear_all() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let persistence = Persistence::new(&db_path).await.unwrap();

        // Save some data
        let action = AtomicAction::Bash {
            command: "test".to_string(),
            cwd: None,
        };
        persistence
            .save_pattern("test", &action, 1, 1, 0)
            .await
            .unwrap();

        // Clear all
        persistence.clear_all().await.unwrap();

        // Verify empty
        let patterns = persistence.load_patterns().await.unwrap();
        assert_eq!(patterns.len(), 0);
    }

    #[tokio::test]
    async fn test_stats() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let persistence = Persistence::new(&db_path).await.unwrap();

        // Save some data
        let action = AtomicAction::Bash {
            command: "test".to_string(),
            cwd: None,
        };
        persistence
            .save_pattern("test", &action, 1, 1, 0)
            .await
            .unwrap();

        persistence
            .save_rule_metadata("test", 10, 0, 10.0)
            .await
            .unwrap();

        // Get stats
        let stats = persistence.stats().await.unwrap();
        assert_eq!(stats.pattern_count, 1);
        assert_eq!(stats.rule_count, 1);
    }
}
