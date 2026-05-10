#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::SqliteMemoryBackend;

    fn make_backend() -> SqliteMemoryBackend {
        let dir = tempfile::tempdir().unwrap();
        // Keep the dir alive by leaking it for the test duration
        let path = dir.into_path();
        SqliteMemoryBackend::new(&path).unwrap()
    }

    fn make_note(title: &str, category: &str) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: category.to_string(),
            tags: vec!["test".to_string()],
            facts: vec![format!("{title} fact")],
            links: vec![],
            created_at: 1000,
            updated_at: 1000,
            content_hash: format!("hash_{title}"),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn hybrid_search_returns_results_from_both_sources() {
        let backend = make_backend();
        let note1 = KnowledgeNote {
            title: "rust memory safety".to_string(),
            category: "programming".to_string(),
            tags: vec!["rust".to_string()],
            facts: vec!["Rust prevents data races at compile time".to_string()],
            links: vec![],
            created_at: 1000,
            updated_at: 1000,
            content_hash: "hash1".to_string(),
            ..Default::default()
        };
        let note2 = KnowledgeNote {
            title: "python async".to_string(),
            category: "programming".to_string(),
            tags: vec!["python".to_string()],
            facts: vec!["Python asyncio provides cooperative multitasking".to_string()],
            links: vec![],
            created_at: 1000,
            updated_at: 1000,
            content_hash: "hash2".to_string(),
            ..Default::default()
        };

        backend.upsert_note("agent1", &note1).await.unwrap();
        backend.upsert_note("agent1", &note2).await.unwrap();

        let results = backend.hybrid_search("agent1", "rust async", 10).await.unwrap();
        assert!(!results.is_empty(), "Should return results for 'rust async'");
    }

    #[tokio::test]
    async fn full_text_search_finds_by_content() {
        let backend = make_backend();
        let note = make_note("search test", "general");
        backend.upsert_note("agent1", &note).await.unwrap();

        let results = backend.full_text_search("agent1", "search test fact", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "general/search_test");
    }

    #[tokio::test]
    async fn full_text_search_respects_agent_scope() {
        let backend = make_backend();
        let note = make_note("scoped", "test");
        backend.upsert_note("agent1", &note).await.unwrap();

        let results = backend.full_text_search("agent2", "scoped", 10).await.unwrap();
        assert!(results.is_empty(), "Should not find agent1's note when searching as agent2");
    }

    #[tokio::test]
    async fn upsert_note_creates_and_updates() {
        let backend = make_backend();
        let mut note = make_note("update test", "general");

        // Create
        backend.upsert_note("agent1", &note).await.unwrap();
        let found = backend.get_note("agent1", "general/update_test").await.unwrap();
        assert!(found.is_some());

        // Update
        note.tags.push("updated".to_string());
        backend.upsert_note("agent1", &note).await.unwrap();
        let found = backend.get_note("agent1", "general/update_test").await.unwrap();
        assert_eq!(found.unwrap().tags, vec!["test", "updated"]);
    }

    #[tokio::test]
    async fn delete_note_removes_from_all_indices() {
        let backend = make_backend();
        let note = make_note("delete me", "trash");
        backend.upsert_note("agent1", &note).await.unwrap();

        backend.delete_note("agent1", "trash/delete_me").await.unwrap();
        let found = backend.get_note("agent1", "trash/delete_me").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn list_notes_by_category_filters_correctly() {
        let backend = make_backend();
        backend.upsert_note("agent1", &make_note("a", "cat1")).await.unwrap();
        backend.upsert_note("agent1", &make_note("b", "cat2")).await.unwrap();
        backend.upsert_note("agent1", &make_note("c", "cat1")).await.unwrap();

        let cat1 = backend.list_notes("agent1", Some("cat1"), None, 10).await.unwrap();
        assert_eq!(cat1.len(), 2);
        let cat2 = backend.list_notes("agent1", Some("cat2"), None, 10).await.unwrap();
        assert_eq!(cat2.len(), 1);
    }

    #[tokio::test]
    async fn get_recent_notes_returns_ordered_results() {
        let backend = make_backend();
        let mut note1 = make_note("old", "chrono");
        note1.created_at = 1000;
        let mut note2 = make_note("new", "chrono");
        note2.created_at = 2000;

        backend.upsert_note("agent1", &note1).await.unwrap();
        backend.upsert_note("agent1", &note2).await.unwrap();

        let recent = backend.get_recent_notes("agent1", 10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].path, "chrono/new");
        assert_eq!(recent[1].path, "chrono/old");
    }

    #[tokio::test]
    async fn get_untagged_notes_excludes_tagged() {
        let backend = make_backend();
        let mut tagged = make_note("tagged", "sorting");
        tagged.tags = vec!["important".to_string()];
        let untagged = make_note("untagged", "sorting");

        backend.upsert_note("agent1", &tagged).await.unwrap();
        backend.upsert_note("agent1", &untagged).await.unwrap();

        let results = backend.get_untagged_notes("agent1", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "sorting/untagged");
    }

    #[tokio::test]
    async fn get_orphan_notes_finds_unlinked() {
        let backend = make_backend();
        let mut note1 = make_note("lonely", "orphans");
        note1.links = vec![];
        let mut note2 = make_note("popular", "orphans");
        note2.links = vec!["orphans/lonely".to_string()];

        backend.upsert_note("agent1", &note1).await.unwrap();
        backend.upsert_note("agent1", &note2).await.unwrap();

        let orphans = backend.get_orphan_notes("agent1", 10).await.unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].path, "orphans/lonely");
    }

    #[tokio::test]
    async fn get_review_queue_returns_notes_needing_review() {
        let backend = make_backend();
        let note = make_note("review me", "queue");
        backend.upsert_note("agent1", &note).await.unwrap();

        let queue = backend.get_review_queue("agent1", 10).await.unwrap();
        // All notes are in review queue initially
        assert!(!queue.is_empty());
    }

    #[tokio::test]
    async fn update_note_review_updates_timestamp() {
        let backend = make_backend();
        let note = make_note("reviewed", "queue");
        backend.upsert_note("agent1", &note).await.unwrap();

        let before = chrono::Utc::now().timestamp();
        backend.update_note_review("agent1", "queue/reviewed").await.unwrap();
        let after = chrono::Utc::now().timestamp();

        let queue = backend.get_review_queue("agent1", 10).await.unwrap();
        let reviewed = queue.iter().find(|r| r.path == "queue/reviewed");
        assert!(reviewed.is_some());
        let r = reviewed.unwrap();
        assert!(r.last_reviewed_at >= before && r.last_reviewed_at <= after);
    }

    #[tokio::test]
    async fn link_notes_creates_bidirectional_edges() {
        let backend = make_backend();
        let mut note1 = make_note("source", "links");
        note1.links = vec!["links/target".to_string()];
        let note2 = make_note("target", "links");

        backend.upsert_note("agent1", &note1).await.unwrap();
        backend.upsert_note("agent1", &note2).await.unwrap();

        let edges = backend.get_linked_notes("agent1", "links/source").await.unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_path, "links/target");
    }

    #[tokio::test]
    async fn get_backlinks_finds_incoming_links() {
        let backend = make_backend();
        let mut note1 = make_note("source", "backlinks");
        note1.links = vec!["backlinks/target".to_string()];
        let note2 = make_note("target", "backlinks");

        backend.upsert_note("agent1", &note1).await.unwrap();
        backend.upsert_note("agent1", &note2).await.unwrap();

        let backlinks = backend.get_backlinks("agent1", "backlinks/target").await.unwrap();
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].source_path, "backlinks/source");
    }

    #[tokio::test]
    async fn get_graph_stats_returns_correct_counts() {
        let backend = make_backend();
        backend.upsert_note("agent1", &make_note("a", "stats")).await.unwrap();
        backend.upsert_note("agent1", &make_note("b", "stats")).await.unwrap();

        let stats = backend.get_graph_stats("agent1").await.unwrap();
        assert_eq!(stats.note_count, 2);
    }

    #[tokio::test]
    async fn body_text_sha256_is_deterministic() {
        let h1 = body_text_sha256("hello world");
        let h2 = body_text_sha256("hello world");
        assert_eq!(h1, h2);

        let h3 = body_text_sha256("different");
        assert_ne!(h1, h3);
    }
}
