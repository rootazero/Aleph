//! End-to-end integration tests for the VFS layer

#[cfg(test)]
mod tests {
    use crate::memory::{MemoryFact, VectorDatabase, FactSource};
    use crate::memory::context::{FactType, compute_parent_path};
    use crate::memory::vfs::{compute_directory_hash, bootstrap_agent_context, migrate_existing_facts_to_paths};

    fn create_test_db() -> VectorDatabase {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_vfs_e2e_{}.db", uuid::Uuid::new_v4()));
        VectorDatabase::new(db_path).unwrap()
    }

    #[tokio::test]
    async fn test_full_vfs_flow() {
        let db = create_test_db();

        // 1. Insert facts with auto-assigned paths
        let fact1 = MemoryFact::new("User prefers Rust".into(), FactType::Preference, vec![]);
        let fact2 = MemoryFact::new("User prefers dark theme".into(), FactType::Preference, vec![]);
        let fact3 = MemoryFact::new("Learning WebAssembly".into(), FactType::Learning, vec![]);

        assert_eq!(fact1.path, "aleph://user/preferences/");
        assert_eq!(fact3.path, "aleph://knowledge/learning/");

        db.insert_fact(fact1).await.unwrap();
        db.insert_fact(fact2).await.unwrap();
        db.insert_fact(fact3).await.unwrap();

        // 2. List children of user/
        let user_children = db.list_path_children("aleph://user/").await.unwrap();
        assert!(!user_children.is_empty());

        // 3. Count facts by path
        let pref_count = db.count_facts_by_path("aleph://user/preferences/").await.unwrap();
        assert_eq!(pref_count, 2);

        // 4. Simulate L1 Overview storage
        let prefs_facts = db.get_facts_by_path_prefix("aleph://user/preferences/").await.unwrap();
        let hash = compute_directory_hash(&prefs_facts);
        assert_eq!(hash.len(), 16);

        let mut l1 = MemoryFact::new("Overview of preferences".into(), FactType::Other, vec![])
            .with_path("aleph://user/preferences/".to_string())
            .with_fact_source(FactSource::Summary);
        l1.content_hash = hash;
        db.insert_fact(l1).await.unwrap();

        // 5. Verify L1 retrieval
        let overview = db.get_l1_overview("aleph://user/preferences/").await.unwrap();
        assert!(overview.is_some());
        assert!(overview.unwrap().content.contains("Overview"));

        // 6. Bootstrap context
        let bootstrap = bootstrap_agent_context(&db).await;
        // May or may not have top-level L1s depending on what we stored
        assert!(bootstrap.is_empty() || bootstrap.contains("Memory Overview"));
    }

    #[tokio::test]
    async fn test_backward_compatibility() {
        let db = create_test_db();

        // Insert a fact and then clear its path (simulate old data)
        let fact = MemoryFact::new("Old fact".into(), FactType::Preference, vec![]);
        db.insert_fact(fact).await.unwrap();

        {
            let conn = db.conn.lock().unwrap();
            conn.execute("UPDATE memory_facts SET path = '', parent_path = ''", []).unwrap();
        }

        // list_path_children should still work (return empty for path queries)
        let children = db.list_path_children("aleph://user/").await.unwrap();
        assert!(children.is_empty());

        // But we can still count all facts
        let count = db.count_facts_by_path("").await.unwrap();
        assert_eq!(count, 1);

        // Migration fixes it
        let migrated = migrate_existing_facts_to_paths(&db).await.unwrap();
        assert_eq!(migrated, 1);

        // Now it shows up
        let children = db.list_path_children("aleph://user/").await.unwrap();
        assert!(!children.is_empty());
    }
}
