use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind};
use rusqlite::{params, Connection};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Default)]
pub struct CatalogFilter {
    pub id: Option<String>,
    pub kind: Option<ExtensionKind>,
    pub category: Option<ExtensionCategory>,
    pub source_id: Option<String>,
    /// Free-text query, applied by [`matches_query`] after the indexed columns
    /// have narrowed the rows.
    pub query: Option<String>,
}

/// Case-insensitive free-text match over the fields a human (or a model) would
/// search by: name, description, tags, and author.
///
/// Applied in Rust rather than SQL because the indexed `name_lc` column alone
/// misses the description and tags — which is where an extension actually says
/// what it does — and matching the stored JSON blob with `LIKE` would hit the
/// key names too (`"kind":"mcp"` matching a search for `mcp`).
#[must_use]
fn matches_query(e: &ExtensionEntry, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    e.name.to_lowercase().contains(&q)
        || e.description.to_lowercase().contains(&q)
        || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
        || e.author
            .as_deref()
            .is_some_and(|a| a.to_lowercase().contains(&q))
}

/// Schema: one row per extension; `data` holds the full JSON, indexed columns
/// drive filtering. `name_lc` is the stable sort key.
///
/// The same file also carries the install provenance ledger
/// (`hub::origin::init_origin_schema`) so both tables are created together and a
/// fresh install never sees one without the other.
pub(super) fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        // PRAGMA ordering matters: WAL must come before any read/write on this
        // connection, and `busy_timeout` only takes effect on the next statement.
        // We don't `PRAGMA synchronous=NORMAL` here — full-sync is cheaper than
        // diagnosing a half-written catalog row after a crash.
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         CREATE TABLE IF NOT EXISTS catalog (
            id        TEXT PRIMARY KEY,
            kind      TEXT NOT NULL,
            category  TEXT NOT NULL,
            name_lc   TEXT NOT NULL,
            source_id TEXT NOT NULL,
            installed INTEGER NOT NULL DEFAULT 0,
            data      TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_catalog_category ON catalog(category);
        CREATE INDEX IF NOT EXISTS idx_catalog_kind ON catalog(kind);
        CREATE INDEX IF NOT EXISTS idx_catalog_source ON catalog(source_id);",
    )?;
    crate::hub::origin::init_origin_schema(conn)
}

fn upsert_entry(conn: &Connection, e: &ExtensionEntry) -> rusqlite::Result<()> {
    let data = serde_json::to_string(e)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    conn.execute(
        "INSERT INTO catalog (id, kind, category, name_lc, source_id, installed, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            kind=excluded.kind, category=excluded.category, name_lc=excluded.name_lc,
            source_id=excluded.source_id, installed=excluded.installed, data=excluded.data",
        params![
            e.id,
            e.kind.as_str(),
            e.category.as_str(),
            e.name.to_lowercase(),
            e.source_id,
            e.installed as i64,
            data,
        ],
    )?;
    Ok(())
}

fn query_entries(conn: &Connection, f: &CatalogFilter) -> rusqlite::Result<Vec<ExtensionEntry>> {
    let mut sql = String::from("SELECT data FROM catalog WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(id) = &f.id {
        sql.push_str(" AND id = ?");
        args.push(Box::new(id.clone()));
    }
    if let Some(k) = f.kind {
        sql.push_str(" AND kind = ?");
        args.push(Box::new(k.as_str().to_string()));
    }
    if let Some(c) = f.category {
        sql.push_str(" AND category = ?");
        args.push(Box::new(c.as_str().to_string()));
    }
    if let Some(s) = &f.source_id {
        sql.push_str(" AND source_id = ?");
        args.push(Box::new(s.clone()));
    }
    sql.push_str(" ORDER BY name_lc");
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |row| {
        let data: String = row.get(0)?;
        serde_json::from_str::<ExtensionEntry>(&data).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
    })?;
    let mut out: Vec<ExtensionEntry> = rows.collect::<rusqlite::Result<_>>()?;
    if let Some(q) = &f.query {
        out.retain(|e| matches_query(e, q));
    }
    Ok(out)
}

fn clear_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM catalog WHERE source_id = ?1",
        params![source_id],
    )
}

fn count_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
    conn.query_row(
        "SELECT COUNT(*) FROM catalog WHERE source_id = ?1",
        params![source_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as usize)
}

