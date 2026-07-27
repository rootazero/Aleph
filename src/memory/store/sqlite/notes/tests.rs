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
    async fn fts_search_finds_cjk_substring_via_trigram() {
        // `unicode61` indexes a run of CJK ideographs as a single token, so the
        // substring `记忆管理` cannot match inside `记忆管理系统` on notes_fts
        // alone. The trigram companion (consulted for CJK-bearing queries ≥3
        // chars) enables the substring match. This also proves notes_fts_trigram
        // is created + dual-written by index_note.
        let backend = make_backend();
        let note = make_note("记忆管理系统", "general");
        backend
            .index_note(&note, "agent1", "general")
            .await
            .unwrap();

        let results = backend
            .search_notes_fts("记忆管理", "agent1", 10)
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "CJK substring query should find the note via the trigram companion"
        );
        assert_eq!(results[0].path, "general/记忆管理系统");
    }

    #[tokio::test]
    async fn fts_search_finds_multiword_cjk_via_trigram() {
        // A space-separated multi-word CJK query must substring-match each word
        // (per-term trigram OR), not be sent as one whole phrase — the interior
        // space would otherwise break the trigram match and find nothing.
        let backend = make_backend();
        let mut note = make_note("运维手册", "general");
        note.facts = vec!["记忆管理的系统运维方案".to_string()];
        backend
            .index_note(&note, "agent1", "general")
            .await
            .unwrap();

        let results = backend
            .search_notes_fts("记忆管理 系统运维", "agent1", 10)
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "multi-word CJK query should match via per-term trigram OR"
        );
        assert_eq!(results[0].path, "general/运维手册");
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
    async fn note_store_recovers_from_poisoned_lock() {
        // Regression (B3): a panic while another note-store call held the
        // connection mutex used to set the sticky poison flag, after which
        // `lock_conn!` returned Err on every subsequent call — a permanent
        // note-store outage. The lock is now recovered via into_inner().
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let backend = make_backend();
        let note = make_note("survivor", "general");
        backend
            .index_note(&note, "agent1", "general")
            .await
            .unwrap();

        // Poison the connection mutex by panicking while holding the guard.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = backend.conn().lock().unwrap();
            panic!("intentional poison for test");
        }));
        assert!(
            backend.conn().lock().is_err(),
            "precondition: the mutex must be poisoned after the panic"
        );

        // Must still succeed despite the poisoned lock.
        let results = backend
            .search_notes_fts("survivor", "agent1", 10)
            .await
            .expect("note store must recover from a poisoned lock");
        assert!(!results.is_empty(), "recovered call should find the note");
    }

    #[tokio::test]
    async fn fts_search_empty_query_returns_empty_not_error() {
        // Regression (B2): an empty / whitespace-only query built a bare
        // `MATCH '""'`, which FTS5 can reject as a syntax error. It must
        // degrade to "no results" instead of failing the whole query.
        let backend = make_backend();
        let note = make_note("anything", "general");
        backend
            .index_note(&note, "agent1", "general")
            .await
            .unwrap();

        for q in ["", "   ", "\t\n"] {
            let results = backend
                .search_notes_fts(q, "agent1", 10)
                .await
                .unwrap_or_else(|e| panic!("empty query {q:?} must not error: {e}"));
            assert!(results.is_empty(), "empty query {q:?} should yield no rows");
        }
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
    async fn remove_note_index_clears_embedding() {
        // Regression: deleted notes used to leave orphan vectors behind that
        // kept occupying KNN slots forever (no delete path cleared
        // notes_vec_map / notes_vec_{dim}).
        let backend = make_backend();
        let note = make_note("vectored", "reference");
        backend
            .index_note(&note, "agent1", "reference")
            .await
            .unwrap();
        let path = "reference/vectored";
        let embedding = vec![0.5_f32; 768];
        backend
            .upsert_embedding(path, "agent1", &embedding, 768)
            .await
            .unwrap();
        assert!(backend
            .get_embedding(path, "agent1", 768)
            .await
            .unwrap()
            .is_some());

        backend.remove_note_index(path, "agent1").await.unwrap();

        assert!(
            backend
                .get_embedding(path, "agent1", 768)
                .await
                .unwrap()
                .is_none(),
            "embedding must be cleared with the index entry"
        );
        let hits = backend
            .vector_search(&embedding, 768, "agent1", 5)
            .await
            .unwrap();
        assert!(
            hits.iter().all(|(p, _)| p != path),
            "deleted note must not surface in KNN: {hits:?}"
        );
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
    async fn alias_resolves_wikilink_to_aliased_note() {
        let backend = make_backend();

        // Target note titled "Bob Smith" carrying frontmatter alias "Bob".
        let mut target = make_note("Bob Smith", "people");
        target.aliases = vec!["Bob".to_string()];
        // Source links to the bare name [[Bob]] (no slash, not a filename).
        let mut source = make_note("source", "people");
        source.links = vec!["Bob".to_string()];

        // Target indexed first so the alias is present at resolution time.
        backend
            .index_note(&target, "agent1", "people")
            .await
            .unwrap();
        backend
            .index_note(&source, "agent1", "people")
            .await
            .unwrap();

        let edges = backend
            .get_outgoing_links("people/source", "agent1")
            .await
            .unwrap();
        assert_eq!(edges.len(), 1);
        // [[Bob]] resolved through the alias to the "Bob Smith" note path.
        assert_eq!(edges[0], "people/Bob Smith");
    }

    #[tokio::test]
    async fn relink_unresolved_dedupes_colliding_dangling_variants() {
        // Regression: two dangling wikilink variants on the SAME from_note
        // that both resolve to the SAME target (tier-2 exact filename +
        // tier-4 normalized matching) used to violate
        // UNIQUE(agent_id, from_note, to_note) on the second UPDATE, with no
        // transaction wrapping the loop — aborting the whole relink pass and
        // leaving every later dangling row (even on unrelated notes)
        // unprocessed. `UPDATE OR IGNORE` + cleanup DELETE must dedupe the
        // losing variant instead of erroring out mid-pass.
        let backend = make_backend();
        const AGENT: &str = "agent1";

        // Source note links two raw variants of the same not-yet-existing
        // target -> both dangle as distinct to_note rows (raw text).
        let mut source = make_note("source", "wiki");
        source.links = vec!["Rust Guide".to_string(), "rust guide".to_string()];
        backend.index_note(&source, AGENT, "wiki").await.unwrap();

        // A second, independent dangling link on a different from_note.
        let mut other = make_note("other", "wiki");
        other.links = vec!["Other Target".to_string()];
        backend.index_note(&other, AGENT, "wiki").await.unwrap();

        {
            let conn = backend.conn().lock().unwrap();
            let dangling: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM notes_links \
                     WHERE agent_id = ?1 AND status = 'dangling'",
                    rusqlite::params![AGENT],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(dangling, 3, "precondition: all three links start dangling");
        }

        // Now create the targets so relink_unresolved can resolve them.
        backend
            .index_note(&make_note("Rust Guide", "wiki"), AGENT, "wiki")
            .await
            .unwrap();
        backend
            .index_note(&make_note("Other Target", "wiki"), AGENT, "wiki")
            .await
            .unwrap();

        // Must not abort even though two dangling rows collide on the same
        // resolved target.
        let relinked = backend
            .relink_unresolved(AGENT)
            .await
            .expect("relink_unresolved must not error on a UNIQUE collision");
        assert_eq!(
            relinked, 2,
            "one deduped edge for the colliding pair + one independent edge"
        );

        let conn = backend.conn().lock().unwrap();

        // Exactly one active edge remains for the deduped pair.
        let active_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes_links \
                 WHERE agent_id = ?1 AND from_note = 'wiki/source' \
                 AND to_note = 'wiki/Rust Guide' AND status = 'active'",
                rusqlite::params![AGENT],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            active_count, 1,
            "deduped pair must collapse to exactly one active edge"
        );

        // No dangling duplicate remains from the losing UPDATE OR IGNORE.
        let dangling_remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes_links \
                 WHERE agent_id = ?1 AND from_note = 'wiki/source' AND status = 'dangling'",
                rusqlite::params![AGENT],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling_remaining, 0, "no dangling duplicate should remain");

        // The independent dangling link also got relinked, proving the pass
        // did not abort partway through.
        let independent_active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes_links \
                 WHERE agent_id = ?1 AND from_note = 'wiki/other' \
                 AND to_note = 'wiki/Other Target' AND status = 'active'",
                rusqlite::params![AGENT],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            independent_active, 1,
            "independent dangling link must also be relinked (no mid-pass abort)"
        );
    }

    #[tokio::test]
    async fn backfill_inbound_links_dedupes_unique_collision_without_panic() {
        // Regression: reviving a dangling/tombstone row's `to_note` can collide
        // with another row already occupying the same UNIQUE(agent_id,
        // from_note, to_note) key on the SAME from_note — e.g. the source note
        // was manually re-linked to the target through a different raw
        // wikilink text while the original link was still dangling. Mirrors
        // `relink_unresolved_dedupes_colliding_dangling_variants`'s defense:
        // `UPDATE OR IGNORE` + cleanup DELETE must not error out mid-pass.
        let backend = make_backend();
        const AGENT: &str = "agent1";

        // Source links a not-yet-existing target by raw text "Target Note" ->
        // dangles (to_note falls back to the raw text).
        let mut source = make_note("source", "wiki");
        source.links = vec!["Target Note".to_string()];
        backend.index_note(&source, AGENT, "wiki").await.unwrap();

        // Manually re-link the same source note to the target's eventual full
        // path via a different raw form — an independent ACTIVE row.
        backend
            .add_link_with_relation(AGENT, "wiki/source", "wiki/Target Note", "related")
            .await
            .unwrap();

        // Now the target is created — the dangling row's raw text "Target
        // Note" resolves to the SAME to_note ("wiki/Target Note") as the
        // active row above.
        backend
            .index_note(&make_note("Target Note", "wiki"), AGENT, "wiki")
            .await
            .unwrap();

        let revived = backend
            .backfill_inbound_links(AGENT, &["Target Note".to_string()])
            .await
            .expect("backfill must not error on a UNIQUE collision");
        assert_eq!(
            revived, 0,
            "the pre-existing active row wins; OR IGNORE skips the revival"
        );

        // Exactly one row remains for the (from, to) pair — no duplicate, no
        // leftover dangling remnant.
        let conn = backend.conn().lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes_links \
                 WHERE agent_id = ?1 AND from_note = 'wiki/source' AND to_note = 'wiki/Target Note'",
                rusqlite::params![AGENT],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "no duplicate/dangling remnant after the collision"
        );
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

    /// A relation stamped by a dream stage must SURVIVE a re-index of the source
    /// note. `relation` is DB-only enrichment — `NoteWeave` writes 'semantic' /
    /// 'related' / '<keyword>' via `add_link_with_relation` and never puts it in
    /// frontmatter — but `index_note` rebuilds its desired edge set from markdown,
    /// where a body wikilink carries no relation. Writing that `None` back through
    /// NULLed the label on the next re-index for any reason (NoteDecay's per-cycle
    /// frontmatter patch, an ingest append, a panel edit, or even NoteWeave's own
    /// next write in the same cycle), and nothing ever re-stamped it.
    #[tokio::test]
    async fn reindexing_a_note_preserves_relations_stamped_by_the_weave() {
        let backend = make_backend();
        const AGENT: &str = "default";

        // Two notes, with a body wikilink a -> b (the shape NoteWeave leaves behind:
        // `append_to_note` writes the link, then `add_link_with_relation` labels it).
        let mut a = make_note("a", "cat");
        a.links = vec!["cat/b".to_string()];
        backend.index_note(&a, AGENT, "cat").await.unwrap();
        backend
            .index_note(&make_note("b", "cat"), AGENT, "cat")
            .await
            .unwrap();

        backend
            .add_link_with_relation(AGENT, "cat/a", "cat/b", "semantic")
            .await
            .unwrap();

        // Re-index `a` for an unrelated reason (e.g. NoteDecay patching frontmatter).
        // The markdown is unchanged and still says nothing about the relation.
        let mut a_again = a.clone();
        a_again.updated_at = 2000;
        backend.index_note(&a_again, AGENT, "cat").await.unwrap();

        let rels = backend.get_typed_relations("cat/a", AGENT).await.unwrap();
        assert!(
            rels.iter()
                .any(|(to, rel)| to == "cat/b" && rel == "semantic"),
            "the 'semantic' relation must survive a re-index of the source note \
             (markdown owns the edge, the weave owns the label); got: {rels:?}"
        );
    }

    /// ...but an explicit frontmatter `relations:` entry still wins over whatever
    /// is in the DB — markdown remains authoritative when it actually says something.
    #[tokio::test]
    async fn frontmatter_relation_overrides_a_previously_stamped_one() {
        use crate::memory::notes::Relation;
        let backend = make_backend();
        const AGENT: &str = "default";

        let mut a = make_note("a", "cat");
        a.links = vec!["cat/b".to_string()];
        backend.index_note(&a, AGENT, "cat").await.unwrap();
        backend
            .index_note(&make_note("b", "cat"), AGENT, "cat")
            .await
            .unwrap();
        backend
            .add_link_with_relation(AGENT, "cat/a", "cat/b", "semantic")
            .await
            .unwrap();

        // Now the note declares the relation explicitly in frontmatter.
        let mut a_typed = a.clone();
        a_typed.relations = vec![Relation {
            to: "cat/b".to_string(),
            rel_type: "contradicts".to_string(),
            confidence: 0.9,
        }];
        backend.index_note(&a_typed, AGENT, "cat").await.unwrap();

        let rels = backend.get_typed_relations("cat/a", AGENT).await.unwrap();
        assert!(
            rels.iter()
                .any(|(to, rel)| to == "cat/b" && rel == "contradicts"),
            "an explicit frontmatter relation must override the stamped one; got: {rels:?}"
        );
    }

    #[tokio::test]
    async fn community_peers_returns_same_community_excluding_self() {
        const AGENT: &str = "agent1";
        let backend = make_backend();

        // Materialize a cache: a + b share community 0, c is in community 1.
        // Rows are (node_path, community_id, cohesion, degree).
        backend
            .replace_graph_cache(
                AGENT,
                &[
                    ("cat/a".to_string(), 0, 0.5, 2),
                    ("cat/b".to_string(), 0, 0.5, 2),
                    ("cat/c".to_string(), 1, 0.3, 1),
                ],
            )
            .await
            .unwrap();

        // a's peers = same community (0) minus a itself → [b], never c.
        let peers = backend.community_peers(AGENT, "cat/a", 8).await.unwrap();
        assert_eq!(peers, vec!["cat/b".to_string()]);
        assert!(!peers.contains(&"cat/a".to_string()), "self excluded");
        assert!(
            !peers.contains(&"cat/c".to_string()),
            "other community excluded"
        );

        // A path with no cache row (cold/absent) → empty, graceful fallback.
        let none = backend
            .community_peers(AGENT, "cat/missing", 8)
            .await
            .unwrap();
        assert!(none.is_empty());

        // A different agent has no cache rows → empty (scoping holds).
        let other = backend.community_peers("agent2", "cat/a", 8).await.unwrap();
        assert!(other.is_empty());
    }

    #[tokio::test]
    async fn related_peers_returns_scored_rows_descending() {
        const AGENT: &str = "agent1";
        let backend = make_backend();

        // Materialize 5-signal related edges for cat/a.
        // Rows are (node_path, related_path, score).
        backend
            .replace_graph_related(
                AGENT,
                &[
                    ("cat/a".to_string(), "cat/b".to_string(), 1.5),
                    ("cat/a".to_string(), "cat/c".to_string(), 9.0),
                    ("cat/a".to_string(), "cat/d".to_string(), 4.0),
                ],
            )
            .await
            .unwrap();

        // a's related peers, ordered by score descending.
        let peers = backend.related_peers(AGENT, "cat/a", 8).await.unwrap();
        assert_eq!(
            peers,
            vec![
                ("cat/c".to_string(), 9.0),
                ("cat/d".to_string(), 4.0),
                ("cat/b".to_string(), 1.5),
            ]
        );

        // limit truncates after the highest scores.
        let top = backend.related_peers(AGENT, "cat/a", 2).await.unwrap();
        assert_eq!(
            top,
            vec![("cat/c".to_string(), 9.0), ("cat/d".to_string(), 4.0)]
        );

        // A node with no related rows (cold/absent) → empty, graceful fallback.
        let none = backend
            .related_peers(AGENT, "cat/missing", 8)
            .await
            .unwrap();
        assert!(none.is_empty());

        // A different agent has no related rows → empty (scoping holds).
        let other = backend.related_peers("agent2", "cat/a", 8).await.unwrap();
        assert!(other.is_empty());
    }

    #[tokio::test]
    async fn related_edges_between_filters_visible_and_caps_per_node() {
        use std::collections::HashSet;

        const AGENT: &str = "agent1";
        let backend = make_backend();

        // cat/a has 3 related rows (b, c, d); only b and c are "visible".
        // cat/x has 1 related row to an invisible node → must be dropped entirely.
        backend
            .replace_graph_related(
                AGENT,
                &[
                    ("cat/a".to_string(), "cat/b".to_string(), 1.5),
                    ("cat/a".to_string(), "cat/c".to_string(), 9.0),
                    ("cat/a".to_string(), "cat/d".to_string(), 4.0),
                    ("cat/x".to_string(), "cat/invisible".to_string(), 5.0),
                ],
            )
            .await
            .unwrap();

        let visible: HashSet<String> = ["cat/a", "cat/b", "cat/c", "cat/x"]
            .into_iter()
            .map(String::from)
            .collect();

        // per_node=1 keeps only the top-scored visible row for cat/a (cat/c);
        // cat/d is excluded because it is not in `visible`, regardless of the cap.
        let rows = backend
            .related_edges_between(AGENT, &visible, 1)
            .await
            .unwrap();
        assert_eq!(
            rows,
            vec![("cat/a".to_string(), "cat/c".to_string(), 9.0)],
            "only the top-1 visible-both-ends row for cat/a must survive: {rows:?}"
        );

        // cat/x's only related row points at an invisible node → dropped even
        // though cat/x itself is visible (both ends must be visible).
        assert!(
            !rows.iter().any(|(n, _, _)| n == "cat/x"),
            "row with an invisible endpoint must not surface: {rows:?}"
        );

        // per_node=2 lets cat/b back in (score-DESC: c then b; d stays excluded
        // because it is not in `visible`).
        let rows2 = backend
            .related_edges_between(AGENT, &visible, 2)
            .await
            .unwrap();
        assert_eq!(
            rows2,
            vec![
                ("cat/a".to_string(), "cat/c".to_string(), 9.0),
                ("cat/a".to_string(), "cat/b".to_string(), 1.5),
            ]
        );

        // A different agent has no related rows → empty (scoping holds).
        let other = backend
            .related_edges_between("agent2", &visible, 8)
            .await
            .unwrap();
        assert!(other.is_empty());
    }

    #[tokio::test]
    async fn get_notes_with_content_returns_known_paths_skips_unknown() {
        let backend = make_backend();

        // index_note stores path = "{category}/{title}" (extensionless,
        // verified in sqlite/notes.rs index_note). make_note uses tags/facts/
        // content_hash with ..Default::default().
        backend
            .index_note(&make_note("alpha", "general"), "default", "general")
            .await
            .unwrap();
        backend
            .index_note(&make_note("beta", "general"), "default", "general")
            .await
            .unwrap();

        let query = vec![
            "general/alpha".to_string(),
            "general/beta".to_string(),
            "general/does-not-exist".to_string(),
        ];
        let got = backend
            .get_notes_with_content("default", &query)
            .await
            .unwrap();
        let got_paths: std::collections::HashSet<&str> =
            got.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(got.len(), 2, "unknown path must be skipped");
        assert!(got_paths.contains("general/alpha"));
        assert!(got_paths.contains("general/beta"));

        // Empty input -> empty output (no IN () footgun).
        let empty = backend
            .get_notes_with_content("default", &[])
            .await
            .unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn sources_of_and_notes_citing_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let backend = SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap();
        let note = KnowledgeNote {
            title: "typescript".into(),
            category: "preference".into(),
            facts: vec!["prefers ts".into()],
            source_notes: vec!["raw-1".into(), "raw-2".into()],
            ..Default::default()
        };
        backend
            .index_note(&note, "default", "preference")
            .await
            .unwrap();

        let mut srcs = backend
            .sources_of("default", "preference/typescript")
            .await
            .unwrap();
        srcs.sort();
        assert_eq!(srcs, vec!["raw-1".to_string(), "raw-2".to_string()]);

        let citing = backend.notes_citing("default", "raw-1").await.unwrap();
        assert_eq!(citing, vec!["preference/typescript".to_string()]);
    }

    #[tokio::test]
    async fn get_graph_data_surfaces_edge_relation_kind() {
        let backend = make_backend();
        backend
            .index_note(&make_note("a", "reference"), "agent1", "reference")
            .await
            .unwrap();
        backend
            .index_note(&make_note("b", "reference"), "agent1", "reference")
            .await
            .unwrap();
        // Discover the actual stored paths (path convention is internal).
        let (nodes, _e) = backend.get_graph_data("agent1", 100).await.unwrap();
        assert!(nodes.len() >= 2, "expected both seeded notes");
        let a = nodes[0].path.clone();
        let b = nodes[1].path.clone();
        backend
            .add_link_with_relation("agent1", &a, &b, "semantic")
            .await
            .unwrap();

        let (_n, edges) = backend.get_graph_data("agent1", 100).await.unwrap();
        let e = edges
            .iter()
            .find(|edge| edge.from == a && edge.to == b)
            .expect("edge a->b present");
        assert_eq!(e.relation.as_deref(), Some("semantic"));
    }

    #[tokio::test]
    async fn count_notes_is_agent_scoped() {
        let backend = make_backend();
        backend
            .index_note(&make_note("a", "reference"), "agent1", "reference")
            .await
            .unwrap();
        backend
            .index_note(&make_note("b", "reference"), "agent1", "reference")
            .await
            .unwrap();
        assert_eq!(backend.count_notes("agent1").await.unwrap(), 2);
        assert_eq!(backend.count_notes("other-agent").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn community_ids_reads_graph_cache() {
        let backend = make_backend();
        backend
            .replace_graph_cache("agent1", &[("notes/a".to_string(), 7usize, 0.5f32, 3usize)])
            .await
            .unwrap();
        let map = backend.community_ids("agent1").await.unwrap();
        assert_eq!(map.get("notes/a").copied(), Some(7));
        assert!(backend
            .community_ids("other-agent")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn increment_review_retry_counts_up_and_survives_reads() {
        let backend = make_backend();
        let id = backend
            .enqueue_review("agent1", "{}", "med", 0.4, "low confidence")
            .await
            .unwrap();

        // Fresh row starts at retry_count 0.
        let pending = backend
            .list_pending_review("agent1", i64::MAX)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].retry_count, 0);

        // Each increment returns the new count monotonically.
        assert_eq!(backend.increment_review_retry(&id).await.unwrap(), 1);
        assert_eq!(backend.increment_review_retry(&id).await.unwrap(), 2);
        assert_eq!(backend.increment_review_retry(&id).await.unwrap(), 3);

        // And it is persisted on the row (so the stage can enforce a ceiling).
        let pending = backend
            .list_pending_review("agent1", i64::MAX)
            .await
            .unwrap();
        assert_eq!(pending[0].retry_count, 3);
    }
}
