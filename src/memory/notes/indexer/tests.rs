use super::*;
use crate::memory::store::SqliteMemoryBackend;
use tempfile::TempDir;
use uuid::Uuid;

const AGENT: &str = "default";

fn create_test_db() -> Arc<SqliteMemoryBackend> {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("test_indexer_{}", Uuid::new_v4()));
    Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
}

fn sample_md(category: &str, facts: &[&str], links: &[&str]) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("category: {category}\n"));
    out.push_str("tags: [test]\n");
    out.push_str("created: 2026-04-01\n");
    out.push_str("updated: 2026-04-10\n");
    out.push_str("---\n\n");
    for fact in facts {
        out.push_str(&format!("- {fact}\n"));
    }
    if !links.is_empty() {
        out.push('\n');
        let link_strs: Vec<String> = links.iter().map(|l| format!("[[{l}]]")).collect();
        out.push_str(&format!("Related: {}\n", link_strs.join(" ")));
    }
    out
}

/// Create memory_dir/{agent_id}/{category}/ directory structure.
async fn setup_category_dir(memory_dir: &Path, agent_id: &str, category: &str) -> PathBuf {
    let cat_dir = memory_dir.join(agent_id).join(category);
    fs::create_dir_all(&cat_dir).await.unwrap();
    cat_dir
}

#[tokio::test]
async fn ensure_dirs_creates_all_categories() {
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    let indexer = NoteIndexer::new(memory_dir.clone(), db);

    indexer.ensure_dirs(AGENT).await.unwrap();

    for cat in CATEGORY_DIRS {
        assert!(
            memory_dir.join(AGENT).join(cat).is_dir(),
            "Missing dir: {cat}"
        );
    }
}

#[tokio::test]
async fn full_rebuild_indexes_all_notes() {
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    // Write files into category subdirs
    let pref_dir = setup_category_dir(&memory_dir, AGENT, "preference").await;
    let skill_dir = setup_category_dir(&memory_dir, AGENT, "skill").await;

    let note1 = sample_md("preference", &["User likes Vim"], &["Dev Environment"]);
    let note2 = sample_md("skill", &["User knows Rust"], &["Editor Preferences"]);

    fs::write(pref_dir.join("Editor Preferences.md"), &note1)
        .await
        .unwrap();
    fs::write(skill_dir.join("Rust Learning.md"), &note2)
        .await
        .unwrap();

    let indexer = NoteIndexer::new(memory_dir, db.clone());

    let stats = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats.indexed, 2);
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.skipped, 0);

    // Verify indexed
    let notes = db.list_notes(AGENT).await.unwrap();
    assert_eq!(notes.len(), 2);

    // Verify wikilinks are indexed
    let out_links = db
        .get_outgoing_links("preference/Editor Preferences", AGENT)
        .await
        .unwrap();
    assert!(out_links.contains(&"Dev Environment".to_string()));

    let out_links2 = db
        .get_outgoing_links("skill/Rust Learning", AGENT)
        .await
        .unwrap();
    assert!(out_links2.contains(&"preference/Editor Preferences".to_string()));
}

#[tokio::test]
async fn full_rebuild_prunes_orphan_index_rows() {
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let pref_dir = setup_category_dir(&memory_dir, AGENT, "preference").await;
    fs::write(
        pref_dir.join("Keep.md"),
        sample_md("preference", &["keep me"], &[]),
    )
    .await
    .unwrap();
    fs::write(
        pref_dir.join("Gone.md"),
        sample_md("preference", &["delete me"], &[]),
    )
    .await
    .unwrap();

    let indexer = NoteIndexer::new(memory_dir, db.clone());

    // First rebuild indexes both; nothing to prune yet.
    let stats = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats.indexed, 2);
    assert_eq!(stats.pruned, 0);
    assert_eq!(db.list_notes(AGENT).await.unwrap().len(), 2);

    // Remove one file from disk → its index row is now an orphan.
    fs::remove_file(pref_dir.join("Gone.md")).await.unwrap();

    // Second rebuild prunes exactly the orphan; the surviving row is kept.
    let stats = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats.pruned, 1, "the file-less row must be pruned");
    let notes = db.list_notes(AGENT).await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].path, "preference/Keep");
}

