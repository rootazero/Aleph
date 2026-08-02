use super::*;
use crate::memory::store::SqliteMemoryBackend;
use tempfile::TempDir;
use uuid::Uuid;

const AGENT: &str = "default";

fn create_test_db() -> Arc<SqliteMemoryBackend> {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("test_indexer_{}", Uuid::new_v4()));
    // rust-doctor-disable-next-line unwrap-in-production
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
    // rust-doctor-disable-next-line unwrap-in-production
    fs::create_dir_all(&cat_dir).await.unwrap();
    cat_dir
}

#[tokio::test]
async fn ensure_dirs_creates_all_categories() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db);

    // rust-doctor-disable-next-line unwrap-in-production
    indexer.ensure_dirs(AGENT).await.unwrap();

    for cat in CATEGORY_DIRS {
        assert!(
            memory_dir.join(AGENT).join(cat).is_dir(),
            "Missing dir: {cat}"
        );
    }
}

struct StubEmbedder;

#[async_trait::async_trait]
impl crate::memory::embedding_provider::EmbeddingProvider for StubEmbedder {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Ok(vec![0.25_f32; 768])
    }
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Ok(texts.iter().map(|_| vec![0.25_f32; 768]).collect())
    }
    fn dimensions(&self) -> usize {
        768
    }
    fn model_name(&self) -> &str {
        "stub"
    }
    fn provider_id(&self) -> &str {
        "stub"
    }
}

#[tokio::test]
async fn write_note_embeds_on_write_only_with_embedder() {
    // W1: with an embedder attached, write_note refreshes the note's vector so
    // it is immediately vector-searchable instead of waiting for reembed_all.
    // Without one, the write path stays FTS-only (byte-identical old behaviour).
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let db = create_test_db();
    let note = KnowledgeNote {
        title: "Vectorable".to_string(),
        category: "learning".to_string(),
        facts: vec!["some content worth embedding".to_string()],
        created_at: 1000,
        updated_at: 1000,
        ..Default::default()
    };

    // No embedder → no vector after the write.
    // rust-doctor-disable-next-line excessive-clone
    let plain = NoteIndexer::new(dir.path().to_path_buf(), db.clone());
    // rust-doctor-disable-next-line unwrap-in-production
    plain.write_note(AGENT, "learning", &note).await.unwrap();
    assert!(
        db.get_embedding("learning/Vectorable", AGENT, 768)
            .await
            .unwrap()
            .is_none(),
        "a plain indexer must not embed on write"
    );

    // With an embedder → vector present immediately after the write.
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(dir.path().to_path_buf(), db.clone())
        .with_embedder(Arc::new(StubEmbedder));
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.write_note(AGENT, "learning", &note).await.unwrap();
    assert_eq!(
        db.get_embedding("learning/Vectorable", AGENT, 768)
            .await
            .unwrap(),
        Some(vec![0.25_f32; 768]),
        "embed-on-write must upsert the note's vector"
    );
}

