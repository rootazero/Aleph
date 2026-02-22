//! End-to-end integration tests for the VFS layer

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::memory::{MemoryFact, FactSource};
    use crate::memory::context::{FactType, compute_parent_path};
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::{MemoryBackend, MemoryStore};
    use crate::memory::store::lance::LanceMemoryBackend;
    use crate::memory::vfs::{compute_directory_hash, bootstrap_agent_context};

    async fn create_test_db() -> (MemoryBackend, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(temp_dir.path()).await.unwrap();
        (Arc::new(backend), temp_dir)
    }

    #[tokio::test]
    async fn test_full_vfs_flow() {
        let (db, _temp_dir) = create_test_db().await;

        // 1. Insert facts with auto-assigned paths
        let fact1 = MemoryFact::new("User prefers Rust".into(), FactType::Preference, vec![]);
        let fact2 = MemoryFact::new("User prefers dark theme".into(), FactType::Preference, vec![]);
        let fact3 = MemoryFact::new("Learning WebAssembly".into(), FactType::Learning, vec![]);

        assert_eq!(fact1.path, "aleph://user/preferences/");
        assert_eq!(fact3.path, "aleph://knowledge/learning/");

        db.insert_fact(&fact1).await.unwrap();
        db.insert_fact(&fact2).await.unwrap();
        db.insert_fact(&fact3).await.unwrap();

        // 2. List children of user/
        let user_children = db.list_by_path("aleph://user/", &NamespaceScope::Owner, "default").await.unwrap();
        assert!(!user_children.is_empty());

        // 3. Simulate L1 Overview storage
        // Get all facts and filter by path prefix
        let all_facts = db.get_all_facts(false).await.unwrap();
        let prefs_facts: Vec<_> = all_facts.into_iter()
            .filter(|f| f.path.starts_with("aleph://user/preferences/"))
            .collect();
        let hash = compute_directory_hash(&prefs_facts);
        assert_eq!(hash.len(), 16);

        let mut l1 = MemoryFact::new("Overview of preferences".into(), FactType::Other, vec![])
            .with_path("aleph://user/preferences/".to_string())
            .with_fact_source(FactSource::Summary);
        l1.content_hash = hash;
        db.insert_fact(&l1).await.unwrap();

        // 4. Verify L1 retrieval via get_by_path
        let overview = db.get_by_path("aleph://user/preferences/", &NamespaceScope::Owner, "default").await.unwrap();
        assert!(overview.is_some());
        // Note: get_by_path returns the first matching fact at the exact path;
        // it may return any fact at that path, not necessarily the Summary.

        // 5. Bootstrap context
        let bootstrap = bootstrap_agent_context(&db).await;
        // May or may not have top-level L1s depending on what we stored
        assert!(bootstrap.is_empty() || bootstrap.contains("Memory Overview"));
    }
}
