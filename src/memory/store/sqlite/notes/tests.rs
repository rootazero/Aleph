#[cfg(test)]
mod tests {

    use super::super::body_text_sha256;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::SqliteMemoryBackend;

    fn make_backend() -> SqliteMemoryBackend {
        SqliteMemoryBackend::in_memory().unwrap()
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
    async fn fts_search_finds_by_content() {
        let backend = make_backend();
        let note1 = make_note("rust async", "general");
        let note2 = make_note("python sync", "general");

        backend
            .index_note(&note1, "agent1", "general")
            .await
            .unwrap();
        backend
            .index_note(&note2, "agent1", "general")
            .await
            .unwrap();

        let results = backend
            .search_notes_fts("rust async", "agent1", 10)
            .await
            .unwrap();
        assert!(
            !results.is_empty(),
            "Should return results for 'rust async'"
        );
    }

    #[tokio::test]
    async fn fts_search_finds_by_content_direct() {
        let backend = make_backend();
        let note = make_note("search test", "general");
        backend
            .index_note(&note, "agent1", "general")
            .await
            .unwrap();

        let results = backend
            .search_notes_fts("search test fact", "agent1", 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "general/search test");
    }

    #[tokio::test]
    async fn fts_search_respects_agent_scope() {
        let backend = make_backend();
        let note = make_note("scoped", "test");
        backend.index_note(&note, "agent1", "test").await.unwrap();

        let results = backend
            .search_notes_fts("scoped", "agent2", 10)
            .await
            .unwrap();
        assert!(
            results.is_empty(),
            "Should not find agent1's note when searching as agent2"
        );
    }

    #[tokio::test]
    async fn index_note_creates_and_updates() {
        let backend = make_backend();
        let mut note = make_note("update test", "general");

        // Create
        backend
            .index_note(&note, "agent1", "general")
            .await
            .unwrap();
        let found = backend
            .get_note_index("general/update test", "agent1")
            .await
            .unwrap();
        assert!(found.is_some());

        // Update
        note.tags.push("updated".to_string());
        backend
            .index_note(&note, "agent1", "general")
            .await
            .unwrap();
        let found = backend
            .get_note_index("general/update test", "agent1")
            .await
            .unwrap();
        assert!(found.is_some());
    }

    #[tokio::test]
    async fn remove_note_index_removes_entry() {
        let backend = make_backend();
        let note = make_note("delete me", "trash");
        backend.index_note(&note, "agent1", "trash").await.unwrap();

        backend
            .remove_note_index("trash/delete_me", "agent1")
            .await
            .unwrap();
        let found = backend
            .get_note_index("trash/delete_me", "agent1")
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn list_notes_by_category_filters_correctly() {
        let backend = make_backend();
        backend
            .index_note(&make_note("a", "cat1"), "agent1", "cat1")
            .await
            .unwrap();
        backend
            .index_note(&make_note("b", "cat2"), "agent1", "cat2")
            .await
            .unwrap();
        backend
            .index_note(&make_note("c", "cat1"), "agent1", "cat1")
            .await
            .unwrap();

        let cat1 = backend
            .get_notes_by_category("agent1", "cat1", 10)
            .await
            .unwrap();
        assert_eq!(cat1.len(), 2);
        let cat2 = backend
            .get_notes_by_category("agent1", "cat2", 10)
            .await
            .unwrap();
        assert_eq!(cat2.len(), 1);
    }

    #[tokio::test]
    async fn list_notes_returns_all_for_agent() {
        let backend = make_backend();
        let mut note1 = make_note("old", "chrono");
        note1.created_at = 1000;
        let mut note2 = make_note("new", "chrono");
        note2.created_at = 2000;

        backend
            .index_note(&note1, "agent1", "chrono")
            .await
            .unwrap();
        backend
            .index_note(&note2, "agent1", "chrono")
            .await
            .unwrap();

        let notes = backend.list_notes("agent1").await.unwrap();
        assert_eq!(notes.len(), 2);
    }

    #[tokio::test]
    async fn link_notes_creates_outgoing_edges() {
        let backend = make_backend();
        let mut note1 = make_note("source", "links");
        note1.links = vec!["links/target".to_string()];
        let note2 = make_note("target", "links");

        backend.index_note(&note1, "agent1", "links").await.unwrap();
        backend.index_note(&note2, "agent1", "links").await.unwrap();

        let edges = backend
            .get_outgoing_links("links/source", "agent1")
            .await
            .unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0], "links/target");
    }