#[tokio::test]
async fn full_rebuild_skips_unchanged() {
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let misc_dir = setup_category_dir(&memory_dir, AGENT, "other").await;
    let note1 = sample_md("other", &["fact one"], &[]);
    fs::write(misc_dir.join("Note1.md"), &note1).await.unwrap();

    let indexer = NoteIndexer::new(memory_dir, db.clone());

    // First rebuild
    let stats1 = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats1.indexed, 1);

    // Second rebuild — same content → skip
    let stats2 = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats2.skipped, 1);
    assert_eq!(stats2.indexed, 0);
}

#[tokio::test]
async fn index_file_detects_change() {
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let misc_dir = setup_category_dir(&memory_dir, AGENT, "other").await;
    let path = misc_dir.join("Dynamic.md");
    fs::write(&path, sample_md("other", &["v1"], &[]))
        .await
        .unwrap();

    let indexer = NoteIndexer::new(memory_dir, db.clone());

    // First index
    assert!(indexer.index_file(AGENT, "other", &path).await.unwrap());
    // Same content → skip
    assert!(!indexer.index_file(AGENT, "other", &path).await.unwrap());

    // Change content
    fs::write(&path, sample_md("other", &["v2"], &[]))
        .await
        .unwrap();
    // Changed → re-index
    assert!(indexer.index_file(AGENT, "other", &path).await.unwrap());
}

#[tokio::test]
async fn write_note_creates_file() {
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let indexer = NoteIndexer::new(memory_dir.clone(), db);

    let note = KnowledgeNote {
        title: "Test Note".to_string(),
        category: "other".to_string(),
        tags: vec!["a".to_string()],
        facts: vec!["hello".to_string()],
        links: vec![],
        created_at: 1_700_000_000,
        updated_at: 1_700_000_000,
        content_hash: String::new(),
        ..Default::default()
    };

    let path = indexer.write_note(AGENT, "other", &note).await.unwrap();
    assert!(path.exists());
    assert!(path.starts_with(memory_dir.join(AGENT).join("other")));

    let content = fs::read_to_string(&path).await.unwrap();
    assert!(content.contains("category: other"));
    assert!(content.contains("- hello"));
}

#[tokio::test]
async fn append_to_existing_note() {
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let pref_dir = setup_category_dir(&memory_dir, AGENT, "preference").await;
    let initial = sample_md("preference", &["fact1"], &["Link1"]);
    fs::write(pref_dir.join("Target.md"), &initial)
        .await
        .unwrap();

    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

    indexer
        .append_to_note(
            AGENT,
            "preference/Target",
            &["fact2".to_string()],
            &["Link1".to_string(), "Link2".to_string()],
        )
        .await
        .unwrap();

    // Read back the file
    let content = fs::read_to_string(pref_dir.join("Target.md"))
        .await
        .unwrap();
    assert!(content.contains("- fact1"));
    assert!(content.contains("- fact2"));
    assert!(content.contains("[[Link1]]"));
    assert!(content.contains("[[Link2]]"));

    // Verify indexed
    let entry = db
        .get_note_index("preference/Target", AGENT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.link_count, 2); // Link1 deduped + Link2
}

#[tokio::test]
async fn append_creates_new_note() {
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

    indexer
        .append_to_note(AGENT, "other/Brand New", &["a fact".to_string()], &[])
        .await
        .unwrap();

    assert!(memory_dir
        .join(AGENT)
        .join("other")
        .join("Brand New.md")
        .exists());

    let entry = db
        .get_note_index("other/Brand New", AGENT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.category, "other");
}

#[tokio::test]
async fn full_rebuild_parallel_matches_serial_results() {
    // Phase B B3 parity contract: parallel full_rebuild must produce the
    // same (indexed + skipped) total and the same set of indexed rows as
    // the prior serial implementation.
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());
    indexer.ensure_dirs(AGENT).await.unwrap();

    // 50 notes spread across 5 categories.
    for cat in &["preference", "skill", "reference", "plan", "learning"] {
        for i in 0..10 {
            let note = KnowledgeNote {
                title: format!("n-{cat}-{i}"),
                category: (*cat).into(),
                facts: vec![format!("fact {i}")],
                content_hash: String::new(),
                ..Default::default()
            };
            indexer.write_note(AGENT, cat, &note).await.unwrap();
        }
    }

    let stats = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats.indexed + stats.skipped, 50);
    assert_eq!(stats.errors, 0);

    let listed = db.list_notes(AGENT).await.unwrap();
    assert_eq!(listed.len(), 50);
}

