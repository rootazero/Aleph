use super::MarkdownSessionStore;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

fn raw(id: &str, created_at: i64, agent: &str, path: &str) -> RawMemory {
    let mut raw = RawMemory::new(
        "Hello 世界 %/_\\\n---\n終わり\n".into(),
        RawMemorySource::Transcript,
    )
    .with_agent(agent)
    .with_session("session-a")
    .with_path(path);
    raw.id = id.into();
    raw.created_at = created_at;
    raw
}

fn ids(rows: &[RawMemory]) -> Vec<&str> {
    rows.iter().map(|row| row.id.as_str()).collect()
}

async fn deletion_paths(
    store: &MarkdownSessionStore,
    id: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let (agent, file_id): (String, String) = store
        .db
        .lock()
        .await
        .query_row(
            "SELECT agent_id, file_id FROM raw_memory_index WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let filename = format!("{}.md", uuid::Uuid::parse_str(&file_id).unwrap().simple());
    (
        store
            .base_dir
            .join("raw/memories")
            .join(super::utils::compute_sha256(agent.as_bytes()))
            .join(&filename),
        store.base_dir.join("raw/recovery").join(filename),
    )
}

#[tokio::test]
async fn raw_memory_path_uniqueness_is_installed_on_existing_metadata_db() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    let original = raw("original", 1, "owner", "shared/path");
    store.insert_raw_memory(&original).await.unwrap();
    // Simulate metadata.db created before the partial unique index existed.
    store
        .db
        .lock()
        .await
        .execute_batch("DROP INDEX IF EXISTS idx_raw_memory_agent_path_unique")
        .unwrap();
    drop(store);

    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    let mut duplicate = raw("duplicate", 2, "owner", "shared/path");
    duplicate.content = "must not overwrite".into();
    let error = store.insert_raw_memory(&duplicate).await.unwrap_err();
    assert!(error.to_string().contains("UNIQUE constraint failed"));
    store.insert_raw_memory_or_ignore(&duplicate).await.unwrap();
    assert_eq!(store.raw_owner(&duplicate.id).await.unwrap(), None);
    let retained = store
        .find_by_path("shared/path", "owner")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&retained).unwrap(),
        serde_json::to_value(&original).unwrap()
    );

    let other = raw("other", 3, "other-owner", "shared/path");
    store.insert_raw_memory(&other).await.unwrap();
    assert_eq!(
        store
            .find_by_path("shared/path", "other-owner")
            .await
            .unwrap()
            .unwrap()
            .id,
        other.id
    );
    for id in ["null-one", "null-two"] {
        let mut record = raw(id, 4, "owner", "unused");
        record.path = None;
        store.insert_raw_memory(&record).await.unwrap();
        assert_eq!(store.raw_owner(id).await.unwrap().as_deref(), Some("owner"));
    }
}

#[tokio::test]
async fn raw_memory_reopen_recovers_rolled_back_delete_and_cleans_committed_tombstone() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    let kept = raw("keep", 1, "../owner", "one");
    let removed = raw("remove", 2, "other", "two");
    store.insert_raw_memory(&kept).await.unwrap();
    store.insert_raw_memory(&removed).await.unwrap();
    store
        .mark_raw_as_processed(std::slice::from_ref(&kept.id))
        .await
        .unwrap();
    let (kept_path, kept_backup) = deletion_paths(&store, &kept.id).await;
    let (removed_path, removed_backup) = deletion_paths(&store, &removed.id).await;
    std::fs::create_dir_all(kept_backup.parent().unwrap()).unwrap();
    let original = std::fs::read(&kept_path).unwrap();

    // Simulate process death after the move, before SQLite commit. Closing this
    // connection rolls back the uncommitted DELETE, leaving a missing body.
    {
        let mut db = store.db.lock().await;
        let tx = db.transaction().unwrap();
        tx.execute("DELETE FROM raw_memory_index WHERE id = 'keep'", [])
            .unwrap();
        std::fs::rename(&kept_path, &kept_backup).unwrap();
    }
    // Simulate process death after commit, before tombstone cleanup.
    std::fs::rename(&removed_path, &removed_backup).unwrap();
    store
        .db
        .lock()
        .await
        .execute("DELETE FROM raw_memory_index WHERE id = 'remove'", [])
        .unwrap();
    drop(store);
    // Recovery also recreates an empty owner directory removed after the move.
    std::fs::remove_dir(kept_path.parent().unwrap()).unwrap();
    assert!(!kept_path.exists());
    assert!(kept_backup.exists());
    assert!(removed_backup.exists());

    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    assert_eq!(std::fs::read(&kept_path).unwrap(), original);
    assert!(!kept_backup.exists());
    assert!(!removed_path.exists());
    assert!(!removed_backup.exists());
    let rows = store
        .get_raws_by_ids(&kept.agent_id, std::slice::from_ref(&kept.id))
        .await
        .unwrap();
    assert_eq!(ids(&rows), ["keep"]);
    assert_eq!(rows[0].content, kept.content);
    assert!(rows[0].is_processed);
    assert_eq!(store.raw_owner(&removed.id).await.unwrap(), None);
    // Owner denial must not move or delete any body, including after recovery.
    assert!(!store.remove_raw(&kept.id, "other").await.unwrap());
    assert_eq!(std::fs::read(&kept_path).unwrap(), original);
    assert!(!kept_backup.exists());
    drop(store);

    // Recovery is idempotent; normal owner-authorized deletion still works.
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    assert!(store.remove_raw(&kept.id, &kept.agent_id).await.unwrap());
    assert_eq!(store.raw_owner(&kept.id).await.unwrap(), None);
    assert!(!kept_path.exists());
    assert!(!kept_backup.exists());
    assert!(!store.remove_raw(&kept.id, &kept.agent_id).await.unwrap());
}

