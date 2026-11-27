use super::super::*;
use super::RENAME_PLAN_FILE;
use crate::memory::notes::wikilink::rewrite_resolved_targets;
use crate::memory::store::SqliteMemoryBackend;

async fn fixture() -> (tempfile::TempDir, NoteIndexer<SqliteMemoryBackend>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap());
    let indexer = NoteIndexer::new(dir.path().join("notes"), db);
    (dir, indexer)
}

async fn seed(
    indexer: &NoteIndexer<SqliteMemoryBackend>,
    category: &str,
    title: &str,
    body: &str,
) -> String {
    let content = format!("---\ncategory: {category}\n---\n{body}\n");
    indexer
        .write_note_raw("default", category, title, &content)
        .await
        .unwrap();
    content
}

#[tokio::test]
async fn checked_rename_selects_exact_source_and_preserves_ambiguous_references() {
    let (_dir, indexer) = fixture().await;
    let content = seed(&indexer, "reference", "same", "Selected").await;
    let other = seed(&indexer, "skill", "same", "Other").await;
    let other_destination = seed(&indexer, "skill", "new", "Other destination").await;
    let source = "---\ncategory: reference\nrelations:\n  - to: reference/same\n    type: supports\n  - to: skill/same\n    type: supports\n---\n[[reference/same|Selected]] [[skill/same]] [[same]]\n";
    indexer
        .write_note_raw("default", "reference", "source", source)
        .await
        .unwrap();
    let address = indexer
        .mutate_note_checked(
            "default",
            "reference/same",
            &note_revision(&content),
            CheckedNoteAction::Rename {
                title: "new".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(address, "reference/new");
    let root = indexer.memory_dir.join("default");
    assert_eq!(
        fs::read_to_string(root.join("skill/new.md")).await.unwrap(),
        other_destination
    );
    assert_eq!(
        fs::read_to_string(root.join("skill/same.md"))
            .await
            .unwrap(),
        other
    );
    assert!(indexer
        .store
        .get_note_index("skill/same", "default")
        .await
        .unwrap()
        .is_some());
    assert!(indexer
        .store
        .get_note_index("reference/same", "default")
        .await
        .unwrap()
        .is_none());
    let rewritten = fs::read_to_string(root.join("reference/source.md"))
        .await
        .unwrap();
    assert!(rewritten.contains("[[reference/new|Selected]] [[skill/same]] [[same]]"));
    assert!(rewritten.contains("- to: reference/new"));
    assert!(rewritten.contains("- to: skill/same"));
    assert!(indexer
        .store
        .get_outgoing_links("reference/source", "default")
        .await
        .unwrap()
        .contains(&"reference/new".into()));
}

#[tokio::test]
async fn checked_mutations_reject_stale_revisions_and_existing_destination() {
    let (_dir, indexer) = fixture().await;
    let original = seed(&indexer, "reference", "original", "Original").await;
    let destination = seed(&indexer, "reference", "destination", "Destination").await;
    let before = indexer
        .store
        .get_note_index("reference/original", "default")
        .await
        .unwrap()
        .unwrap();
    for action in [
        CheckedNoteAction::Update {
            content: "changed".into(),
        },
        CheckedNoteAction::Rename {
            title: "unused".into(),
        },
        CheckedNoteAction::Delete,
    ] {
        assert!(matches!(
            indexer
                .mutate_note_checked("default", "reference/original", "stale", action)
                .await,
            Err(CheckedNoteError::Conflict)
        ));
    }
    assert!(matches!(
        indexer
            .mutate_note_checked(
                "default",
                "reference/original",
                &note_revision(&original),
                CheckedNoteAction::Rename {
                    title: "destination".into()
                }
            )
            .await,
        Err(CheckedNoteError::DestinationExists)
    ));
    assert_eq!(
        fs::read_to_string(indexer.memory_dir.join("default/reference/original.md"))
            .await
            .unwrap(),
        original
    );
    assert_eq!(
        fs::read_to_string(indexer.memory_dir.join("default/reference/destination.md"))
            .await
            .unwrap(),
        destination
    );
    assert_eq!(
        indexer
            .store
            .get_note_index("reference/original", "default")
            .await
            .unwrap()
            .unwrap()
            .content_hash,
        before.content_hash
    );
}

#[tokio::test]
async fn checked_update_reports_partial_and_can_be_reindexed() {
    let (_dir, indexer) = fixture().await;
    let original = seed(&indexer, "reference", "original", "Original").await;
    indexer.store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_checked BEFORE INSERT ON notes_index BEGIN SELECT RAISE(ABORT, 'test: checked projection failure'); END;")
            .map_err(|error| AlephError::other(error.to_string()))
    }).await.unwrap();
    let changed = "---\ncategory: reference\n---\nChanged\n";
    let error = indexer
        .mutate_note_checked(
            "default",
            "reference/original",
            &note_revision(&original),
            CheckedNoteAction::Update {
                content: changed.into(),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, CheckedNoteError::Partial { address, failed_source: None, source } if address == "reference/original" && source.to_string().contains("checked projection failure"))
    );
    let path = indexer.memory_dir.join("default/reference/original.md");
    assert_eq!(fs::read_to_string(&path).await.unwrap(), changed);
    indexer
        .store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_checked;")
                .map_err(|error| AlephError::other(error.to_string()))
        })
        .await
        .unwrap();
    indexer
        .index_file("default", "reference", &path)
        .await
        .unwrap();
    assert!(indexer
        .store
        .get_note_index("reference/original", "default")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn checked_rename_reports_new_address_after_projection_failure() {
    let (_dir, indexer) = fixture().await;
    let original = seed(&indexer, "reference", "original", "Original").await;
    indexer.store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_rename BEFORE INSERT ON notes_index WHEN NEW.path = 'reference/new' BEGIN SELECT RAISE(ABORT, 'test: rename projection failure'); END;")
            .map_err(|error| AlephError::other(error.to_string()))
    }).await.unwrap();
    let result = indexer
        .mutate_note_checked(
            "default",
            "reference/original",
            &note_revision(&original),
            CheckedNoteAction::Rename {
                title: "new".into(),
            },
        )
        .await;
    assert!(
        matches!(result, Err(CheckedNoteError::Partial { address, .. }) if address == "reference/new")
    );
    assert_eq!(
        fs::read_to_string(indexer.memory_dir.join("default/reference/new.md"))
            .await
            .unwrap(),
        original
    );
    assert!(!indexer
        .memory_dir
        .join("default/reference/original.md")
        .exists());
}

