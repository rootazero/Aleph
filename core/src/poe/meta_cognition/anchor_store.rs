//! Storage layer for behavioral anchors with CRUD operations
//!
//! This module provides persistent storage for behavioral anchors using SQLite,
//! with JSON serialization for complex fields and proper error handling.

use super::types::{AnchorScope, AnchorSource, BehavioralAnchor};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Result};
use std::sync::Arc;

/// Storage manager for behavioral anchors
///
/// Provides CRUD operations for persisting and retrieving behavioral anchors
/// from the SQLite database. Complex types are serialized as JSON.
pub struct AnchorStore {
    conn: Arc<Connection>,
}

impl AnchorStore {
    /// Create a new AnchorStore with the given database connection
    ///
    /// # Arguments
    ///
    /// * `conn` - Shared database connection
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use rusqlite::Connection;
    /// use alephcore::poe::meta_cognition::anchor_store::AnchorStore;
    ///
    /// let conn = Arc::new(Connection::open_in_memory().unwrap());
    /// let store = AnchorStore::new(conn);
    /// ```
    pub fn new(conn: Arc<Connection>) -> Self {
        Self { conn }
    }

    /// Add a new behavioral anchor to the database
    ///
    /// # Arguments
    ///
    /// * `anchor` - The behavioral anchor to add
    ///
    /// # Returns
    ///
    /// * `Result<String>` - The ID of the added anchor
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use rusqlite::Connection;
    /// # use alephcore::poe::meta_cognition::{BehavioralAnchor, AnchorSource, AnchorScope, anchor_store::AnchorStore};
    /// # let conn = Arc::new(Connection::open_in_memory().unwrap());
    /// # let mut store = AnchorStore::new(conn);
    /// let anchor = BehavioralAnchor::new(
    ///     "test-id".to_string(),
    ///     "Always check Python version".to_string(),
    ///     vec!["Python".to_string()],
    ///     AnchorSource::ManualInjection { author: "test".to_string() },
    ///     AnchorScope::Global,
    ///     100,
    ///     0.8,
    /// );
    /// let id = store.add(anchor).unwrap();
    /// ```
    pub fn add(&mut self, anchor: BehavioralAnchor) -> Result<String> {
        let trigger_tags_json = serde_json::to_string(&anchor.trigger_tags)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let source_json = serde_json::to_string(&anchor.source)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let scope_json = serde_json::to_string(&anchor.scope)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let conflicts_with_json = serde_json::to_string(&anchor.conflicts_with)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        self.conn.execute(
            r#"
            INSERT INTO behavioral_anchors (
                id, rule_text, trigger_tags, confidence,
                created_at, last_validated, validation_count, failure_count,
                source, scope, priority, conflicts_with, supersedes
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            "#,
            params![
                anchor.id,
                anchor.rule_text,
                trigger_tags_json,
                anchor.confidence,
                anchor.created_at.to_rfc3339(),
                anchor.last_validated.to_rfc3339(),
                anchor.validation_count,
                anchor.failure_count,
                source_json,
                scope_json,
                anchor.priority,
                conflicts_with_json,
                anchor.supersedes,
            ],
        )?;

        Ok(anchor.id)
    }

    /// Get a behavioral anchor by ID
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the anchor to retrieve
    ///
    /// # Returns
    ///
    /// * `Result<Option<BehavioralAnchor>>` - The anchor if found, None otherwise
    pub fn get(&self, id: &str) -> Result<Option<BehavioralAnchor>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, rule_text, trigger_tags, confidence,
                   created_at, last_validated, validation_count, failure_count,
                   source, scope, priority, conflicts_with, supersedes
            FROM behavioral_anchors
            WHERE id = ?1
            "#,
        )?;