    #[tokio::test]
    async fn get_incoming_links_finds_backlinks() {
        let backend = make_backend();
        let mut note1 = make_note("source", "backlinks");
        note1.links = vec!["backlinks/target".to_string()];
        let note2 = make_note("target", "backlinks");

        backend
            .index_note(&note1, "agent1", "backlinks")
            .await
            .unwrap();
        backend
            .index_note(&note2, "agent1", "backlinks")
            .await
            .unwrap();

        let backlinks = backend
            .get_incoming_links("backlinks/target", "agent1")
            .await
            .unwrap();
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0], "backlinks/source");
    }

    #[tokio::test]
    async fn incoming_links_any_matches_fullpath_and_filename() {
        let backend = make_backend();
        const AGENT: &str = "agent1";

        // Target note: path "reference/target", filename "target".
        backend
            .index_note(
                &KnowledgeNote {
                    title: "target".into(),
                    category: "reference".into(),
                    content_hash: "h0".into(),
                    ..Default::default()
                },
                AGENT,
                "reference",
            )
            .await
            .unwrap();

        // Source A links by full path -> resolve_target keeps full path in to_note.
        backend
            .index_note(
                &KnowledgeNote {
                    title: "srcA".into(),
                    category: "notes".into(),
                    links: vec!["reference/target".into()],
                    content_hash: "hA".into(),
                    ..Default::default()
                },
                AGENT,
                "notes",
            )
            .await
            .unwrap();

        // Source B: force a row whose to_note is the BARE filename (legacy shape).
        backend
            .index_note(
                &KnowledgeNote {
                    title: "srcB".into(),
                    category: "notes".into(),
                    content_hash: "hB".into(),
                    ..Default::default()
                },
                AGENT,
                "notes",
            )
            .await
            .unwrap();
        backend
            .add_link_with_relation(AGENT, "notes/srcB", "target", "related")
            .await
            .unwrap();

        let incoming = backend
            .get_incoming_links_any("reference/target", "target", AGENT)
            .await
            .unwrap();
        assert!(
            incoming.iter().any(|f| f == "notes/srcA"),
            "full-path row missing: {incoming:?}"
        );
        assert!(
            incoming.iter().any(|f| f == "notes/srcB"),
            "bare-filename row missing: {incoming:?}"
        );
    }

    #[tokio::test]
    async fn count_all_notes_returns_correct_count() {
        let backend = make_backend();
        backend
            .index_note(&make_note("a", "stats"), "agent1", "stats")
            .await
            .unwrap();
        backend
            .index_note(&make_note("b", "stats"), "agent1", "stats")
            .await
            .unwrap();

        let count = backend.count_all_notes().await.unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn body_text_sha256_is_deterministic() {
        let h1 = body_text_sha256("hello world");
        let h2 = body_text_sha256("hello world");
        assert_eq!(h1, h2);

        let h3 = body_text_sha256("different");
        assert_ne!(h1, h3);
    }

    #[test]
    fn retrieval_tuning_default_matches_legacy_constants() {
        let t = crate::memory::store::sqlite::RetrievalTuning::default();
        assert_eq!(t.rrf_k, 60);
        assert_eq!(t.bm25_bonus_weight, 0.15);
    }

    #[test]
    fn with_retrieval_tuning_overrides_fields() {
        let backend = crate::memory::store::sqlite::SqliteMemoryBackend::in_memory()
            .unwrap()
            .with_retrieval_tuning(42, 0.5);
        assert_eq!(backend.tuning.rrf_k, 42);
        assert_eq!(backend.tuning.bm25_bonus_weight, 0.5);
    }

    #[tokio::test]
    async fn get_typed_relations_returns_typed_edges_only() {
        use crate::memory::notes::Relation;

        let backend = make_backend();
        const AGENT: &str = "main";

        // Step 1: index bob (plain target, no outgoing typed relations).
        let bob = KnowledgeNote {
            title: "bob".into(),
            category: "entity".into(),
            content_hash: "hash_bob".into(),
            created_at: 1000,
            updated_at: 1000,
            ..Default::default()
        };
        backend.index_note(&bob, AGENT, "entity").await.unwrap();

        // Step 2: index alice with both a plain wikilink AND a typed relation to bob.
        let mut alice = KnowledgeNote {
            title: "alice".into(),
            category: "entity".into(),
            content_hash: "hash_alice".into(),
            created_at: 1000,
            updated_at: 1000,
            ..Default::default()
        };
        alice.links = vec!["entity/bob".into()];
        alice.relations = vec![Relation {
            to: "entity/bob".into(),
            rel_type: "colleague".into(),
            confidence: 0.7,
        }];
        backend.index_note(&alice, AGENT, "entity").await.unwrap();

        // Step 3: typed query for alice must return the typed edge.
        let alice_rels = backend
            .get_typed_relations("entity/alice", AGENT)
            .await
            .unwrap();
        assert_eq!(
            alice_rels,
            vec![("entity/bob".to_string(), "colleague".to_string())],
            "get_typed_relations must return the typed edge for entity/alice"
        );

        // Step 4: typed query for bob must be empty (bob has no outgoing typed relations).
        let bob_rels = backend
            .get_typed_relations("entity/bob", AGENT)
            .await
            .unwrap();
        assert!(
            bob_rels.is_empty(),
            "bob has no outgoing typed relations; result must be empty"
        );
    }

    #[tokio::test]
    async fn index_note_writes_typed_relation_column() {
        use crate::memory::notes::Relation;

        let backend = make_backend();
        const AGENT: &str = "main";

        // Seed target note first so resolution can find it.
        let target = KnowledgeNote {
            title: "bob".into(),
            category: "entity".into(),
            content_hash: "hash_bob".into(),
            created_at: 1000,
            updated_at: 1000,
            ..Default::default()
        };
        backend.index_note(&target, AGENT, "entity").await.unwrap();

        // Alice has both a plain body wikilink AND a typed frontmatter relation
        // to the same target "entity/bob". The typed relation must win.
        let mut alice = KnowledgeNote {
            title: "alice".into(),
            category: "entity".into(),
            content_hash: "hash_alice".into(),
            created_at: 1000,
            updated_at: 1000,
            ..Default::default()
        };
        alice.links = vec!["entity/bob".into()];
        alice.relations = vec![Relation {
            to: "entity/bob".into(),
            rel_type: "colleague".into(),
            confidence: 0.7,
        }];
        backend.index_note(&alice, AGENT, "entity").await.unwrap();

        // Read the relation column directly via the test conn accessor.
        let conn = backend.conn().lock().unwrap();
        let relation: Option<String> = conn
            .query_row(
                "SELECT relation FROM notes_links \
                 WHERE agent_id = ?1 AND from_note = ?2 AND to_note = ?3",
                rusqlite::params![AGENT, "entity/alice", "entity/bob"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            relation.as_deref(),
            Some("colleague"),
            "typed relation must override plain wikilink for the same target"
        );
    }

    #[tokio::test]
    async fn add_link_with_relation_sets_relation_column() {
        let backend = make_backend();
        const AGENT: &str = "default";

        // Index both notes so the paths exist in notes_index.
        backend
            .index_note(&make_note("a", "cat"), AGENT, "cat")
            .await
            .unwrap();
        backend
            .index_note(&make_note("b", "cat"), AGENT, "cat")
            .await
            .unwrap();

        // Insert a typed link via the new method.
        backend
            .add_link_with_relation(AGENT, "cat/a", "cat/b", "shared-topic")
            .await
            .unwrap();

        // get_typed_relations returns (to_note, relation) for rows with non-NULL relation.
        let rels = backend.get_typed_relations("cat/a", AGENT).await.unwrap();
        assert!(
            rels.iter()
                .any(|(to, rel)| to == "cat/b" && rel == "shared-topic"),
            "expected typed relation cat/a -> cat/b with 'shared-topic', got: {rels:?}"
        );
    }
}