#[tokio::test]
async fn full_rebuild_indexes_all_notes() {
    // rust-doctor-disable-next-line unwrap-in-production
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
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    fs::write(skill_dir.join("Rust Learning.md"), &note2)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir, db.clone());

    // rust-doctor-disable-next-line unwrap-in-production
    let stats = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats.indexed, 2);
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.skipped, 0);

    // Verify indexed
    // rust-doctor-disable-next-line unwrap-in-production
    let notes = db.list_notes(AGENT).await.unwrap();
    assert_eq!(notes.len(), 2);

    // Verify wikilinks are indexed
    let out_links = db
        .get_outgoing_links("preference/Editor Preferences", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(out_links.contains(&"Dev Environment".to_string()));

    let out_links2 = db
        .get_outgoing_links("skill/Rust Learning", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(out_links2.contains(&"preference/Editor Preferences".to_string()));
}

#[tokio::test]
async fn full_rebuild_prunes_orphan_index_rows() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let pref_dir = setup_category_dir(&memory_dir, AGENT, "preference").await;
    fs::write(
        pref_dir.join("Keep.md"),
        sample_md("preference", &["keep me"], &[]),
    )
    .await
    // rust-doctor-disable-next-line unwrap-in-production
    .unwrap();
    fs::write(
        pref_dir.join("Gone.md"),
        sample_md("preference", &["delete me"], &[]),
    )
    .await
    // rust-doctor-disable-next-line unwrap-in-production
    .unwrap();

    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir, db.clone());

    // First rebuild indexes both; nothing to prune yet.
    // rust-doctor-disable-next-line unwrap-in-production
    let stats = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats.indexed, 2);
    assert_eq!(stats.pruned, 0);
    assert_eq!(db.list_notes(AGENT).await.unwrap().len(), 2);

    // Remove one file from disk → its index row is now an orphan.
    // rust-doctor-disable-next-line unwrap-in-production
    fs::remove_file(pref_dir.join("Gone.md")).await.unwrap();

    // Second rebuild prunes exactly the orphan; the surviving row is kept.
    // rust-doctor-disable-next-line unwrap-in-production
    let stats = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats.pruned, 1, "the file-less row must be pruned");
    // rust-doctor-disable-next-line unwrap-in-production
    let notes = db.list_notes(AGENT).await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].path, "preference/Keep");
}

#[tokio::test]
async fn full_rebuild_skips_unchanged() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let misc_dir = setup_category_dir(&memory_dir, AGENT, "other").await;
    let note1 = sample_md("other", &["fact one"], &[]);
    // rust-doctor-disable-next-line unwrap-in-production
    fs::write(misc_dir.join("Note1.md"), &note1).await.unwrap();

    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir, db.clone());

    // First rebuild
    // rust-doctor-disable-next-line unwrap-in-production
    let stats1 = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats1.indexed, 1);

    // Second rebuild — same content → skip
    // rust-doctor-disable-next-line unwrap-in-production
    let stats2 = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats2.skipped, 1);
    assert_eq!(stats2.indexed, 0);
}

#[tokio::test]
async fn index_file_detects_change() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let misc_dir = setup_category_dir(&memory_dir, AGENT, "other").await;
    let path = misc_dir.join("Dynamic.md");
    fs::write(&path, sample_md("other", &["v1"], &[]))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir, db.clone());

    // First index
    assert!(indexer.index_file(AGENT, "other", &path).await.unwrap());
    // Same content → skip
    assert!(!indexer.index_file(AGENT, "other", &path).await.unwrap());

    // Change content
    fs::write(&path, sample_md("other", &["v2"], &[]))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // Changed → re-index
    assert!(indexer.index_file(AGENT, "other", &path).await.unwrap());
}

#[tokio::test]
async fn write_note_creates_file() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    // rust-doctor-disable-next-line excessive-clone
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

    // rust-doctor-disable-next-line unwrap-in-production
    let path = indexer.write_note(AGENT, "other", &note).await.unwrap();
    assert!(path.exists());
    assert!(path.starts_with(memory_dir.join(AGENT).join("other")));

    // rust-doctor-disable-next-line unwrap-in-production
    let content = fs::read_to_string(&path).await.unwrap();
    assert!(content.contains("category: other"));
    assert!(content.contains("- hello"));
}