#[tokio::test]
async fn rename_note_cascades_wikilinks() {
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    // Create two notes in the same category
    let misc_dir = setup_category_dir(&memory_dir, AGENT, "other").await;
    let note_a = sample_md("other", &["fact A"], &["Old Name"]);
    let note_b = sample_md("other", &["fact B"], &[]);
    fs::write(misc_dir.join("Linker.md"), &note_a)
        .await
        .unwrap();
    fs::write(misc_dir.join("Old Name.md"), &note_b)
        .await
        .unwrap();

    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

    // Initial index
    indexer.full_rebuild(AGENT).await.unwrap();

    // Rename "Old Name" → "New Name"
    indexer
        .rename_note(AGENT, "Old Name", "New Name")
        .await
        .unwrap();

    // Old file gone, new file exists
    assert!(!misc_dir.join("Old Name.md").exists());
    assert!(misc_dir.join("New Name.md").exists());

    // Linker.md should now reference [[New Name]]
    let linker_content = fs::read_to_string(misc_dir.join("Linker.md"))
        .await
        .unwrap();
    assert!(linker_content.contains("[[New Name]]"));
    assert!(!linker_content.contains("[[Old Name]]"));

    // Old index entry removed, new one present
    let old_paths = db.find_by_filename("Old Name", AGENT).await.unwrap();
    assert!(old_paths.is_empty());
    let new_paths = db.find_by_filename("New Name", AGENT).await.unwrap();
    assert!(!new_paths.is_empty());

    // Linker's outgoing links updated
    let out = db.get_outgoing_links("other/Linker", AGENT).await.unwrap();
    assert!(out.contains(&"New Name".to_string()));
    assert!(!out.contains(&"Old Name".to_string()));
}