#[tokio::test]
async fn raw_memory_commit_failure_restores_body_immediately_and_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    let record = raw("commit-failure", 1, "owner", "one");
    store.insert_raw_memory(&record).await.unwrap();
    let (path, backup) = deletion_paths(&store, &record.id).await;
    let original = std::fs::read(&path).unwrap();
    // A deferred FK fails only at COMMIT, after the real deletion path moved
    // the Markdown body. No production fault hook or schema change is needed.
    store
        .db
        .lock()
        .await
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE delete_guard (id TEXT PRIMARY KEY);
             CREATE TABLE delete_pending (
                 id TEXT REFERENCES delete_guard(id) DEFERRABLE INITIALLY DEFERRED
             );
             CREATE TRIGGER fail_raw_delete AFTER DELETE ON raw_memory_index
             BEGIN INSERT INTO delete_pending VALUES (OLD.id); END;",
        )
        .unwrap();
    let error = store
        .remove_raw(&record.id, &record.agent_id)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("FOREIGN KEY constraint failed"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!backup.exists());
    assert_eq!(
        store.raw_owner(&record.id).await.unwrap().as_deref(),
        Some("owner")
    );
    assert_eq!(
        store
            .get_raws_by_ids("owner", std::slice::from_ref(&record.id))
            .await
            .unwrap()[0]
            .content,
        record.content
    );
    drop(store);
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!backup.exists());
    assert_eq!(
        store.raw_owner(&record.id).await.unwrap().as_deref(),
        Some("owner")
    );
    store
        .db
        .lock()
        .await
        .execute_batch("DROP TRIGGER fail_raw_delete")
        .unwrap();
    assert!(store
        .remove_raw(&record.id, &record.agent_id)
        .await
        .unwrap());
    assert!(!path.exists());
    assert!(!backup.exists());
}