#[tokio::test]
async fn checked_rename_rewrites_unique_alias_and_self_link_then_delete_tombstones() {
    let (_dir, indexer) = fixture().await;
    let original = "---\ncategory: reference\naliases: [nickname]\n---\n[[nickname]]\n";
    indexer
        .write_note_raw("default", "reference", "original", original)
        .await
        .unwrap();
    seed(&indexer, "skill", "source", "[[nickname|Label]]").await;
    indexer
        .mutate_note_checked(
            "default",
            "reference/original",
            &note_revision(original),
            CheckedNoteAction::Rename {
                title: "new".into(),
            },
        )
        .await
        .unwrap();
    let path = indexer.memory_dir.join("default/reference/new.md");
    let updated = fs::read_to_string(&path).await.unwrap();
    assert!(updated.contains("[[reference/new]]"));
    let source = fs::read_to_string(indexer.memory_dir.join("default/skill/source.md"))
        .await
        .unwrap();
    assert!(source.contains("[[reference/new|Label]]"));
    indexer
        .mutate_note_checked(
            "default",
            "reference/new",
            &note_revision(&updated),
            CheckedNoteAction::Delete,
        )
        .await
        .unwrap();
    assert!(!path.exists());
    let rows = indexer
        .store
        .get_outgoing_link_rows("skill/source", "default")
        .await
        .unwrap();
    assert!(rows
        .iter()
        .any(|row| row.to_note == "reference/new" && row.status == "tombstone"));
}