#[cfg(test)]
mod reference_hook_tests {
    use super::*;
    use crate::memory::notes::note::KnowledgeNote;
    use crate::memory::notes::orientation::types::{
        IndexStats, LogEntry, OrientationSnapshot, TokenBudget,
    };
    use crate::memory::notes::orientation::NoteOrientation;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    struct CountingOrient {
        calls: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl NoteOrientation for CountingOrient {
        async fn bootstrap(&self, _a: &str) -> Result<(), AlephError> {
            Ok(())
        }
        async fn read_snapshot(
            &self,
            _a: &str,
            _b: TokenBudget,
        ) -> Result<OrientationSnapshot, AlephError> {
            Ok(OrientationSnapshot {
                schema_text: String::new(),
                index_text: String::new(),
                recent_log_tail: String::new(),
            })
        }
        async fn record_ingest(&self, _a: &str, _e: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_query(&self, _a: &str, _e: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_lint(&self, _a: &str, _e: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_session_end(&self, _a: &str, _e: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn rebuild_index(&self, _a: &str) -> Result<IndexStats, AlephError> {
            Ok(IndexStats::default())
        }
        async fn rotate_log_if_needed(&self, _a: &str) -> Result<bool, AlephError> {
            Ok(false)
        }
        fn invalidate(&self, agent_id: &str, note_path: &str) {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((agent_id.to_string(), note_path.to_string()));
        }
    }

    #[tokio::test]
    async fn write_note_invalidates_orientation() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let orient = Arc::new(CountingOrient {
            calls: Mutex::new(vec![]),
        });
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone())
            .with_orientation(orient.clone() as Arc<dyn NoteOrientation>);

        let note = KnowledgeNote {
            title: "rust".into(),
            category: "learning".into(),
            tags: vec![],
            facts: vec!["f1".into()],
            links: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
            ..Default::default()
        };
        indexer
            .write_note("default", "learning", &note)
            .await
            .unwrap();

        let calls = orient
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "default");
        assert_eq!(calls[0].1, "learning/rust");
    }

    #[tokio::test]
    async fn write_note_also_indexes_to_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        let note = KnowledgeNote {
            title: "rust-async".into(),
            category: "learning".into(),
            tags: vec!["rust".into()],
            facts: vec!["Tokio is the async runtime".into()],
            links: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
            ..Default::default()
        };
        indexer
            .write_note("default", "learning", &note)
            .await
            .unwrap();

        // Without the fix, list_notes returns [] until full_rebuild runs.
        let listed = backend.list_notes("default").await.unwrap();
        assert_eq!(listed.len(), 1, "write_note must also index to SQLite");
        assert_eq!(listed[0].path, "learning/rust-async");
    }

    #[tokio::test]
    async fn apply_distill_action_strengthen_appends_source_facts() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        // Seed an existing skill note with one source_fact
        let seed = KnowledgeNote {
            title: "async-error-handling".into(),
            category: "skill".into(),
            tags: vec![],
            facts: vec!["always propagate errors with ?".into()],
            links: vec![],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            content_hash: String::new(),
            source_notes: vec!["fact-original".into()],
            ..Default::default()
        };
        indexer.write_note("default", "skill", &seed).await.unwrap();

        // Strengthen with a new source_fact
        let action = DistillAction::Strengthen {
            existing_note_path: "skill/async-error-handling".into(),
            source_facts: vec!["fact-new".into(), "fact-original".into()], // includes a dup
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        // Re-read from disk and verify source_notes merged + de-duplicated
        let file = dir
            .path()
            .join("note")
            .join("default")
            .join("skill")
            .join("async-error-handling.md");
        let content = fs::read_to_string(&file).await.unwrap();
        let reparsed = KnowledgeNote::from_markdown("async-error-handling", &content).unwrap();
        assert_eq!(reparsed.source_notes.len(), 2, "should have 2 unique facts");
        assert!(reparsed.source_notes.contains(&"fact-original".to_string()));
        assert!(reparsed.source_notes.contains(&"fact-new".to_string()));
        // updated_at must be bumped above the seeded value
        assert!(reparsed.updated_at >= seed.updated_at);
    }

    #[tokio::test]
    async fn append_to_note_also_indexes_to_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        indexer
            .append_to_note("default", "learning/rust-async", &["new fact".into()], &[])
            .await
            .unwrap();
        let listed = backend.list_notes("default").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].path == "learning/rust-async");
    }

    // -----------------------------------------------------------------
    // H1 — Strengthen lifts confidence monotonically with new facts.
    // -----------------------------------------------------------------

    /// Read a note's current confidence by re-parsing its markdown from disk.
    async fn read_confidence(dir: &Path, agent: &str, path: &str) -> f32 {
        let (cat, name) = path.split_once('/').unwrap();
        let file = dir
            .join("note")
            .join(agent)
            .join(cat)
            .join(format!("{name}.md"));
        let content = fs::read_to_string(&file).await.unwrap();
        KnowledgeNote::from_markdown(name, &content)
            .unwrap()
            .confidence
    }

    /// Seed a skill note with the given confidence and source_notes.
    async fn seed_note<S: NoteStore>(
        indexer: &NoteIndexer<S>,
        agent: &str,
        category: &str,
        title: &str,
        confidence: f32,
        source_notes: Vec<String>,
    ) {
        let note = KnowledgeNote {
            title: title.into(),
            category: category.into(),
            facts: vec!["body fact".into()],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            confidence,
            source_notes,
            ..Default::default()
        };
        indexer.write_note(agent, category, &note).await.unwrap();
    }

    #[tokio::test]
    async fn strengthen_with_new_facts_bumps_confidence_monotonically() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        seed_note(&indexer, "default", "skill", "topic", 0.4, vec![]).await;

        let action = DistillAction::Strengthen {
            existing_note_path: "skill/topic".into(),
            source_facts: (0..5).map(|i| format!("fact-{i}")).collect(),
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        let after = read_confidence(dir.path(), "default", "skill/topic").await;
        // 0.4 + 5 * 0.05 = 0.65 — within float tolerance.
        assert!(
            (0.6499..=0.6501).contains(&after),
            "expected confidence ~0.65, got {after}"
        );
    }

    #[tokio::test]
    async fn strengthen_with_zero_new_facts_leaves_confidence_unchanged() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        seed_note(
            &indexer,
            "default",
            "skill",
            "stable",
            0.42,
            vec!["fact-a".into(), "fact-b".into()],
        )
        .await;

        // Re-submit the SAME source_facts — none should be new.
        let action = DistillAction::Strengthen {
            existing_note_path: "skill/stable".into(),
            source_facts: vec!["fact-a".into(), "fact-b".into()],
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        let after = read_confidence(dir.path(), "default", "skill/stable").await;
        assert!(
            (after - 0.42).abs() < 1e-5,
            "confidence must stay at 0.42 when no new facts arrive, got {after}"
        );
    }

    #[tokio::test]
    async fn strengthen_saturates_at_one() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        seed_note(&indexer, "default", "skill", "almost", 0.95, vec![]).await;

        // 10 new facts × 0.05 = 0.50 → would push to 1.45 without the clamp.
        let action = DistillAction::Strengthen {
            existing_note_path: "skill/almost".into(),
            source_facts: (0..10).map(|i| format!("f{i}")).collect(),
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        let after = read_confidence(dir.path(), "default", "skill/almost").await;
        assert!(
            (after - 1.0).abs() < 1e-5,
            "confidence must saturate at 1.0, got {after}"
        );
    }

    // -----------------------------------------------------------------
    // H2 — DistillAction::New collision-guards by demoting to Strengthen.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn apply_new_with_filename_collision_strengthens_existing() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        // First New: confidence 0.4, one source fact.
        let first = DistillAction::New {
            title: "duplicate-topic".into(),
            rule: "first body".into(),
            confidence: 0.4,
            severity: Default::default(),
            source_facts: vec!["seed-fact".into()],
        };
        indexer
            .apply_distill_action("default", "skill", &first)
            .await
            .unwrap();

        // Second New with the SAME safe_title — must not silently overwrite.
        let second = DistillAction::New {
            title: "duplicate-topic".into(),
            rule: "second body".into(), // would replace the body if we naively wrote
            confidence: 0.9,            // floor that the existing note must be lifted to
            severity: Default::default(),
            source_facts: vec!["seed-fact".into(), "extra-fact".into()],
        };
        indexer
            .apply_distill_action("default", "skill", &second)
            .await
            .unwrap();

        // Re-read the (single) note from disk.
        let file = dir
            .path()
            .join("note")
            .join("default")
            .join("skill")
            .join("duplicate-topic.md");
        let content = fs::read_to_string(&file).await.unwrap();
        let merged = KnowledgeNote::from_markdown("duplicate-topic", &content).unwrap();

        // Source notes are merged + deduped (seed-fact stays, extra-fact added).
        assert_eq!(merged.source_notes.len(), 2);
        assert!(merged.source_notes.contains(&"seed-fact".to_string()));
        assert!(merged.source_notes.contains(&"extra-fact".to_string()));

        // Confidence is lifted to ≥ the new action's confidence (0.9), then
        // bumped by STRENGTHEN_STEP for each new fact (1 new = +0.05).
        // Expected: 0.9 + 0.05 = 0.95.
        assert!(
            merged.confidence >= 0.949 && merged.confidence <= 0.951,
            "confidence must be lifted to ~0.95, got {}",
            merged.confidence
        );

        // The original body must NOT have been replaced — collision demoted
        // to Strengthen, so the body content is preserved.
        assert!(
            content.contains("first body"),
            "first body must be preserved when collision is demoted to Strengthen"
        );
    }

    #[tokio::test]
    async fn apply_new_without_collision_writes_fresh() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        let action = DistillAction::New {
            title: "fresh-topic".into(),
            rule: "fresh body".into(),
            confidence: 0.7,
            severity: Default::default(),
            source_facts: vec!["fact-1".into()],
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        let after = read_confidence(dir.path(), "default", "skill/fresh-topic").await;
        // No collision → no bump applied; confidence == as written.
        assert!((after - 0.7).abs() < 1e-5, "got {after}");
    }
}
