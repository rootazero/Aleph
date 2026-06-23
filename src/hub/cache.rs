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
    pub query: Option<String>,
}

/// Schema: one row per extension; `data` holds the full JSON, indexed columns
/// drive filtering. `name_lc` enables case-insensitive substring search.
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS catalog (
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
    )
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
            serde_json::to_value(e.kind).unwrap().as_str().unwrap(),
            serde_json::to_value(e.category).unwrap().as_str().unwrap(),
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
        args.push(Box::new(
            serde_json::to_value(k)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
        ));
    }
    if let Some(c) = f.category {
        sql.push_str(" AND category = ?");
        args.push(Box::new(
            serde_json::to_value(c)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
        ));
    }
    if let Some(s) = &f.source_id {
        sql.push_str(" AND source_id = ?");
        args.push(Box::new(s.clone()));
    }
    if let Some(q) = &f.query {
        sql.push_str(" AND name_lc LIKE ?");
        args.push(Box::new(format!("%{}%", q.to_lowercase())));
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
    rows.collect()
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
        let guard = self.conn.lock().await;
        for e in entries {
            upsert_entry(&guard, e)?;
        }
        Ok(())
    }
    pub async fn query(&self, f: &CatalogFilter) -> rusqlite::Result<Vec<ExtensionEntry>> {
        let guard = self.conn.lock().await;
        query_entries(&guard, f)
    }
    /// Atomic per-source refresh: clear the source's rows then insert fresh.
    pub async fn replace_source(
        &self,
        source_id: &str,
        entries: &[ExtensionEntry],
    ) -> rusqlite::Result<()> {
        let guard = self.conn.lock().await;
        clear_source(&guard, source_id)?;
        for e in entries {
            upsert_entry(&guard, e)?;
        }
        Ok(())
    }
    /// Number of cached rows for a source. Used by the cold-start primer.
    pub async fn count_source(&self, source_id: &str) -> rusqlite::Result<usize> {
        let guard = self.conn.lock().await;
        count_source(&guard, source_id)
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