#[tokio::test]
async fn checked_mutation_keeps_partitions_and_exact_paths_separate() {
    let (_dir, indexer) = fixture().await;
    let original = seed(&indexer, "reference", "original", "Original").await;
    assert!(matches!(
        indexer
            .mutate_note_checked(
                "other",
                "reference/original",
                &note_revision(&original),
                CheckedNoteAction::Delete
            )
            .await,
        Err(CheckedNoteError::NotFound)
    ));
    assert!(matches!(
        indexer
            .mutate_note_checked(
                "default",
                "reference/../original",
                &note_revision(&original),
                CheckedNoteAction::Delete
            )
            .await,
        Err(CheckedNoteError::Invalid(source)) if source.to_string().contains("path traversal")
    ));
    assert_eq!(
        note_revision("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(indexer
        .memory_dir
        .join("default/reference/original.md")
        .exists());
}

#[test]
fn checked_reference_rewrite_handles_inline_relations_without_rewriting_metadata() {
    let source = "---\ntags: ['[[reference/old]]']\ncustom: [{to: reference/old}]\nrelations: [{to: reference/old, type: supports, confidence: 0.75}, {to: skill/old, type: knows}]\n---\n[[reference/old|Label]] [[old]]\n";
    let rewritten = rewrite_resolved_targets(source, |target| {
        (target == "reference/old").then(|| "reference/new".to_string())
    })
    .unwrap();
    let note = KnowledgeNote::from_markdown("source", &rewritten).unwrap();
    assert_eq!(note.tags, vec!["[[reference/old]]"]);
    assert_eq!(note.relations[0].to, "reference/new");
    assert_eq!(note.relations[0].confidence, 0.75);
    assert_eq!(note.relations[1].to, "skill/old");
    assert!(rewritten.contains("[[reference/new|Label]] [[old]]"));
    assert!(rewritten.contains("to: reference/old"));
}

#[tokio::test]
async fn checked_updates_serialize_revision_check_and_persist_one_winner() {
    let (_dir, indexer) = fixture().await;
    let original = seed(&indexer, "reference", "original", "Original").await;
    let revision = note_revision(&original);
    let first = "---\ncategory: reference\n---\nFirst\n";
    let second = "---\ncategory: reference\n---\nSecond\n";
    let (a, b) = tokio::join!(
        indexer.mutate_note_checked(
            "default",
            "reference/original",
            &revision,
            CheckedNoteAction::Update {
                content: first.into()
            }
        ),
        indexer.mutate_note_checked(
            "default",
            "reference/original",
            &revision,
            CheckedNoteAction::Update {
                content: second.into()
            }
        )
    );
    let expected = match (a, b) {
        (Ok(address), Err(CheckedNoteError::Conflict)) => {
            assert_eq!(address, "reference/original");
            first
        }
        (Err(CheckedNoteError::Conflict), Ok(address)) => {
            assert_eq!(address, "reference/original");
            second
        }
        other => panic!("expected one successful writer and one conflict: {other:?}"),
    };
    assert_eq!(
        fs::read_to_string(indexer.memory_dir.join("default/reference/original.md"))
            .await
            .unwrap(),
        expected
    );
    let indexed = indexer
        .store
        .get_note_index("reference/original", "default")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(indexed.content_hash, note_revision(expected));
}

#[tokio::test]
async fn checked_mutation_surfaces_read_failure_before_side_effects() {
    let (_dir, indexer) = fixture().await;
    let path = indexer.memory_dir.join("default/reference/unreadable.md");
    fs::create_dir_all(&path).await.unwrap();
    let result = indexer
        .mutate_note_checked(
            "default",
            "reference/unreadable",
            "revision",
            CheckedNoteAction::Delete,
        )
        .await;
    assert!(
        matches!(result, Err(CheckedNoteError::Failed(source)) if !source.to_string().is_empty())
    );
    assert!(path.is_dir());
}

#[tokio::test]
async fn checked_rename_preserves_previously_resolved_short_backlink() {
    let (_dir, indexer) = fixture().await;
    let original = seed(&indexer, "reference", "same", "Selected").await;
    seed(&indexer, "reference", "source", "[[same]]").await;
    seed(&indexer, "skill", "same", "Other").await;
    let outgoing = indexer
        .store
        .get_outgoing_link_rows("reference/source", "default")
        .await
        .unwrap();
    assert!(outgoing.iter().any(|row| row.to_raw == "same"
        && row.to_note == "reference/same"
        && row.status == "active"));

    indexer
        .mutate_note_checked(
            "default",
            "reference/same",
            &note_revision(&original),
            CheckedNoteAction::Rename {
                title: "new".into(),
            },
        )
        .await
        .unwrap();
    let source = fs::read_to_string(indexer.memory_dir.join("default/reference/source.md"))
        .await
        .unwrap();
    assert!(source.contains("[[reference/new]]"));
    assert_eq!(
        indexer
            .store
            .get_outgoing_links("reference/source", "default")
            .await
            .unwrap(),
        ["reference/new"]
    );
}

#[tokio::test]
async fn checked_rename_resumes_backlinks_after_restart() {
    let (dir, indexer) = fixture().await;
    const PARTITION: &str = "main";
    let original = "---\ncategory: reference\n---\nSelected\n";
    let first = "---\ncategory: reference\n---\n[[reference/old]]\n";
    let second = "---\ncategory: reference\n---\n[[reference/old|Second]]\n";
    for (title, content) in [("old", original), ("a", first), ("b", second)] {
        indexer
            .write_note_raw(PARTITION, "reference", title, content)
            .await
            .unwrap();
    }
    indexer.store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_first_backlink BEFORE INSERT ON notes_index WHEN NEW.path = 'reference/a' BEGIN SELECT RAISE(ABORT, 'test: backlink projection failure'); END;")
            .map_err(|error| AlephError::other(error.to_string()))
    }).await.unwrap();
    let root = indexer.memory_dir.join(PARTITION);
    let rename = || CheckedNoteAction::Rename {
        title: "new".into(),
    };
    let revision = note_revision(original);
    let result = indexer
        .mutate_note_checked(PARTITION, "reference/old", &revision, rename())
        .await;
    assert!(
        matches!(result, Err(CheckedNoteError::Partial { address, failed_source: Some(failed_source), source }) if address == "reference/new" && failed_source == "reference/a" && source.to_string().contains("backlink projection failure"))
    );
    assert!(fs::read_to_string(root.join("reference/a.md"))
        .await
        .unwrap()
        .contains("[[reference/new]]"));
    assert_eq!(
        fs::read_to_string(root.join("reference/b.md"))
            .await
            .unwrap(),
        second
    );
    drop(indexer);

    let db = Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap());
    let indexer = NoteIndexer::new(dir.path().join("notes"), db);
    indexer
        .store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_first_backlink;")
                .map_err(|error| AlephError::other(error.to_string()))
        })
        .await
        .unwrap();
    assert!(matches!(
        indexer
            .mutate_note_checked(PARTITION, "reference/old", "stale", rename())
            .await,
        Err(CheckedNoteError::Conflict)
    ));
    let edited = second.replace("Second", "Edited");
    fs::write(root.join("reference/b.md"), &edited)
        .await
        .unwrap();
    let result = indexer
        .mutate_note_checked(PARTITION, "reference/old", &revision, rename())
        .await;
    assert!(
        matches!(result, Err(CheckedNoteError::Partial { address, failed_source: Some(failed_source), .. }) if address == "reference/new" && failed_source == "reference/b")
    );
    assert_eq!(
        fs::read_to_string(root.join("reference/b.md"))
            .await
            .unwrap(),
        edited
    );
    fs::write(root.join("reference/b.md"), second)
        .await
        .unwrap();

    assert_eq!(
        indexer
            .mutate_note_checked(PARTITION, "reference/old", &revision, rename())
            .await
            .unwrap(),
        "reference/new"
    );
    assert!(!root.join(RENAME_PLAN_FILE).exists());
    assert!(!root.join("reference/old.md").exists());
    assert_eq!(
        fs::read_to_string(root.join("reference/new.md"))
            .await
            .unwrap(),
        original
    );
    assert!(fs::read_to_string(root.join("reference/a.md"))
        .await
        .unwrap()
        .contains("[[reference/new]]"));
    assert!(fs::read_to_string(root.join("reference/b.md"))
        .await
        .unwrap()
        .contains("[[reference/new|Second]]"));
    assert!(indexer
        .store
        .get_note_index("reference/old", PARTITION)
        .await
        .unwrap()
        .is_none());
    assert!(indexer
        .store
        .get_note_index("reference/new", PARTITION)
        .await
        .unwrap()
        .is_some());
    for source in ["reference/a", "reference/b"] {
        assert_eq!(
            indexer
                .store
                .get_outgoing_links(source, PARTITION)
                .await
                .unwrap(),
            ["reference/new"]
        );
    }
}
