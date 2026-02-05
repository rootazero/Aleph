/// Audit log database operations
///
/// Provides methods for storing and retrieving audit log entries for memory operations.
use crate::error::AlephError;
use crate::memory::audit::{AuditAction, AuditActor, AuditDetails, AuditEntry};
use crate::memory::database::VectorDatabase;
use rusqlite::params;

impl VectorDatabase {
    /// Insert an audit log entry
    pub async fn insert_audit_entry(&self, entry: &AuditEntry) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let details_json = entry.details_json();

        conn.execute(
            r#"
            INSERT INTO memory_audit_log (id, fact_id, action, reason, actor, details, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                entry.id,
                entry.fact_id,
                entry.action.to_string(),
                entry.reason,
                entry.actor.to_string(),
                details_json,
                entry.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert audit entry: {}", e)))?;

        Ok(())
    }

    /// Get audit entries for a specific fact
    pub async fn get_audit_entries_for_fact(
        &self,
        fact_id: &str,
    ) -> Result<Vec<AuditEntry>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, fact_id, action, reason, actor, details, created_at
                FROM memory_audit_log
                WHERE fact_id = ?1
                ORDER BY created_at ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare statement: {}", e)))?;

        let entries = stmt
            .query_map(params![fact_id], |row| Ok(Self::row_to_audit_entry(row)))
            .map_err(|e| AlephError::config(format!("Failed to query audit entries: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }

    /// Get recent audit entries
    pub async fn get_recent_audit_entries(
        &self,
        limit: usize,
    ) -> Result<Vec<AuditEntry>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, fact_id, action, reason, actor, details, created_at
                FROM memory_audit_log
                ORDER BY created_at DESC
                LIMIT ?1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare statement: {}", e)))?;

        let entries = stmt
            .query_map(params![limit], |row| Ok(Self::row_to_audit_entry(row)))
            .map_err(|e| AlephError::config(format!("Failed to query audit entries: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }

    fn row_to_audit_entry(row: &rusqlite::Row) -> AuditEntry {
        let action_str: String = row.get(2).unwrap_or_default();
        let actor_str: String = row.get(4).unwrap_or_default();
        let details_json: Option<String> = row.get(5).ok();

        AuditEntry {
            id: row.get(0).unwrap_or_default(),
            fact_id: row.get(1).unwrap_or_default(),
            action: action_str.parse().unwrap_or(AuditAction::Created),
            reason: row.get(3).ok(),
            actor: actor_str.parse().unwrap_or(AuditActor::System),
            details: details_json.and_then(|j| AuditEntry::parse_details(&j)),
            created_at: row.get(6).unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::database::VectorDatabase;
    use tempfile::tempdir;

    fn create_test_db() -> VectorDatabase {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_audit.db");
        // Keep temp_dir alive by leaking it (acceptable in tests)
        let _ = Box::leak(Box::new(temp_dir));
        VectorDatabase::new(db_path).unwrap()
    }

    #[tokio::test]
    async fn test_insert_and_retrieve_audit_entry() {
        let db = create_test_db();

        let entry = AuditEntry::new(
            "fact-123".to_string(),
            AuditAction::Created,
            AuditActor::Agent,
            Some("Test creation".to_string()),
            Some(AuditDetails::Created {
                source: "session".to_string(),
                extraction_context: Some("User preference".to_string()),
            }),
        );

        db.insert_audit_entry(&entry).await.unwrap();

        let entries = db.get_audit_entries_for_fact("fact-123").await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].fact_id, "fact-123");
        assert_eq!(entries[0].action, AuditAction::Created);
        assert_eq!(entries[0].actor, AuditActor::Agent);
    }

    #[tokio::test]
    async fn test_get_fact_history_chronological_order() {
        let db = create_test_db();

        // Insert entries with different timestamps
        for action in [
            AuditAction::Created,
            AuditAction::Accessed,
            AuditAction::Invalidated,
        ] {
            let entry = AuditEntry::new(
                "fact-456".to_string(),
                action,
                AuditActor::Agent,
                None,
                None,
            );
            db.insert_audit_entry(&entry).await.unwrap();
            // Small delay to ensure different timestamps
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        let entries = db.get_audit_entries_for_fact("fact-456").await.unwrap();
        assert_eq!(entries.len(), 3);
        // Entries should be in chronological order (oldest first)
        assert_eq!(entries[0].action, AuditAction::Created);
        assert_eq!(entries[1].action, AuditAction::Accessed);
        assert_eq!(entries[2].action, AuditAction::Invalidated);
    }

    #[tokio::test]
    async fn test_get_recent_audit_entries() {
        let db = create_test_db();

        // Insert 5 entries
        for i in 0..5 {
            let entry = AuditEntry::new(
                format!("fact-{}", i),
                AuditAction::Created,
                AuditActor::Agent,
                None,
                None,
            );
            db.insert_audit_entry(&entry).await.unwrap();
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Get only 3 most recent
        let entries = db.get_recent_audit_entries(3).await.unwrap();
        assert_eq!(entries.len(), 3);
        // Most recent should be first
        assert_eq!(entries[0].fact_id, "fact-4");
        assert_eq!(entries[1].fact_id, "fact-3");
        assert_eq!(entries[2].fact_id, "fact-2");
    }

    #[tokio::test]
    async fn test_audit_entry_with_details() {
        let db = create_test_db();

        let entry = AuditEntry::new(
            "fact-789".to_string(),
            AuditAction::Accessed,
            AuditActor::Agent,
            None,
            Some(AuditDetails::Accessed {
                query: Some("what is my favorite color?".to_string()),
                relevance_score: Some(0.95),
                used_in_response: true,
            }),
        );

        db.insert_audit_entry(&entry).await.unwrap();

        let entries = db.get_audit_entries_for_fact("fact-789").await.unwrap();
        assert_eq!(entries.len(), 1);

        if let Some(AuditDetails::Accessed {
            query,
            relevance_score,
            used_in_response,
        }) = &entries[0].details
        {
            assert_eq!(query, &Some("what is my favorite color?".to_string()));
            assert_eq!(*relevance_score, Some(0.95));
            assert!(*used_in_response);
        } else {
            panic!("Expected Accessed details");
        }
    }

    #[tokio::test]
    async fn test_audit_entry_invalidated_with_details() {
        let db = create_test_db();

        let entry = AuditEntry::new(
            "fact-inv".to_string(),
            AuditAction::Invalidated,
            AuditActor::Decay,
            Some("Strength below threshold".to_string()),
            Some(AuditDetails::Invalidated {
                reason: "decay".to_string(),
                strength_at_invalidation: Some(0.08),
            }),
        );

        db.insert_audit_entry(&entry).await.unwrap();

        let entries = db.get_audit_entries_for_fact("fact-inv").await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].actor, AuditActor::Decay);

        if let Some(AuditDetails::Invalidated {
            reason,
            strength_at_invalidation,
        }) = &entries[0].details
        {
            assert_eq!(reason, "decay");
            assert_eq!(*strength_at_invalidation, Some(0.08));
        } else {
            panic!("Expected Invalidated details");
        }
    }

    #[tokio::test]
    async fn test_empty_fact_history() {
        let db = create_test_db();

        let entries = db
            .get_audit_entries_for_fact("nonexistent-fact")
            .await
            .unwrap();
        assert!(entries.is_empty());
    }
}