#[tokio::test]
async fn raw_memory_markdown_round_trip_and_file_backed_search() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    let mut record = raw("one", 1, "main", "aleph://transcript/one");
    record.source = RawMemorySource::Delegation {
        child_agent_id: "child: \"中文\"\n---".into(),
    };
    record.attachment_text = Some("附件\n---\n%/_\\\u{0}".into());
    store.insert_raw_memory(&record).await.unwrap();

    let rows = store
        .get_raws_by_ids("main", &["one".into()])
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&rows[0]).unwrap(),
        serde_json::to_value(&record).unwrap()
    );
    assert!(store
        .get_raws_by_ids("other", &["one".into()])
        .await
        .unwrap()
        .is_empty());
    assert!(store.get_raws_by_ids("main", &[]).await.unwrap().is_empty());
    let found = store
        .search_raw_by_path_prefix("aleph://transcript/", "main", "hELLo 世界 %/_\\", 1)
        .await
        .unwrap();
    assert_eq!(ids(&found), ["one"]);
    assert!(store
        .search_raw_by_path_prefix("", "main", "世間", 1)
        .await
        .unwrap()
        .is_empty());

    // Editing only the Markdown body must change reads/search, not an SQLite copy.
    let agent_dir = std::fs::read_dir(dir.path().join("raw/memories"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let file = std::fs::read_dir(agent_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let markdown = std::fs::read_to_string(&file).unwrap();
    assert!(markdown.starts_with("---\n"));
    assert!(markdown.ends_with(&record.content));
    std::fs::write(
        &file,
        markdown.strip_suffix(&record.content).unwrap().to_owned() + "Edited 正文\n",
    )
    .unwrap();
    assert!(store
        .search_raw_by_path_prefix("", "main", "hello", 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .find_by_path("aleph://transcript/one", "main")
            .await
            .unwrap()
            .unwrap()
            .content,
        "Edited 正文\n"
    );
    assert!(store
        .find_by_path("aleph://transcript/one/", "main")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .find_by_path("aleph://transcript/one", "other")
        .await
        .unwrap()
        .is_none());

    let db = store.db.lock().await;
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'raw_memories'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let mut stmt = db.prepare("PRAGMA table_info(raw_memory_index)").unwrap();
    let columns: Vec<String> = stmt
        .query_map([], |r| r.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!columns
        .iter()
        .any(|c| c == "content" || c == "attachment_text"));
}

#[tokio::test]
async fn raw_memory_pending_counts_order_and_processed_state_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    for record in [
        raw("new", 30, "z", "p/new"),
        raw("old", 10, "z", "p/old"),
        raw("other", 20, "a", "p/other"),
    ] {
        store.insert_raw_memory(&record).await.unwrap();
    }
    assert_eq!(store.unprocessed_agent_ids().await.unwrap(), ["a", "z"]);
    assert_eq!(store.count_unprocessed("z").await.unwrap(), 2);
    assert_eq!(store.count_unprocessed("missing").await.unwrap(), 0);
    assert_eq!(
        ids(&store.get_unprocessed_raw_memories("z", 1).await.unwrap()),
        ["old"]
    );
    assert!(store
        .get_unprocessed_raw_memories("z", 0)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(store.mark_raw_as_processed(&[]).await.unwrap(), 0);
    assert_eq!(
        store
            .mark_raw_as_processed(&["old".into(), "new".into(), "missing".into()])
            .await
            .unwrap(),
        2
    );
    // SQLite's matched-row count includes already processed IDs.
    assert_eq!(
        store.mark_raw_as_processed(&["old".into()]).await.unwrap(),
        1
    );
    drop(store);
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    assert_eq!(store.count_unprocessed("z").await.unwrap(), 0);
    assert_eq!(store.unprocessed_agent_ids().await.unwrap(), ["a"]);
    assert!(store
        .get_unprocessed_raw_memories("z", 10)
        .await
        .unwrap()
        .is_empty());
    let rows = store.get_raw_by_path_prefix("p/", "z", 10).await.unwrap();
    assert_eq!(ids(&rows), ["old", "new"]);
    assert!(rows.iter().all(|r| r.is_processed));
}

#[tokio::test]
async fn raw_memory_prefix_since_and_search_are_literal_and_limit_after_filtering() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    for record in [
        raw("new", 30, "main", "p%/_\\new"),
        raw("old", 10, "main", "p%/_\\old"),
        raw("middle", 20, "main", "p%/_\\middle"),
        raw("wildcard", 40, "main", "pXX/X\\wrong"),
        raw("other", 50, "other", "p%/_\\other"),
    ] {
        store.insert_raw_memory(&record).await.unwrap();
    }
    assert_eq!(
        ids(&store
            .get_raw_by_path_prefix("p%/_\\", "main", 10)
            .await
            .unwrap()),
        ["old", "middle", "new"]
    );
    assert_eq!(
        ids(&store
            .get_raw_by_path_prefix_since("p%/_\\", "main", 10, 1)
            .await
            .unwrap()),
        ["middle"]
    );
    assert_eq!(
        ids(&store
            .search_raw_by_path_prefix("p%/_\\", "main", "%/_\\", 1)
            .await
            .unwrap()),
        ["new"]
    );
    assert!(store
        .search_raw_by_path_prefix("", "main", "", 0)
        .await
        .unwrap()
        .is_empty());
    let mut recent = raw("latest-match", 2000, "main", "p%/_\\recent");
    recent.content = "unique needle".into();
    store.insert_raw_memory(&recent).await.unwrap();
    assert_eq!(
        ids(&store
            .search_raw_by_path_prefix("p%/_\\", "main", "needle", 1)
            .await
            .unwrap()),
        ["latest-match"]
    );
}

#[tokio::test]
async fn raw_memory_session_and_source_queries_include_processed_and_source_details() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    let mut first = raw("first", 10, "main", "one");
    first.source = RawMemorySource::Delegation {
        child_agent_id: "child-a".into(),
    };
    first.is_processed = true;
    let mut second = raw("second", 20, "main", "two");
    second.source = RawMemorySource::Delegation {
        child_agent_id: "child-b".into(),
    };
    let mut third = raw("third", 30, "main", "three");
    third.session_id = Some("session-b".into());
    for record in [&second, &third, &first, &raw("other", 1, "other", "one")] {
        store.insert_raw_memory(record).await.unwrap();
    }
    assert_eq!(
        ids(&store
            .get_raws_by_session("main", "session-a")
            .await
            .unwrap()),
        ["first", "second"]
    );
    assert!(store
        .get_raws_by_session("main", "unknown")
        .await
        .unwrap()
        .is_empty());
    let rows = store
        .get_raw_by_source(
            RawMemorySource::Delegation {
                child_agent_id: "ignored-detail".into(),
            },
            "main",
            10,
        )
        .await
        .unwrap();
    assert_eq!(ids(&rows), ["second", "first"]);
    assert_eq!(rows[0].source, second.source);
    assert!(rows[1].is_processed);
    assert_eq!(
        ids(&store
            .get_raw_by_source(first.source.clone(), "main", 1)
            .await
            .unwrap()),
        ["second"]
    );
    let rows = store
        .get_raws_by_ids(
            "main",
            &[
                "third".into(),
                "first".into(),
                "third".into(),
                "other".into(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(ids(&rows).contains(&"first"));
    assert!(ids(&rows).contains(&"third"));
}

#[tokio::test]
async fn raw_memory_duplicates_do_not_overwrite_and_unsafe_ids_stay_inside_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarkdownSessionStore::new(dir.path()).await.unwrap();
    let original = raw("../../outside:ID\\file", 1, "../agent/C:\\bad", "one");
    store.insert_raw_memory(&original).await.unwrap();
    let mut duplicate = original.clone();
    duplicate.content = "must not overwrite".into();
    duplicate.agent_id = "other".into();
    assert!(store.insert_raw_memory(&duplicate).await.is_err());
    store.insert_raw_memory_or_ignore(&duplicate).await.unwrap();
    let rows = store
        .get_raws_by_ids(&original.agent_id, std::slice::from_ref(&original.id))
        .await
        .unwrap();
    assert_eq!(rows[0].content, original.content);
    assert!(store
        .get_raws_by_ids("other", std::slice::from_ref(&original.id))
        .await
        .unwrap()
        .is_empty());
    let root = dir.path().join("raw/memories");
    let agent_dirs: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(agent_dirs.len(), 1);
    let files: Vec<_> = std::fs::read_dir(agent_dirs[0].path())
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path().extension().unwrap(), "md");
    assert!(files[0]
        .path()
        .canonicalize()
        .unwrap()
        .starts_with(dir.path().canonicalize().unwrap()));
    std::fs::remove_file(files[0].path()).unwrap();
    assert!(store
        .get_raws_by_ids(&original.agent_id, &[original.id])
        .await
        .is_err());
}

#[tokio::test]
async fn raw_memory_concurrent_store_instances_keep_one_complete_winner() {
    let dir = tempfile::tempdir().unwrap();
    let first = MarkdownSessionStore::new(dir.path()).await.unwrap();
    let second = MarkdownSessionStore::new(dir.path()).await.unwrap();
    let left = raw("race", 1, "left", "one");
    let mut right = raw("race", 2, "right", "two");
    right.content = "right winner".into();
    let (a, b) = tokio::join!(
        first.insert_raw_memory_or_ignore(&left),
        second.insert_raw_memory_or_ignore(&right)
    );
    a.unwrap();
    b.unwrap();
    let left_rows = first
        .get_raws_by_ids("left", &["race".into()])
        .await
        .unwrap();
    let right_rows = second
        .get_raws_by_ids("right", &["race".into()])
        .await
        .unwrap();
    assert_eq!(left_rows.len() + right_rows.len(), 1);
    let winner = left_rows.first().or_else(|| right_rows.first()).unwrap();
    assert_eq!(
        serde_json::to_value(winner).unwrap(),
        serde_json::to_value(if left_rows.is_empty() { &right } else { &left }).unwrap()
    );
}