        let result = stmt.query_row(params![id], |row| {
            let trigger_tags_json: String = row.get(2)?;
            let source_json: String = row.get(8)?;
            let scope_json: String = row.get(9)?;
            let conflicts_with_json: String = row.get(11)?;

            let trigger_tags: Vec<String> = serde_json::from_str(&trigger_tags_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e)))?;
            let source: AnchorSource = serde_json::from_str(&source_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e)))?;
            let scope: AnchorScope = serde_json::from_str(&scope_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e)))?;
            let conflicts_with: Vec<String> = serde_json::from_str(&conflicts_with_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(e)))?;

            let created_at_str: String = row.get(4)?;
            let last_validated_str: String = row.get(5)?;

            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e)))?
                .with_timezone(&Utc);
            let last_validated = DateTime::parse_from_rfc3339(&last_validated_str)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e)))?
                .with_timezone(&Utc);

            Ok(BehavioralAnchor {
                id: row.get(0)?,
                rule_text: row.get(1)?,
                trigger_tags,
                confidence: row.get(3)?,
                created_at,
                last_validated,
                validation_count: row.get(6)?,
                failure_count: row.get(7)?,
                source,
                scope,
                priority: row.get(10)?,
                conflicts_with,
                supersedes: row.get(12)?,
            })
        });

        match result {
            Ok(anchor) => Ok(Some(anchor)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Update an existing behavioral anchor
    ///
    /// # Arguments
    ///
    /// * `anchor` - The anchor with updated values
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Ok if update was successful
    pub fn update(&mut self, anchor: &BehavioralAnchor) -> Result<()> {
        let trigger_tags_json = serde_json::to_string(&anchor.trigger_tags)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let source_json = serde_json::to_string(&anchor.source)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let scope_json = serde_json::to_string(&anchor.scope)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let conflicts_with_json = serde_json::to_string(&anchor.conflicts_with)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        self.conn.execute(
            r#"
            UPDATE behavioral_anchors
            SET rule_text = ?2, trigger_tags = ?3, confidence = ?4,
                created_at = ?5, last_validated = ?6, validation_count = ?7, failure_count = ?8,
                source = ?9, scope = ?10, priority = ?11, conflicts_with = ?12, supersedes = ?13
            WHERE id = ?1
            "#,
            params![
                anchor.id,
                anchor.rule_text,
                trigger_tags_json,
                anchor.confidence,
                anchor.created_at.to_rfc3339(),
                anchor.last_validated.to_rfc3339(),
                anchor.validation_count,
                anchor.failure_count,
                source_json,
                scope_json,
                anchor.priority,
                conflicts_with_json,
                anchor.supersedes,
            ],
        )?;

        Ok(())
    }

    /// Delete a behavioral anchor by ID
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the anchor to delete
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Ok if deletion was successful
    pub fn delete(&mut self, id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM behavioral_anchors WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// List all behavioral anchors, sorted by priority and confidence
    ///
    /// # Returns
    ///
    /// * `Result<Vec<BehavioralAnchor>>` - All anchors sorted by priority DESC, confidence DESC
    pub fn list_all(&self) -> Result<Vec<BehavioralAnchor>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, rule_text, trigger_tags, confidence,
                   created_at, last_validated, validation_count, failure_count,
                   source, scope, priority, conflicts_with, supersedes
            FROM behavioral_anchors
            ORDER BY priority DESC, confidence DESC
            "#,
        )?;

        let anchors = stmt.query_map([], |row| {
            let trigger_tags_json: String = row.get(2)?;
            let source_json: String = row.get(8)?;
            let scope_json: String = row.get(9)?;
            let conflicts_with_json: String = row.get(11)?;

            let trigger_tags: Vec<String> = serde_json::from_str(&trigger_tags_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e)))?;
            let source: AnchorSource = serde_json::from_str(&source_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e)))?;
            let scope: AnchorScope = serde_json::from_str(&scope_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e)))?;
            let conflicts_with: Vec<String> = serde_json::from_str(&conflicts_with_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(e)))?;

            let created_at_str: String = row.get(4)?;
            let last_validated_str: String = row.get(5)?;

            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e)))?
                .with_timezone(&Utc);
            let last_validated = DateTime::parse_from_rfc3339(&last_validated_str)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e)))?
                .with_timezone(&Utc);

            Ok(BehavioralAnchor {
                id: row.get(0)?,
                rule_text: row.get(1)?,
                trigger_tags,
                confidence: row.get(3)?,
                created_at,
                last_validated,
                validation_count: row.get(6)?,
                failure_count: row.get(7)?,
                source,
                scope,
                priority: row.get(10)?,
                conflicts_with,
                supersedes: row.get(12)?,
            })
        })?;

        anchors.collect()
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::memory::cortex::meta_cognition::schema::initialize_schema;

    #[test]
    fn test_add_and_get_anchor() {
        // Create in-memory database and initialize schema
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        initialize_schema(&conn).unwrap();
        let mut store = AnchorStore::new(conn);

        // Create a test anchor
        let anchor = BehavioralAnchor::new(
            "test-id-1".to_string(),
            "Always check Python version before running scripts".to_string(),
            vec!["Python".to_string(), "macOS".to_string()],
            AnchorSource::ReactiveReflection {
                task_id: "task-123".to_string(),
                error_type: "VersionMismatch".to_string(),
            },
            AnchorScope::Tagged {
                tags: vec!["Python".to_string()],
            },
            100,
            0.8,
        );

        // Add the anchor
        let id = store.add(anchor.clone()).unwrap();
        assert_eq!(id, "test-id-1");

        // Retrieve the anchor
        let retrieved = store.get(&id).unwrap();
        assert!(retrieved.is_some());

        let retrieved_anchor = retrieved.unwrap();
        assert_eq!(retrieved_anchor.id, anchor.id);
        assert_eq!(retrieved_anchor.rule_text, anchor.rule_text);
        assert_eq!(retrieved_anchor.trigger_tags, anchor.trigger_tags);
        assert_eq!(retrieved_anchor.confidence, anchor.confidence);
        assert_eq!(retrieved_anchor.priority, anchor.priority);
    }

    #[test]
    fn test_update_anchor() {
        // Create in-memory database and initialize schema
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        initialize_schema(&conn).unwrap();
        let mut store = AnchorStore::new(conn);

        // Create and add a test anchor
        let mut anchor = BehavioralAnchor::new(
            "test-id-2".to_string(),
            "Original rule text".to_string(),
            vec!["tag1".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            50,
            0.5,
        );
        store.add(anchor.clone()).unwrap();

        // Update the anchor
        anchor.rule_text = "Updated rule text".to_string();
        anchor.confidence = 0.9;
        anchor.priority = 200;
        store.update(&anchor).unwrap();

        // Retrieve and verify the update
        let retrieved = store.get("test-id-2").unwrap().unwrap();
        assert_eq!(retrieved.rule_text, "Updated rule text");
        assert_eq!(retrieved.confidence, 0.9);
        assert_eq!(retrieved.priority, 200);
    }

    #[test]
    fn test_delete_anchor() {
        // Create in-memory database and initialize schema
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        initialize_schema(&conn).unwrap();
        let mut store = AnchorStore::new(conn);

        // Create and add a test anchor
        let anchor = BehavioralAnchor::new(
            "test-id-3".to_string(),
            "Rule to be deleted".to_string(),
            vec![],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            50,
            0.5,
        );
        store.add(anchor).unwrap();

        // Verify it exists
        assert!(store.get("test-id-3").unwrap().is_some());

        // Delete the anchor
        store.delete("test-id-3").unwrap();

        // Verify it's gone
        assert!(store.get("test-id-3").unwrap().is_none());
    }

    #[test]
    fn test_list_all_anchors() {
        // Create in-memory database and initialize schema
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        initialize_schema(&conn).unwrap();
        let mut store = AnchorStore::new(conn);

        // Add multiple anchors with different priorities and confidences
        let anchor1 = BehavioralAnchor::new(
            "test-id-4".to_string(),
            "Low priority anchor".to_string(),
            vec![],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            10,
            0.5,
        );

        let anchor2 = BehavioralAnchor::new(
            "test-id-5".to_string(),
            "High priority anchor".to_string(),
            vec![],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            100,
            0.8,
        );

        let anchor3 = BehavioralAnchor::new(
            "test-id-6".to_string(),
            "Medium priority, high confidence".to_string(),
            vec![],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            50,
            0.95,
        );

        store.add(anchor1).unwrap();
        store.add(anchor2).unwrap();
        store.add(anchor3).unwrap();

        // List all anchors
        let all_anchors = store.list_all().unwrap();
        assert_eq!(all_anchors.len(), 3);

        // Verify sorting: priority DESC, then confidence DESC
        assert_eq!(all_anchors[0].id, "test-id-5"); // priority 100
        assert_eq!(all_anchors[1].id, "test-id-6"); // priority 50, confidence 0.95
        assert_eq!(all_anchors[2].id, "test-id-4"); // priority 10
    }
}
