#[cfg(test)]
mod tests {
    use crate::memory::store::sqlite::schema::{
        ddl, drop_obsolete_facts_tables, init_schema, migrate_notes_links_relation,
        migrate_notes_links_to_raw, migrate_unify_default_to_main_agent, migrations,
    };
    use rusqlite::{Connection, OptionalExtension};

    #[test]
    fn init_schema_is_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("first init_schema");
        init_schema(&conn).expect("second init_schema should not error");
    }

    #[test]
    fn recall_signals_table_created() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        conn.prepare("SELECT id FROM recall_signals LIMIT 0")
            .expect("recall_signals should exist");
    }

    #[test]
    fn governance_tables_present_after_init() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        for table in [
            "notes_provenance",
            "notes_review_queue",
            "notes_review_archive",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![table],
                    |_| Ok(true),
                )
                .optional()
                .unwrap()
                .unwrap_or(false);
            assert!(exists, "{table} missing");
        }
    }

    #[test]
    fn graph_tables_created() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        for table in [
            "notes_sources",
            "notes_graph_cache",
            "notes_graph_insights",
            "notes_graph_related",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {table}");
        }
    }

    #[test]
    fn drop_obsolete_tables_is_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        drop_obsolete_facts_tables(&conn).expect("first drop");
        drop_obsolete_facts_tables(&conn).expect("second drop");
    }

    #[test]
    fn drop_obsolete_tables_removes_legacy_facts() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT PRIMARY KEY); \
             CREATE TABLE graph_nodes (id TEXT PRIMARY KEY);",
        )
        .expect("create legacy tables");
        drop_obsolete_facts_tables(&conn).expect("drop legacy");
        assert!(conn.prepare("SELECT id FROM facts LIMIT 0").is_err());
        assert!(conn.prepare("SELECT id FROM graph_nodes LIMIT 0").is_err());
    }

    #[test]
    fn dream_reports_legacy_schema_migrates_to_new_layout() {
        let conn = Connection::open_in_memory().expect("in-memory db");

        conn.execute_batch(
            r#"CREATE TABLE dream_reports (
                id                    TEXT PRIMARY KEY,
                pipeline_type         TEXT NOT NULL,
                started_at            INTEGER NOT NULL,
                finished_at           INTEGER NOT NULL,
                duration_ms           INTEGER NOT NULL,
                facts_collected       INTEGER NOT NULL DEFAULT 0,
                clusters_found        INTEGER NOT NULL DEFAULT 0,
                drift_detected        INTEGER NOT NULL DEFAULT 0,
                drift_summary         TEXT,
                candidates_evaluated  INTEGER NOT NULL DEFAULT 0,
                facts_promoted        INTEGER NOT NULL DEFAULT 0,
                promotion_details     TEXT,
                facts_decayed         INTEGER NOT NULL DEFAULT 0,
                facts_pruned          INTEGER NOT NULL DEFAULT 0,
                nodes_decayed         INTEGER NOT NULL DEFAULT 0,
                edges_decayed         INTEGER NOT NULL DEFAULT 0,
                synthesis_count       INTEGER NOT NULL DEFAULT 0,
                errors                TEXT,
                namespace             TEXT NOT NULL DEFAULT 'owner'
            );"#,
        )
        .expect("create legacy dream_reports");

        conn.execute_batch(
            "INSERT INTO dream_reports
               (id, pipeline_type, started_at, finished_at, duration_ms,
                facts_collected, clusters_found, drift_detected, drift_summary,
                candidates_evaluated, facts_promoted, promotion_details,
                facts_decayed, facts_pruned, nodes_decayed, edges_decayed,
                synthesis_count, errors, namespace)
             VALUES
               ('r1', 'full', 1000, 2000, 1000,
                42, 3, 1, 'topic shift',
                10, 2, 'promoted f1',
                5, 1, 3, 2,
                1, NULL, 'owner'),
               ('r2', 'weekly', 3000, 5000, 2000,
                7, 0, 0, NULL,
                0, 0, NULL,
                0, 0, 0, 0,
                4, 'retry', 'owner');",
        )
        .expect("insert legacy rows");

        init_schema(&conn).expect("init_schema");

        let mut stmt = conn
            .prepare("PRAGMA table_info(dream_reports)")
            .expect("prepare pragma");
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("pragma query")
            .collect::<Result<Vec<_>, _>>()
            .expect("pragma collect");

        let mut sorted = cols.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            vec![
                "duration_ms",
                "errors",
                "evolution_json",
                "feedback_distilled",
                "finished_at",
                "id",
                "namespace",
                "notes_archived",
                "notes_consolidated",
                "notes_woven",
                "pipeline_type",
                "started_at",
                "synthesis_count",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>(),
            "dream_reports must retain the 8 core columns, the 4 notes-era activity \
             counters, and the evolution_json gate-verdict column"
        );

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dream_reports", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 2);

        let r1: (String, String, i64, i64, i64, u32, Option<String>, String) = conn
            .query_row(
                "SELECT id, pipeline_type, started_at, finished_at,
                        duration_ms, synthesis_count, errors, namespace
                   FROM dream_reports WHERE id = 'r1'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(r1.0, "r1");
        assert_eq!(r1.1, "full");
        assert_eq!(r1.2, 1000);
        assert_eq!(r1.3, 2000);
        assert_eq!(r1.4, 1000);
        assert_eq!(r1.5, 1);
        assert_eq!(r1.6, None);
        assert_eq!(r1.7, "owner");

        init_schema(&conn).expect("second init_schema should be idempotent");

        let row_count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM dream_reports", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count_after, 2);
    }

    #[test]
    fn create_query_filed_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(ddl::CREATE_QUERY_FILED).unwrap();
        conn.execute_batch(ddl::CREATE_QUERY_FILED).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM query_filed", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn migrate_dream_reports_drop_legacy_cols_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE dream_reports (
                id TEXT PRIMARY KEY,
                pipeline_type TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                finished_at INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                facts_collected INTEGER NOT NULL DEFAULT 0,
                facts_promoted INTEGER NOT NULL DEFAULT 0,
                synthesis_count INTEGER NOT NULL DEFAULT 0,
                errors TEXT,
                namespace TEXT NOT NULL DEFAULT 'owner'
            )",
        )
        .unwrap();
        migrations::migrate_dream_reports_drop_legacy_cols(&conn).unwrap();
        migrations::migrate_dream_reports_drop_legacy_cols(&conn).unwrap(); // idempotent
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(dream_reports)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(!cols.contains(&"facts_collected".to_string()));
        assert!(!cols.contains(&"facts_promoted".to_string()));
        assert!(cols.contains(&"pipeline_type".to_string()));
        assert!(cols.contains(&"synthesis_count".to_string()));
    }

    #[test]
    fn migrate_dream_reports_add_activity_counters_idempotent_and_backfills() {
        let conn = Connection::open_in_memory().unwrap();
        // Pre-migration schema: only synthesis_count among activity counters.
        conn.execute_batch(
            "CREATE TABLE dream_reports (
                id TEXT PRIMARY KEY,
                pipeline_type TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                finished_at INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                synthesis_count INTEGER NOT NULL DEFAULT 0,
                errors TEXT,
                namespace TEXT NOT NULL DEFAULT 'owner'
            );
            INSERT INTO dream_reports
                (id, pipeline_type, started_at, finished_at, duration_ms, synthesis_count)
                VALUES ('old', 'consolidate', 100, 110, 10, 0);",
        )
        .unwrap();

        migrations::migrate_dream_reports_add_activity_counters(&conn).unwrap();
        migrations::migrate_dream_reports_add_activity_counters(&conn).unwrap(); // idempotent

        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(dream_reports)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for c in [
            "notes_consolidated",
            "notes_woven",
            "notes_archived",
            "feedback_distilled",
        ] {
            assert!(cols.contains(&c.to_string()), "missing column {c}");
        }

        // Existing row backfilled to DEFAULT 0 (not NULL).
        let (nc, nw, na, fd): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT notes_consolidated, notes_woven, notes_archived, feedback_distilled
                   FROM dream_reports WHERE id = 'old'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((nc, nw, na, fd), (0, 0, 0, 0));
    }

    fn seed_default_agent_row(conn: &Connection, path: &str, content: &str) {
        let now: i64 = 1_700_000_000_000;
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json,
                 created_at, updated_at, last_accessed_at, content_hash)
             VALUES (?1, ?2, 'default', 'misc', '[]', ?3, ?3, ?3, ?4)",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), now, content],
        )
        .expect("seed default notes_index");
        conn.execute(
            "INSERT INTO notes_fts (path, filename, content, agent_id)
             VALUES (?1, ?2, ?3, 'default')",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), content],
        )
        .expect("seed default notes_fts");
    }

    fn seed_main_agent_row(conn: &Connection, path: &str, content: &str) {
        let now: i64 = 1_700_000_000_000;
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json,
                 created_at, updated_at, last_accessed_at, content_hash)
             VALUES (?1, ?2, 'main', 'misc', '[]', ?3, ?3, ?3, ?4)",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), now, content],
        )
        .expect("seed main notes_index");
        conn.execute(
            "INSERT INTO notes_fts (path, filename, content, agent_id)
             VALUES (?1, ?2, ?3, 'main')",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), content],
        )
        .expect("seed main notes_fts");
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .expect("count query")
    }

    #[test]
    fn migration_moves_default_rows_to_main() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_default_agent_row(&conn, "personal/foo.md", "F");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_index WHERE agent_id='main' AND path='personal/foo.md'"
            ),
            1,
            "row should be at main"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_index WHERE agent_id='default'"
            ),
            0,
            "no default rows should remain"
        );
    }

    #[test]
    fn migration_preserves_existing_main_on_collision() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_main_agent_row(&conn, "personal/foo.md", "M");
        seed_default_agent_row(&conn, "personal/foo.md", "D");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        let surviving_hash: String = conn
            .query_row(
                "SELECT content_hash FROM notes_index WHERE agent_id='main' AND path='personal/foo.md'",
                [], |r| r.get(0),
            ).expect("query surviving row");
        assert_eq!(
            surviving_hash, "M",
            "main row content_hash should win on collision"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_index WHERE agent_id='default'"
            ),
            0,
            "default row should be deleted even on collision"
        );
    }

    #[test]
    fn migration_purges_test_memory_validation_rows() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        let now: i64 = 1_700_000_000_000;
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json,
                 created_at, updated_at, last_accessed_at, content_hash)
             VALUES ('residue.md', 'residue.md', 'test-memory-validation', 'misc', '[]', ?1, ?1, ?1, 'h')",
            rusqlite::params![now],
        ).expect("seed residue row");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_index WHERE agent_id='test-memory-validation'"
            ),
            0,
            "test-memory-validation rows should be purged"
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_default_agent_row(&conn, "personal/bar.md", "B");
        migrate_unify_default_to_main_agent(&conn).expect("first run");
        let snapshot1 = count(
            &conn,
            "SELECT COUNT(*) FROM notes_index WHERE agent_id='main'",
        );
        migrate_unify_default_to_main_agent(&conn).expect("second run");
        let snapshot2 = count(
            &conn,
            "SELECT COUNT(*) FROM notes_index WHERE agent_id='main'",
        );
        assert_eq!(snapshot1, snapshot2, "second run must be a no-op");
    }

    #[test]
    fn find_by_filename_uses_composite_index() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json, created_at, updated_at, content_hash)
             VALUES ('preference/rust', 'rust', 'default', 'preference', '[]', 0, 0, 'h')",
            [],
        ).unwrap();

        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT path FROM notes_index WHERE agent_id = ?1 AND filename = ?2",
                rusqlite::params!["default", "rust"],
                |r| r.get::<_, String>(3),
            )
            .unwrap();
        assert!(
            plan.contains("idx_notes_filename_agent"),
            "expected planner to use composite index 'idx_notes_filename_agent', got: {plan}"
        );
    }

    #[test]
    fn migrate_notes_links_adds_to_raw_and_backfills() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE notes_links (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL DEFAULT 'default',
                from_note TEXT NOT NULL,
                to_note TEXT NOT NULL,
                UNIQUE(agent_id, from_note, to_note)
            );
            INSERT INTO notes_links (agent_id, from_note, to_note)
                VALUES ('a', 'cat/x', 'rust');",
        )
        .unwrap();

        migrate_notes_links_to_raw(&conn).unwrap();

        let to_raw: String = conn
            .query_row(
                "SELECT to_raw FROM notes_links WHERE from_note='cat/x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(to_raw, "rust");

        migrate_notes_links_to_raw(&conn).unwrap();
        let to_raw_again: String = conn
            .query_row(
                "SELECT to_raw FROM notes_links WHERE from_note='cat/x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(to_raw_again, "rust");
    }

    #[test]
    fn migration_preserves_link_topology() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_default_agent_row(&conn, "A.md", "a");
        seed_default_agent_row(&conn, "B.md", "b");
        conn.execute(
            "INSERT INTO notes_links (agent_id, from_note, to_note, to_raw) VALUES ('default', 'A.md', 'B.md', 'B.md')",
            [],
        ).expect("seed default link");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        assert_eq!(
            count(&conn,
                "SELECT COUNT(*) FROM notes_links WHERE agent_id='main' AND from_note='A.md' AND to_note='B.md'"
            ),
            1, "link must survive under main"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_links WHERE agent_id='default'"
            ),
            0,
            "default links should be deleted"
        );
    }

    #[test]
    fn notes_links_confidence_migration_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // Simulate a legacy table without confidence.
        conn.execute_batch(
            "CREATE TABLE notes_links (id INTEGER PRIMARY KEY AUTOINCREMENT, \
             agent_id TEXT NOT NULL DEFAULT 'default', from_note TEXT NOT NULL, \
             to_note TEXT NOT NULL, to_raw TEXT NOT NULL, relation TEXT, \
             UNIQUE(agent_id, from_note, to_note))",
        )
        .unwrap();
        migrations::migrate_notes_links_confidence(&conn).unwrap();
        migrations::migrate_notes_links_confidence(&conn).unwrap(); // twice = no-op
        let has: bool = conn
            .prepare("PRAGMA table_info(notes_links)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .any(|n| n.is_ok_and(|n| n == "confidence"));
        assert!(has);
    }

    #[test]
    fn migrate_notes_links_relation_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE notes_links (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL DEFAULT 'default',
                from_note TEXT NOT NULL,
                to_note TEXT NOT NULL,
                to_raw TEXT NOT NULL,
                UNIQUE(agent_id, from_note, to_note)
            );",
        )
        .unwrap();
        migrate_notes_links_relation(&conn).expect("first migration");
        migrate_notes_links_relation(&conn).expect("second migration is a no-op");
        let count = conn
            .prepare("PRAGMA table_info(notes_links)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .filter(|n| n == "relation")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn migrate_notes_links_lifecycle_adds_columns_and_backfills_dangling() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // Legacy-shaped table: no resolved_by / status / label.
        conn.execute_batch(
            "CREATE TABLE notes_links (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL DEFAULT 'default',
                from_note TEXT NOT NULL,
                to_note TEXT NOT NULL,
                to_raw TEXT NOT NULL,
                relation TEXT,
                confidence REAL NOT NULL DEFAULT 1.0,
                UNIQUE(agent_id, from_note, to_note));
             INSERT INTO notes_links (agent_id, from_note, to_note, to_raw)
             VALUES ('a', 'p/x', 'rust', 'rust');          -- legacy dangling marker
             INSERT INTO notes_links (agent_id, from_note, to_note, to_raw)
             VALUES ('a', 'p/x', 'ref/rust', 'rust');       -- resolved",
        )
        .unwrap();

        migrations::migrate_notes_links_lifecycle(&conn).unwrap();

        let (status_dangling, status_active): (String, String) = conn
            .query_row(
                "SELECT
                   (SELECT status FROM notes_links WHERE to_note = 'rust'),
                   (SELECT status FROM notes_links WHERE to_note = 'ref/rust')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status_dangling, "dangling");
        assert_eq!(status_active, "active");

        // Idempotent: second run is a no-op and must not re-flip statuses.
        conn.execute(
            "UPDATE notes_links SET status = 'tombstone' WHERE to_note = 'rust'",
            [],
        )
        .unwrap();
        migrations::migrate_notes_links_lifecycle(&conn).unwrap();
        let s: String = conn
            .query_row(
                "SELECT status FROM notes_links WHERE to_note = 'rust'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(s, "tombstone", "re-run must not re-backfill");
    }
}