#[tokio::test]
async fn append_to_existing_note() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    let pref_dir = setup_category_dir(&memory_dir, AGENT, "preference").await;
    let initial = sample_md("preference", &["fact1"], &["Link1"]);
    fs::write(pref_dir.join("Target.md"), &initial)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

    indexer
        .append_to_note(
            AGENT,
            "preference/Target",
            &["fact2".to_string()],
            &["Link1".to_string(), "Link2".to_string()],
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    // Read back the file
    let content = fs::read_to_string(pref_dir.join("Target.md"))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(content.contains("- fact1"));
    assert!(content.contains("- fact2"));
    assert!(content.contains("[[Link1]]"));
    assert!(content.contains("[[Link2]]"));

    // Verify indexed
    let entry = db
        .get_note_index("preference/Target", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap()
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert_eq!(entry.link_count, 2); // Link1 deduped + Link2
}

#[tokio::test]
async fn append_creates_new_note() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

    indexer
        .append_to_note(AGENT, "other/Brand New", &["a fact".to_string()], &[])
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    assert!(memory_dir
        .join(AGENT)
        .join("other")
        .join("Brand New.md")
        .exists());

    let entry = db
        .get_note_index("other/Brand New", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap()
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert_eq!(entry.category, "other");
}

#[tokio::test]
async fn full_rebuild_parallel_matches_serial_results() {
    // Phase B B3 parity contract: parallel full_rebuild must produce the
    // same (indexed + skipped) total and the same set of indexed rows as
    // the prior serial implementation.
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.ensure_dirs(AGENT).await.unwrap();

    // 50 notes spread across 5 categories.
    for cat in &["preference", "skill", "reference", "plan", "learning"] {
        for i in 0..10 {
            let note = KnowledgeNote {
                title: format!("n-{cat}-{i}"),
                category: (*cat).into(),
                facts: vec![format!("fact {i}")],
                // rust-doctor-disable-next-line unnecessary-allocation
                content_hash: String::new(),
                ..Default::default()
            };
            // rust-doctor-disable-next-line unwrap-in-production
            indexer.write_note(AGENT, cat, &note).await.unwrap();
        }
    }

    // rust-doctor-disable-next-line unwrap-in-production
    let stats = indexer.full_rebuild(AGENT).await.unwrap();
    assert_eq!(stats.indexed + stats.skipped, 50);
    assert_eq!(stats.errors, 0);

    // rust-doctor-disable-next-line unwrap-in-production
    let listed = db.list_notes(AGENT).await.unwrap();
    assert_eq!(listed.len(), 50);
}

#[tokio::test]
async fn rename_note_cascades_wikilinks() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();

    // Create two notes in the same category
    let misc_dir = setup_category_dir(&memory_dir, AGENT, "other").await;
    let note_a = sample_md("other", &["fact A"], &["Old Name"]);
    let note_b = sample_md("other", &["fact B"], &[]);
    fs::write(misc_dir.join("Linker.md"), &note_a)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    fs::write(misc_dir.join("Old Name.md"), &note_b)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

    // Initial index
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.full_rebuild(AGENT).await.unwrap();

    // Rename "Old Name" → "New Name"
    indexer
        .rename_note(AGENT, "Old Name", "New Name")
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    // Old file gone, new file exists
    assert!(!misc_dir.join("Old Name.md").exists());
    assert!(misc_dir.join("New Name.md").exists());

    // Linker.md should now reference [[New Name]]
    let linker_content = fs::read_to_string(misc_dir.join("Linker.md"))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(linker_content.contains("[[New Name]]"));
    assert!(!linker_content.contains("[[Old Name]]"));

    // Old index entry removed, new one present
    // rust-doctor-disable-next-line unwrap-in-production
    let old_paths = db.find_by_filename("Old Name", AGENT).await.unwrap();
    assert!(old_paths.is_empty());
    // rust-doctor-disable-next-line unwrap-in-production
    let new_paths = db.find_by_filename("New Name", AGENT).await.unwrap();
    assert!(!new_paths.is_empty());

    // Linker's outgoing links updated. `to_note` is now the full resolved
    // path rather than the bare title: the wikilink-rewrite loop above
    // re-indexes Linker.md (as "[[New Name]]") before "New Name" itself is
    // re-indexed, so that pass leaves the row dangling on the bare text;
    // rename_note's targeted `backfill_inbound_links` call at the end then
    // revives it as an active, full-path edge.
    // rust-doctor-disable-next-line unwrap-in-production
    let out = db.get_outgoing_links("other/Linker", AGENT).await.unwrap();
    assert!(
        out.contains(&"other/New Name".to_string()),
        "expected the backfilled full-path target, got {out:?}"
    );
    assert!(!out.contains(&"Old Name".to_string()));
}

