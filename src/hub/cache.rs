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
pub fn matches_query(e: &ExtensionEntry, query: &str) -> bool {
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
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
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

pub fn upsert_entry(conn: &Connection, e: &ExtensionEntry) -> rusqlite::Result<()> {
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

pub fn query_entries(
    conn: &Connection,
    f: &CatalogFilter,
) -> rusqlite::Result<Vec<ExtensionEntry>> {
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

pub fn clear_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM catalog WHERE source_id = ?1",
        params![source_id],
    )
}

pub fn count_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
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
    pub async fn upsert_many(&self, entries: &[ExtensionEntry]) -> rusqlite::Result<()> {
        let mut guard = self.conn.lock().await;
        // Wrap the whole batch so any per-row failure rolls back every prior
        // insert in the same call. (See `replace_source` for the slot-level
        // equivalent that adds the clear-and-refill semantics.)
        let tx = guard.transaction()?;
        for e in entries {
            upsert_entry(&tx, e)?;
        }
        tx.commit()
    }
    pub async fn query(&self, f: &CatalogFilter) -> rusqlite::Result<Vec<ExtensionEntry>> {
        let guard = self.conn.lock().await;
        query_entries(&guard, f)
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
        let mut guard = self.conn.lock().await;
        let tx = guard.transaction()?;
        clear_source(&tx, source_id)?;
        for e in entries {
            upsert_entry(&tx, e)?;
        }
        tx.commit()
    }
    /// Number of cached rows for a source. Used by the cold-start primer.
    pub async fn count_source(&self, source_id: &str) -> rusqlite::Result<usize> {
        let guard = self.conn.lock().await;
        count_source(&guard, source_id)
    }

    // --- install provenance ledger (see `hub::origin`) ---------------------

    /// Record one completed Hub install. Idempotent by catalog entry id.
    pub async fn record_origin(
        &self,
        origin: &crate::hub::origin::InstallOrigin,
    ) -> rusqlite::Result<()> {
        let guard = self.conn.lock().await;
        crate::hub::origin::upsert_origin(&guard, origin)
    }

    /// Every recorded install, keyed by catalog entry id by the caller.
    pub async fn origins(&self) -> rusqlite::Result<Vec<crate::hub::origin::InstallOrigin>> {
        let guard = self.conn.lock().await;
        crate::hub::origin::all_origins(&guard)
    }

    /// Forget the install record for an uninstalled backend object, addressed
    /// the way `extensions.uninstall` addresses it.
    pub async fn forget_installed_origin(
        &self,
        kind: ExtensionKind,
        backend: &str,
    ) -> rusqlite::Result<usize> {
        let guard = self.conn.lock().await;
        crate::hub::origin::forget_installed(&guard, kind, backend)
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
}
