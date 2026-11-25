use super::*;
use crate::memory::store::SqliteMemoryBackend;
use std::sync::Arc;

// Use the installed corpus ID: the schema migrates the legacy `default` ID on reopen.
const CORPUS: &str = crate::routing::DEFAULT_AGENT_ID;
const ORIGINAL_FACT: &str = "An obsolete obsidian convention.";
const SUCCESSOR_FACT: &str = "Prefer the zirconium convention.";

fn replace(title: &str) -> DistillAction {
    DistillAction::Supersede {
        old_note_path: "skill/original".into(),
        title: title.into(),
        rule: SUCCESSOR_FACT.into(),
        confidence: 0.9,
        severity: Severity::High,
        source_facts: vec!["correction-1".into()],
    }
}

#[derive(Clone, Copy, Debug)]
enum Interruption {
    OriginalOnly,
    Archived,
    Published,
    BothProjected,
    RemovedFileWithStaleProjection,
    RemovedProjectionWithLiveFile,
}

#[tokio::test]
async fn a_serialized_replacement_resumes_and_is_recallable_after_each_interruption() {
    for interruption in [
        Interruption::OriginalOnly,
        Interruption::Archived,
        Interruption::Published,
        Interruption::BothProjected,
        Interruption::RemovedFileWithStaleProjection,
        Interruption::RemovedProjectionWithLiveFile,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("memory.db");
        let vault = dir.path().join("vault");
        let root = vault.join(CORPUS);
        let db = Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());
        let indexer = NoteIndexer::new(vault.clone(), db.clone());
        indexer
            .write_note(
                CORPUS,
                "skill",
                &KnowledgeNote {
                    title: "original".into(),
                    category: "skill".into(),
                    facts: vec![ORIGINAL_FACT.into()],
                    created_at: 1_700_000_000,
                    updated_at: 1_700_000_000,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let guard = super::super::lock_note_corpus(&vault, CORPUS).await;
        let effect = indexer
            .prepare_distill_effect_locked(CORPUS, "skill", &replace("successor"))
            .await
            .unwrap()
            .unwrap();
        let serialized = serde_json::to_string(&effect).unwrap();
        let old = effect.supersession.as_ref().unwrap();
        let archive = old.archive_file(&root);
        let archive_bytes = old.archive_markdown(&effect.destination);
        let successor = effect.destination.file(&root);
        if !matches!(interruption, Interruption::OriginalOnly) {
            publish_new(&archive, &archive_bytes).await.unwrap();
        }
        if matches!(
            interruption,
            Interruption::Published
                | Interruption::BothProjected
                | Interruption::RemovedFileWithStaleProjection
                | Interruption::RemovedProjectionWithLiveFile
        ) {
            publish_new(&successor, &effect.markdown).await.unwrap();
        }
        if matches!(
            interruption,
            Interruption::BothProjected
                | Interruption::RemovedFileWithStaleProjection
                | Interruption::RemovedProjectionWithLiveFile
        ) {
            let note = KnowledgeNote::from_markdown("successor", &effect.markdown).unwrap();
            db.index_note(&note, CORPUS, "skill").await.unwrap();
        }
        match interruption {
            Interruption::RemovedFileWithStaleProjection => {
                fs::remove_file(old.original.file(&root)).await.unwrap();
            }
            Interruption::RemovedProjectionWithLiveFile => {
                db.remove_note_index("skill/original", CORPUS)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        drop(guard);
        drop(indexer);
        drop(db);

        let db = Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());
        let indexer = NoteIndexer::new(vault.clone(), db.clone());
        let recovered: PreparedDistillEffect = serde_json::from_str(&serialized).unwrap();
        let _guard = super::super::lock_note_corpus(&vault, CORPUS).await;
        for _ in 0..2 {
            indexer
                .verify_distill_effect_locked(CORPUS, &recovered)
                .await
                .unwrap_or_else(|error| panic!("{interruption:?}: {error}"));
        }
        assert_eq!(fs::read_to_string(archive).await.unwrap(), archive_bytes);
        assert_eq!(
            fs::read_to_string(successor).await.unwrap(),
            effect.markdown
        );
        assert!(!old.original.file(&root).exists());
        let hits = db.search_notes_fts("zirconium", CORPUS, 8).await.unwrap();
        assert_eq!(hits.len(), 1, "{interruption:?}");
        assert_eq!(hits[0].path, "skill/successor");
        assert_eq!(hits[0].content_hash, sha2_hash(&effect.markdown));
        assert!(db
            .search_notes_fts("obsidian", CORPUS, 8)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn identical_same_path_replacement_has_no_effect_or_archive() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    let db = Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap());
    let indexer = NoteIndexer::new(vault.clone(), db.clone());
    let action = replace("original");
    let bytes = RuleDraft {
        title: "original",
        rule: SUCCESSOR_FACT,
        confidence: 0.9,
        severity: Severity::High,
        sources: &["correction-1".into()],
    }
    .render("skill");
    // write_note_raw uses the exact seeded bytes; the rendered dates have day granularity.
    indexer
        .write_note_raw(CORPUS, "skill", "original", &bytes)
        .await
        .unwrap();
    let _guard = super::super::lock_note_corpus(&vault, CORPUS).await;
    assert!(indexer
        .prepare_distill_effect_locked(CORPUS, "skill", &action)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        fs::read_to_string(vault.join(CORPUS).join("skill/original.md"))
            .await
            .unwrap(),
        bytes
    );
    assert!(!vault
        .join(CORPUS)
        .join("archive/skill/original.md")
        .exists());
    assert_eq!(
        db.get_note_index("skill/original", CORPUS)
            .await
            .unwrap()
            .unwrap()
            .content_hash,
        sha2_hash(&bytes)
    );
}