#[tokio::test]
async fn rename_note_backfills_dangling_links_pointing_at_new_name() {
    // Rename-trigger variant of `write_note_backfills_dangling_links_in_other_notes`:
    // the targeted backfill must also fire when a note is renamed INTO the
    // raw link text (not just created fresh under that name), reviving any
    // other note that was already dangling on it.
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(dir.path().to_path_buf(), db.clone());

    // Linker links to "Target", which doesn't exist yet -> dangles.
    let linker = KnowledgeNote {
        title: "Linker".to_string(),
        category: "other".to_string(),
        links: vec!["Target".to_string()],
        created_at: 1000,
        updated_at: 1000,
        ..Default::default()
    };
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.write_note(AGENT, "other", &linker).await.unwrap();
    let rows = db
        .get_outgoing_link_rows("other/Linker", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let dangling = rows.iter().find(|r| r.to_raw == "Target").unwrap();
    assert_eq!(dangling.status, "dangling");

    // Write an unrelated note under a different name, then rename it to
    // "Target" -- rename_note's backfill should revive Linker's dangling row
    // for the NEW name end-to-end, with no manual relink_unresolved call.
    let something = KnowledgeNote {
        title: "Something".to_string(),
        category: "reference".to_string(),
        created_at: 1000,
        updated_at: 1000,
        ..Default::default()
    };
    indexer
        .write_note(AGENT, "reference", &something)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    indexer
        .rename_note(AGENT, "Something", "Target")
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    let rows = db
        .get_outgoing_link_rows("other/Linker", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let revived = rows.iter().find(|r| r.to_raw == "Target").unwrap();
    assert_eq!(revived.status, "active");
    assert_eq!(revived.to_note, "reference/Target");
}

#[tokio::test]
async fn rename_note_cascades_frontmatter_typed_relations() {
    // Regression: a rename must also re-point OTHER notes' typed relations
    // (frontmatter `- to: <target>` scalars), not just body `[[wikilinks]]`.
    // These bare scalars are invisible to `rewrite_wikilinks`, so before the
    // fix the source note was never re-indexed and its relation row stayed
    // tombstoned, permanently dangling at the old name.
    use crate::memory::notes::Relation;

    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

    // "linker" references "bob" ONLY through a typed frontmatter relation (no
    // body wikilink), isolating the new code path from the existing cascade.
    let bob = KnowledgeNote {
        title: "bob".to_string(),
        category: "reference".to_string(),
        created_at: 1000,
        updated_at: 1000,
        ..Default::default()
    };
    let linker = KnowledgeNote {
        title: "linker".to_string(),
        category: "reference".to_string(),
        created_at: 1000,
        updated_at: 1000,
        ..Default::default()
    };
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.write_note(AGENT, "reference", &bob).await.unwrap();
    indexer
        .write_note(AGENT, "reference", &linker)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    indexer
        .append_relations(
            AGENT,
            "reference/linker",
            &[Relation {
                to: "reference/bob".to_string(),
                rel_type: "knows".to_string(),
                confidence: 1.0,
            }],
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    let linker_md = memory_dir.join(AGENT).join("reference").join("linker.md");
    // rust-doctor-disable-next-line unwrap-in-production
    let before = fs::read_to_string(&linker_md).await.unwrap();
    assert!(before.contains("to: reference/bob"), "got:\n{before}");

    // Rename the target: bob -> bob2.
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.rename_note(AGENT, "bob", "bob2").await.unwrap();

    // (1) On-disk frontmatter is re-pointed (source of truth).
    // rust-doctor-disable-next-line unwrap-in-production
    let after = fs::read_to_string(&linker_md).await.unwrap();
    assert!(
        after.contains("to: reference/bob2"),
        "frontmatter relation not re-pointed, got:\n{after}"
    );
    assert!(
        !after.contains("to: reference/bob\n"),
        "stale old target left behind, got:\n{after}"
    );

    // (2) The notes_links typed-relation row is revived: active and pointing
    // at the renamed note, not tombstoned on the old name.
    let rows = db
        .get_outgoing_link_rows("reference/linker", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    let rel = rows
        .iter()
        .find(|r| r.relation.as_deref() == Some("knows"))
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("typed relation row must exist");
    assert_eq!(
        rel.status, "active",
        "relation must be revived; to_note={}, to_raw={}",
        rel.to_note, rel.to_raw
    );
    assert_eq!(rel.to_note, "reference/bob2");
    assert_eq!(rel.to_raw, "reference/bob2");
}

#[tokio::test]
async fn write_note_backfills_dangling_links_in_other_notes() {
    // End-to-end: finalize_write's best-effort backfill trigger revives a
    // dangling link in another note once the note it points at is written.
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(dir.path().to_path_buf(), db.clone());

    // Linker links to "target", which doesn't exist yet -> dangles.
    let linker = KnowledgeNote {
        title: "Linker".to_string(),
        category: "other".to_string(),
        links: vec!["target".to_string()],
        created_at: 1000,
        updated_at: 1000,
        ..Default::default()
    };
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.write_note(AGENT, "other", &linker).await.unwrap();
    let rows = db
        .get_outgoing_link_rows("other/Linker", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let dangling = rows.iter().find(|r| r.to_raw == "target").unwrap();
    assert_eq!(dangling.status, "dangling");

    // Writing "target" triggers finalize_write's backfill, reviving Linker's
    // dangling row end-to-end (no manual relink_unresolved call).
    let target = KnowledgeNote {
        title: "target".to_string(),
        category: "reference".to_string(),
        created_at: 1000,
        updated_at: 1000,
        ..Default::default()
    };
    // rust-doctor-disable-next-line unwrap-in-production
    indexer
        .write_note(AGENT, "reference", &target)
        .await
        .unwrap();

    let rows = db
        .get_outgoing_link_rows("other/Linker", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let revived = rows.iter().find(|r| r.to_raw == "target").unwrap();
    assert_eq!(revived.status, "active");
    assert_eq!(revived.to_note, "reference/target");
}

#[cfg(test)]
mod reference_hook_tests {
    use super::*;
    use crate::memory::notes::note::KnowledgeNote;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use std::sync::Arc;

    // `write_note_invalidates_orientation` (and the `CountingOrient` mock that
    // existed only to observe it) went out with the `NoteOrientation::invalidate`
    // CUT: the hook it asserted on no longer exists.

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
        // rust-doctor-disable-next-line unwrap-in-production
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

        // rust-doctor-disable-next-line unwrap-in-production
        let dir = tempfile::tempdir().unwrap();
        // rust-doctor-disable-next-line unwrap-in-production
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

#[tokio::test]
async fn append_to_note_preserves_raw_prose_body() {
    // Regression (RF-01): appending to a raw-written prose note used to
    // round-trip through the lossy facts view and wipe the prose.
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db);

    let raw = "---\ncategory: reference\ntags: []\n---\n\n# Design\n\nProse the panel editor wrote.\n\n- seed fact\n";
    indexer
        .write_note_raw(AGENT, "reference", "design-doc", raw)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    indexer
        .append_to_note(
            AGENT,
            "reference/design-doc",
            &["appended fact".to_string()],
            &["Peer Note".to_string()],
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    let on_disk = fs::read_to_string(
        memory_dir
            .join(AGENT)
            .join("reference")
            .join("design-doc.md"),
    )
    .await
    // rust-doctor-disable-next-line unwrap-in-production
    .unwrap();
    assert!(on_disk.contains("# Design"), "heading lost: {on_disk}");
    assert!(
        on_disk.contains("Prose the panel editor wrote."),
        "prose lost: {on_disk}"
    );
    assert!(on_disk.contains("- seed fact"));
    assert!(on_disk.contains("- appended fact"));
    assert!(on_disk.contains("[[Peer Note]]"));
}

#[tokio::test]
async fn delete_note_removes_file_index_and_is_idempotent() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db);

    let cat_dir = setup_category_dir(&memory_dir, AGENT, "plan").await;
    let file = cat_dir.join("old-plan.md");
    fs::write(&file, sample_md("plan", &["obsolete"], &[]))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.index_file(AGENT, "plan", &file).await.unwrap();
    assert!(indexer
        .store()
        .get_note_index("plan/old-plan", AGENT)
        .await
        .unwrap()
        .is_some());

    // rust-doctor-disable-next-line unwrap-in-production
    indexer
        .delete_note(AGENT, "plan", "old-plan")
        .await
        .unwrap();
    assert!(!file.exists());
    assert!(indexer
        .store()
        .get_note_index("plan/old-plan", AGENT)
        .await
        .unwrap()
        .is_none());

    // Second delete of the same note is a no-op, not an error.
    // rust-doctor-disable-next-line unwrap-in-production
    indexer
        .delete_note(AGENT, "plan", "old-plan")
        .await
        .unwrap();
}

#[tokio::test]
async fn append_relations_adds_typed_edge_and_indexes_it() {
    use crate::memory::notes::Relation;

    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

    let note = KnowledgeNote {
        title: "note-a".to_string(),
        category: "reference".to_string(),
        created_at: 1000,
        updated_at: 1000,
        ..Default::default()
    };
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.write_note(AGENT, "reference", &note).await.unwrap();

    indexer
        .append_relations(
            AGENT,
            "reference/note-a",
            &[Relation {
                to: "reference/note-b".to_string(),
                rel_type: "supersedes".to_string(),
                confidence: 1.0,
            }],
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    let on_disk = fs::read_to_string(memory_dir.join(AGENT).join("reference").join("note-a.md"))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(on_disk.contains("relations:"), "got:\n{on_disk}");
    assert!(on_disk.contains("to: reference/note-b"));
    assert!(on_disk.contains("type: supersedes"));

    // The relation is mirrored into notes_links as a typed edge.
    let typed = db
        .get_typed_relations("reference/note-a", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(
        typed
            .iter()
            .any(|(to, rel)| to == "reference/note-b" && rel == "supersedes"),
        "expected typed edge in {typed:?}"
    );
}

#[tokio::test]
async fn superseded_by_list_materializes_typed_edge() {
    // W1: a note carrying `superseded_by: [X]` (the form ingest's
    // `mark_superseded` and the `## Superseded by [[X]]` body section produce,
    // promoted into the frontmatter list by `sync_body_to_frontmatter`) must
    // become a typed `superseded_by` edge in `notes_links` so retrieval's
    // `surface_relations` force-surfaces the newer note. Before this fix only
    // the `relations:`-block encoding produced such an edge — the list form was
    // silently dropped, breaking the STRUCTURAL_STRONG guarantee.
    let db = create_test_db();

    // Index the superseding (newer) note first so the edge resolves Active.
    let newer = KnowledgeNote {
        title: "note-new".to_string(),
        category: "reference".to_string(),
        facts: vec!["new fact".to_string()],
        created_at: 2000,
        updated_at: 2000,
        content_hash: "hash_new".to_string(),
        ..Default::default()
    };
    // rust-doctor-disable-next-line unwrap-in-production
    db.index_note(&newer, AGENT, "reference").await.unwrap();

    // The superseded (older) note points forward via the `superseded_by` list.
    let older = KnowledgeNote {
        title: "note-old".to_string(),
        category: "reference".to_string(),
        facts: vec!["old fact".to_string()],
        superseded_by: vec!["reference/note-new".to_string()],
        created_at: 1000,
        updated_at: 1000,
        content_hash: "hash_old".to_string(),
        ..Default::default()
    };
    // rust-doctor-disable-next-line unwrap-in-production
    db.index_note(&older, AGENT, "reference").await.unwrap();

    let typed = db
        .get_typed_relations("reference/note-old", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(
        typed
            .iter()
            .any(|(to, rel)| to == "reference/note-new" && rel == "superseded_by"),
        "superseded_by list must materialize a typed STRUCTURAL_STRONG edge, got {typed:?}"
    );
}

#[tokio::test]
async fn dated_body_supersession_promotes_through_index_to_force_surface_edge() {
    // Round-4 end-to-end: the *dated* body heading `## Superseded by [[X]]
    // (YYYY-MM-DD)` — the exact form ingest's `mark_superseded` and the
    // orientation prompts actually write to disk — must flow through the real
    // `index_file` promotion+index path all the way to a queryable, typed,
    // STRUCTURAL_STRONG edge that retrieval's `surface_relations` force-surfaces.
    //
    // Chain under test (each link previously proven in isolation; this closes the
    // seam): dated body heading -> `sync_body_to_frontmatter` regex (round-4
    // widened to tolerate the trailing `(date)`) -> `superseded_by:` frontmatter
    // -> `index_note` typed edge (round-3 W1) -> `is_structural_strong` == the
    // input `structural_targets` feeds force-surface. Before the round-4 regex
    // fix the dated heading was silently rejected, so this whole chain never fired
    // for real ingest/orientation-authored supersessions.
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());
    setup_category_dir(&memory_dir, AGENT, "reference").await;

    // Superseding (newer) note first, so the promoted edge resolves Active.
    let new_path = memory_dir.join(AGENT).join("reference").join("note-new.md");
    fs::write(
        &new_path,
        "---\ncategory: reference\ncreated: 2026-07-15\nupdated: 2026-07-15\n---\n\n- current fact\n",
    )
    .await
    // rust-doctor-disable-next-line unwrap-in-production
    .unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    indexer
        .index_file(AGENT, "reference", &new_path)
        .await
        .unwrap();

    // Superseded (older) note carries ONLY the dated body heading — no
    // `superseded_by:` frontmatter — exactly as `mark_superseded` writes it.
    let old_path = memory_dir.join(AGENT).join("reference").join("note-old.md");
    fs::write(
        &old_path,
        "---\ncategory: reference\ncreated: 2026-04-01\nupdated: 2026-07-15\n---\n\n\
         - stale fact\n\n\
         ## Superseded by [[reference/note-new]] (2026-07-15)\n\n\
         _Superseded by a more recent, contradicting note._\n",
    )
    .await
    // rust-doctor-disable-next-line unwrap-in-production
    .unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    indexer
        .index_file(AGENT, "reference", &old_path)
        .await
        .unwrap();

    // The dated heading promoted into a typed `superseded_by` edge in notes_links.
    let typed = db
        .get_typed_relations("reference/note-old", AGENT)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(
        typed
            .iter()
            .any(|(to, rel)| to == "reference/note-new" && rel == "superseded_by"),
        "dated body heading must promote+materialize a typed edge, got {typed:?}"
    );

    // That edge is STRUCTURAL_STRONG — the sole condition `structural_targets`
    // (already unit-tested) checks before force-surfacing the superseding note
    // when the stale note is retrieved. Edge present + strong ⇒ force-surface fires.
    assert!(
        typed
            .iter()
            .any(|(_, rel)| crate::memory::notes::is_structural_strong(rel)),
        "promoted supersession edge must be force-surfaceable, got {typed:?}"
    );
}

#[tokio::test]
async fn append_relations_is_noop_when_all_already_present() {
    use crate::memory::notes::Relation;

    // rust-doctor-disable-next-line unwrap-in-production
    let dir = TempDir::new().unwrap();
    let memory_dir = dir.path().to_path_buf();
    let db = create_test_db();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

    let note = KnowledgeNote {
        title: "note-c".to_string(),
        category: "reference".to_string(),
        created_at: 1000,
        updated_at: 1000,
        ..Default::default()
    };
    // rust-doctor-disable-next-line unwrap-in-production
    indexer.write_note(AGENT, "reference", &note).await.unwrap();

    let rel = Relation {
        to: "reference/note-d".to_string(),
        rel_type: "refers".to_string(),
        confidence: 1.0,
    };
    indexer
        .append_relations(AGENT, "reference/note-c", std::slice::from_ref(&rel))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    let after_first =
        fs::read_to_string(memory_dir.join(AGENT).join("reference").join("note-c.md"))
            .await
            // rust-doctor-disable-next-line unwrap-in-production
            .unwrap();

    // Calling again with the same (to, rel_type) must not rewrite the file
    // (no duplicate relation entries, no spurious updated_at bump).
    indexer
        .append_relations(AGENT, "reference/note-c", &[rel])
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    let after_second =
        fs::read_to_string(memory_dir.join(AGENT).join("reference").join("note-c.md"))
            .await
            // rust-doctor-disable-next-line unwrap-in-production
            .unwrap();
    assert_eq!(after_first, after_second, "no-op must not rewrite the file");
    assert_eq!(
        after_second.matches("to: reference/note-d").count(),
        1,
        "relation must not be duplicated"
    );
}

#[test]
fn canonicalize_category_merges_plural_and_spelling_variants() {
    // Plurals collapse to the single canonical singular.
    assert_eq!(canonicalize_category("projects"), "project");
    assert_eq!(canonicalize_category("preferences"), "preference");
    // `workflows`/`teams`/`systems`/`interests` are deliberately NOT aliased —
    // their singular forms are not registered categories (CATEGORY_DIRS), so
    // rewriting them would yield a category that validation/rebuild rejects.
    // They pass through unchanged, keeping LLM category sovereignty.
    assert_eq!(canonicalize_category("workflows"), "workflows");
    assert_eq!(canonicalize_category("teams"), "teams");
    assert_eq!(canonicalize_category("entities"), "entity");
    // Case-insensitive on match.
    assert_eq!(canonicalize_category("Projects"), "project");
    // Already-canonical singulars pass through unchanged.
    assert_eq!(canonicalize_category("project"), "project");
    assert_eq!(canonicalize_category("entity"), "entity");
    // Intentionally-plural / hyphenated categories are NEVER mangled.
    assert_eq!(canonicalize_category("goal-lessons"), "goal-lessons");
    assert_eq!(canonicalize_category("subagent-run"), "subagent-run");
    // Unknown free categories keep LLM sovereignty (trimmed, otherwise verbatim).
    assert_eq!(canonicalize_category("  research  "), "research");
}

#[test]
fn entity_and_synthesis_are_registered_indexable_categories() {
    // Regression: the ingest prompt tells the LLM to create `entity/<slug>`
    // notes and NoteSynthesisStage writes `synthesis/` pages. Both must be in
    // CATEGORY_DIRS so full_rebuild scans+reconciles them (no silent data loss
    // on an index rebuild) and dream L1 validation accepts them.
    assert!(CATEGORY_DIRS.contains(&"entity"));
    assert!(CATEGORY_DIRS.contains(&"synthesis"));
}

#[test]
fn every_alias_target_is_a_registered_category() {
    // Guard against the category-drift severed wire: an alias whose canonical
    // form is absent from CATEGORY_DIRS actively rewrites an LLM-authored plural
    // into a singular that validation/rebuild then rejects — silently dropping
    // the note on the next index rebuild. Every CATEGORY_ALIASES target MUST be
    // a registered indexable category; this fails loudly if a new alias (or a
    // removed CATEGORY_DIRS entry) reintroduces the drift.
    for &(variant, canonical) in CATEGORY_ALIASES {
        assert!(
            CATEGORY_DIRS.contains(&canonical),
            "alias '{variant}' -> '{canonical}' targets a category absent from CATEGORY_DIRS"
        );
    }
}