pub struct CatalogCache {
    conn: Arc<Mutex<Connection>>,
}

impl CatalogCache {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
    /// Run `f` against the connection on Tokio's blocking pool.
    ///
    /// Every async method below goes through here, so no caller holds
    /// synchronous SQLite (statement execution, WAL fsync, `busy_timeout`
    /// waits of up to 5 s, JSON (de)serialisation of rows) on a runtime
    /// worker. The async `Mutex` is acquired with `lock_owned().await` BEFORE
    /// the hop and the `OwnedMutexGuard` moves into the blocking closure, so:
    ///
    /// - writers stay serialised exactly as before (one connection, one
    ///   guard; no connection duplication, no second runtime);
    /// - a transaction opened inside `f` begins, runs and commits/rolls back
    ///   entirely inside that one blocking task (atomicity unchanged);
    /// - if the caller's future is dropped mid-flight the blocking task still
    ///   runs to completion and releases the guard when done — a started
    ///   transaction is never abandoned half-applied.
    ///
    /// Only owned inputs cross the boundary (callers clone their slices).
    ///
    /// Failure modes are explicit, never success/default: a panic inside `f`
    /// is re-raised on the caller (the guard is released on unwind and a
    /// dropped `Transaction` rolls back; the tokio mutex does not poison);
    /// a cancelled blocking task (runtime shutdown) becomes
    /// `rusqlite::Error::SqliteFailure(SQLITE_INTERRUPT, ..)`.
    async fn with_conn<T, F>(&self, f: F) -> rusqlite::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let mut guard = Arc::clone(&self.conn).lock_owned().await;
        match tokio::task::spawn_blocking(move || f(&mut guard)).await {
            Ok(result) => result,
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(e) => Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
                Some(format!("catalog blocking task cancelled: {e}")),
            )),
        }
    }
    pub async fn upsert_many(&self, entries: &[ExtensionEntry]) -> rusqlite::Result<()> {
        let entries = entries.to_vec();
        self.with_conn(move |conn| {
            // Wrap the whole batch so any per-row failure rolls back every prior
            // insert in the same call. (See `replace_source` for the slot-level
            // equivalent that adds the clear-and-refill semantics.)
            let tx = conn.transaction()?;
            for e in &entries {
                upsert_entry(&tx, e)?;
            }
            tx.commit()
        })
        .await
    }
    pub async fn query(&self, f: &CatalogFilter) -> rusqlite::Result<Vec<ExtensionEntry>> {
        let filter = f.clone();
        self.with_conn(move |conn| query_entries(conn, &filter))
            .await
    }
    /// Atomic per-source refresh: clear the source's rows then insert fresh.
    ///
    /// The whole delete-then-insert is wrapped in a single SQLite transaction
    /// so any failure (disk-full, malformed entry, lock contention) leaves the
    /// slot exactly as it was — never half-populated.
    pub async fn replace_source(
        &self,
        source_id: &str,
        entries: &[ExtensionEntry],
    ) -> rusqlite::Result<()> {
        let source_id = source_id.to_owned();
        let entries = entries.to_vec();
        self.with_conn(move |conn| {
            let tx = conn.transaction()?;
            clear_source(&tx, &source_id)?;
            for e in &entries {
                upsert_entry(&tx, e)?;
            }
            tx.commit()
        })
        .await
    }
    /// Number of cached rows for a source. Used by the cold-start primer.
    pub async fn count_source(&self, source_id: &str) -> rusqlite::Result<usize> {
        let source_id = source_id.to_owned();
        self.with_conn(move |conn| count_source(conn, &source_id))
            .await
    }

    // --- install provenance ledger (see `hub::origin`) ---------------------

    /// Record one completed Hub install. Idempotent by catalog entry id.
    pub async fn record_origin(
        &self,
        origin: &crate::hub::origin::InstallOrigin,
    ) -> rusqlite::Result<()> {
        let origin = origin.clone();
        self.with_conn(move |conn| crate::hub::origin::upsert_origin(conn, &origin))
            .await
    }

    /// Every recorded install, keyed by catalog entry id by the caller.
    pub async fn origins(&self) -> rusqlite::Result<Vec<crate::hub::origin::InstallOrigin>> {
        self.with_conn(|conn| crate::hub::origin::all_origins(conn))
            .await
    }

    /// Forget the install record for an uninstalled backend object, addressed
    /// the way `extensions.uninstall` addresses it.
    pub async fn forget_installed_origin(
        &self,
        kind: ExtensionKind,
        backend: &str,
    ) -> rusqlite::Result<usize> {
        let backend = backend.to_owned();
        self.with_conn(move |conn| crate::hub::origin::forget_installed(conn, kind, &backend))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::TrustTier;

    fn entry(id: &str, cat: ExtensionCategory, name: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind: ExtensionKind::Mcp,
            category: cat,
            name: name.into(),
            description: "d".into(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: "mcp-official".into(),
            repo_url: None,
            trust_tier: TrustTier::Community,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            via: None,
            install_spec: None,
        }
    }

    #[test]
    fn upsert_then_query_by_category() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "Alpha")).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Beta")).unwrap();

        let dev = query_entries(
            &conn,
            &CatalogFilter {
                category: Some(ExtensionCategory::Developer),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(dev.len(), 1);
        assert_eq!(dev[0].name, "Alpha");

        let all = query_entries(&conn, &CatalogFilter::default()).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn upsert_is_idempotent_by_id() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "Alpha")).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "Alpha v2")).unwrap();
        let all = query_entries(&conn, &CatalogFilter::default()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Alpha v2");
    }

    #[test]
    fn query_substring_matches_name() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "GitHub")).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Postgres")).unwrap();
        let hits = query_entries(
            &conn,
            &CatalogFilter {
                query: Some("git".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
    }

    /// A search has to reach the description and tags — that is where an
    /// extension says what it does. Matching only the name meant a model asking
    /// for "issue tracker" found nothing.
    #[test]
    fn query_reaches_description_tags_and_author() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let mut e = entry("a", ExtensionCategory::Developer, "Octo");
        e.description = "Track issues and pull requests.".into();
        e.tags = vec!["vcs".into()];
        e.author = Some("Acme Corp".into());
        upsert_entry(&conn, &e).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Postgres")).unwrap();

        for needle in ["issues", "VCS", "acme"] {
            let hits = query_entries(
                &conn,
                &CatalogFilter {
                    query: Some(needle.into()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(hits.len(), 1, "'{needle}' should match exactly one entry");
            assert_eq!(hits[0].id, "a");
        }
    }

    /// The query must not leak the stored JSON's key names: searching `mcp`
    /// used to be a candidate for matching every row via `"kind":"mcp"`.
    #[test]
    fn query_does_not_match_stored_json_keys() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "Alpha")).unwrap();
        let hits = query_entries(
            &conn,
            &CatalogFilter {
                query: Some("trust_tier".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn query_by_id_returns_exact_entry() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        upsert_entry(&conn, &entry("a", ExtensionCategory::Developer, "Alpha")).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Beta")).unwrap();
        let hits = query_entries(
            &conn,
            &CatalogFilter {
                id: Some("b".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "b");
    }

    #[test]
    fn clear_source_removes_only_that_source() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let mut e = entry("a", ExtensionCategory::Developer, "Alpha");
        e.source_id = "docker-mcp".into();
        upsert_entry(&conn, &e).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Beta")).unwrap(); // mcp-official
        assert_eq!(clear_source(&conn, "mcp-official").unwrap(), 1);
        assert_eq!(
            query_entries(&conn, &CatalogFilter::default())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn count_source_counts_only_that_source() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let mut a = entry("a", ExtensionCategory::Developer, "Alpha");
        a.source_id = "aleph-hub".into();
        upsert_entry(&conn, &a).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Beta")).unwrap(); // mcp-official
        assert_eq!(count_source(&conn, "aleph-hub").unwrap(), 1);
        assert_eq!(count_source(&conn, "nope").unwrap(), 0);
    }

    // --- off-loop SQLite boundary (`with_conn`) ----------------------------
    //
    // All fixtures are `open_in_memory`: no files, no real home, no env, no
    // keyring. The runtime is the `#[tokio::test]` current-thread one, so the
    // test thread is the ONLY runtime worker.

    /// The closure runs on the blocking pool (shape 4: assert the route, not
    /// that a call exists somewhere).
    #[tokio::test]
    async fn with_conn_runs_sqlite_off_the_runtime_thread() {
        let cache = CatalogCache::open_in_memory().unwrap();
        let here = std::thread::current().id();
        let there = cache
            .with_conn(|conn| {
                conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?;
                Ok(std::thread::current().id())
            })
            .await
            .unwrap();
        assert_ne!(here, there, "SQLite closure ran on the runtime thread");
    }

    /// The public async API still round-trips through the boundary.
    #[tokio::test]
    async fn async_api_round_trips_through_the_blocking_boundary() {
        let cache = CatalogCache::open_in_memory().unwrap();
        cache
            .upsert_many(&[
                entry("a", ExtensionCategory::Developer, "Alpha"),
                entry("b", ExtensionCategory::Data, "Beta"),
            ])
            .await
            .unwrap();
        let dev = cache
            .query(&CatalogFilter {
                category: Some(ExtensionCategory::Developer),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(dev.len(), 1);
        assert_eq!(cache.count_source("mcp-official").await.unwrap(), 2);
        // Clear-and-refill: the old rows are gone, only the new one remains.
        cache
            .replace_source(
                "mcp-official",
                &[entry("c", ExtensionCategory::Data, "Gamma")],
            )
            .await
            .unwrap();
        let all = cache.query(&CatalogFilter::default()).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "c");
        assert!(cache.origins().await.unwrap().is_empty());
        assert_eq!(
            cache
                .forget_installed_origin(ExtensionKind::Mcp, "nope")
                .await
                .unwrap(),
            0
        );
    }

    /// While SQLite work is blocked, the single runtime worker keeps polling
    /// other futures (the driver below only runs once the closure has started
    /// AND is still parked), and a second operation queues behind the owned
    /// guard instead of touching the connection. Deterministic: the pass path
    /// is released explicitly; the 30 s `recv_timeout` is only a safety net
    /// that turns a regression (closure pinning the runtime thread, so the
    /// driver could never release it) into a failed assertion, not a hang.
    #[tokio::test]
    async fn blocked_sqlite_work_neither_pins_the_runtime_nor_admits_a_second_writer() {
        use std::sync::mpsc::{channel, TryRecvError};
        let cache = CatalogCache::open_in_memory().unwrap();
        let here = std::thread::current().id();
        let (started_tx, started_rx) = channel::<std::thread::ThreadId>();
        let (release_tx, release_rx) = channel::<()>();

        let first = cache.with_conn(move |_conn| {
            started_tx.send(std::thread::current().id()).unwrap();
            Ok(release_rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .is_ok())
        });

        let driver = async {
            let worker = loop {
                match started_rx.try_recv() {
                    Ok(id) => break id,
                    Err(TryRecvError::Empty) => tokio::task::yield_now().await,
                    Err(TryRecvError::Disconnected) => panic!("closure dropped before start"),
                }
            };
            // The first closure is now running and parked; this future is
            // being polled, so the runtime thread is free.
            let mut second = Box::pin(cache.count_source("mcp-official"));
            tokio::select! {
                biased;
                _ = &mut second => panic!("second op ran while the guard was held"),
                _ = tokio::task::yield_now() => {}
            }
            release_tx.send(()).unwrap();
            (worker, second.await)
        };

        let (released, (worker, second)) = tokio::join!(first, driver);
        assert!(
            released.unwrap(),
            "closure was never released: runtime thread pinned"
        );
        assert_ne!(worker, here);
        assert_eq!(second.unwrap(), 0);
    }

    /// A panic inside the blocking closure re-raises on the caller (never a
    /// default/success), and the connection stays usable afterwards: the owned
    /// guard was released on unwind and the tokio mutex does not poison.
    #[tokio::test]
    async fn with_conn_reraises_a_panic_and_releases_the_guard() {
        let cache = std::sync::Arc::new(CatalogCache::open_in_memory().unwrap());
        let c = std::sync::Arc::clone(&cache);
        let joined = tokio::spawn(async move {
            c.with_conn(|_conn| -> rusqlite::Result<()> { panic!("sqlite boom") })
                .await
        })
        .await;
        let err = joined.expect_err("panic must propagate, not become Ok");
        assert!(err.is_panic());
        assert_eq!(cache.count_source("mcp-official").await.unwrap(), 0);
    }
}
