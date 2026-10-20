//! Markdown-backed raw memories with a metadata-only SQLite work index.
//!
//! Raw bodies are read from Markdown on every lookup, including search.
//! Processing state is authoritative in `raw_memory_index`, not frontmatter.

use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rusqlite::{
    params, params_from_iter, types::Value, Connection, OptionalExtension, TransactionBehavior,
};

use crate::error::AlephError;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

use super::utils::compute_sha256;
use super::MarkdownSessionStore;

fn storage_error(error: impl std::fmt::Display) -> AlephError {
    AlephError::other(format!("Markdown raw memory: {error}"))
}

/// Neither caller-controlled agent IDs nor record IDs become path components.
/// A fresh file UUID also prevents failed writes from clobbering earlier bodies.
fn memory_file(base: &Path, agent: &str, file_id: &str) -> Result<PathBuf, AlephError> {
    let file_id = uuid::Uuid::parse_str(file_id).map_err(storage_error)?;
    Ok(base
        .join("raw/memories")
        .join(compute_sha256(agent.as_bytes()))
        .join(format!("{}.md", file_id.simple())))
}

/// Stable tombstones survive process termination and are never dropped as temps.
fn recovery_file(base: &Path, file_id: &str) -> Result<PathBuf, AlephError> {
    let file_id = uuid::Uuid::parse_str(file_id).map_err(storage_error)?;
    Ok(base
        .join("raw/recovery")
        .join(format!("{}.md", file_id.simple())))
}

fn recover_raw_deletions(db: &mut Connection, base: &Path) -> Result<(), AlephError> {
    // Coordinate with deletion/publication in every store instance, not just
    // this connection. SQLite has already rolled back interrupted transactions.
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let entries = match std::fs::read_dir(base.join("raw/recovery")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage_error(error)),
    };
    for entry in entries {
        let entry = entry.map_err(storage_error)?;
        let backup = entry.path();
        if !entry.file_type().map_err(storage_error)?.is_file()
            || backup.extension().and_then(|ext| ext.to_str()) != Some("md")
        {
            continue;
        }
        let Some(file_id) = backup
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| uuid::Uuid::parse_str(stem).ok())
        else {
            continue;
        };
        let file_id = file_id.to_string();
        let agent: Option<String> = tx
            .query_row(
                "SELECT agent_id FROM raw_memory_index WHERE file_id = ?1",
                [&file_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(agent) = agent {
            let path = memory_file(base, &agent, &file_id)?;
            // Never replace an existing body; the index's owner determines the
            // destination, not caller-controlled IDs or tombstone metadata.
            if !path.try_exists().map_err(storage_error)? {
                std::fs::create_dir_all(
                    path.parent()
                        .ok_or_else(|| storage_error("missing raw directory"))?,
                )
                .map_err(storage_error)?;
                std::fs::rename(&backup, &path).map_err(storage_error)?;
                continue;
            }
        }
        // The deletion committed, or a previous recovery already restored it.
        std::fs::remove_file(&backup).map_err(storage_error)?;
    }
    Ok(())
}

fn write_memory(path: &Path, raw: &RawMemory) -> Result<(), AlephError> {
    // Reuse the wire shape without duplicating RawMemory's metadata schema.
    // Content stays exclusively in the body; mutable work state stays in SQLite.
    let mut metadata = serde_json::to_value(raw).map_err(storage_error)?;
    let fields = metadata
        .as_object_mut()
        .ok_or_else(|| storage_error("invalid raw metadata"))?;
    fields.remove("content");
    fields.remove("is_processed");
    let header = crate::yaml::to_string(&metadata).map_err(storage_error)?;
    let parent = path
        .parent()
        .ok_or_else(|| storage_error("missing raw directory"))?;
    std::fs::create_dir_all(parent).map_err(storage_error)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(storage_error)?;
    write!(temp, "---\n{header}---\n{}", raw.content).map_err(storage_error)?;
    temp.as_file().sync_all().map_err(storage_error)?;
    // Atomic publication, including on Windows; never replace another writer.
    temp.persist_noclobber(path).map_err(storage_error)?;
    Ok(())
}

fn read_memory(
    base: &Path,
    id: &str,
    agent: &str,
    file_id: &str,
    processed: bool,
) -> Result<RawMemory, AlephError> {
    let path = memory_file(base, agent, file_id)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| storage_error(format!("read {}: {e}", path.display())))?;
    let (header, body) = text
        .strip_prefix("---\n")
        .and_then(|text| text.split_once("\n---\n"))
        .ok_or_else(|| storage_error(format!("invalid frontmatter in {}", path.display())))?;
    let mut metadata: serde_json::Value = crate::yaml::from_str(header).map_err(storage_error)?;
    let fields = metadata
        .as_object_mut()
        .ok_or_else(|| storage_error("invalid raw metadata"))?;
    fields.insert("content".into(), serde_json::Value::String(body.into()));
    fields.insert("is_processed".into(), serde_json::Value::Bool(processed));
    let raw: RawMemory = serde_json::from_value(metadata).map_err(storage_error)?;
    if raw.id != id || raw.agent_id != agent {
        return Err(storage_error(format!(
            "identity mismatch in {}",
            path.display()
        )));
    }
    Ok(raw)
}

/// Shared dashboard predicate; bodies never enter the derived work index.
fn dashboard_filter(
    agents: Option<&[String]>,
    query: Option<&str>,
) -> (String, Vec<Value>, Option<String>) {
    let mut predicate = "source != 'tool_invocation'".to_string();
    let mut values = Vec::new();
    if let Some(agents) = agents {
        predicate.push_str(" AND agent_id IN (SELECT value FROM json_each(?1))");
        values.push(
            serde_json::to_string(agents)
                .expect("string slice serialization")
                .into(),
        );
    }
    let needle = query
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_ascii_lowercase);
    (predicate, values, needle)
}

impl MarkdownSessionStore {
    pub(super) async fn recover_raw_deletions(&self) -> Result<(), AlephError> {
        self.with_raw_db(recover_raw_deletions).await
    }

    /// Keep synchronous SQLite/filesystem I/O off the async executor. A write's
    /// transaction and file publication finish together even if its caller drops.
    async fn with_raw_db<T, F>(&self, operation: F) -> Result<T, AlephError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection, &Path) -> Result<T, AlephError> + Send + 'static,
    {
        let db = self.db.clone();
        let base = self.base_dir.clone();
        tokio::task::spawn_blocking(move || operation(&mut db.blocking_lock(), &base))
            .await
            .map_err(storage_error)?
    }

    async fn insert_markdown_raw(&self, raw: &RawMemory, ignore: bool) -> Result<(), AlephError> {
        let raw = raw.clone();
        self.with_raw_db(move |db, base| {
            // Serializes first-writer-wins across independent store instances,
            // not just tasks sharing the same in-process mutex.
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage_error)?;
            let file_id = uuid::Uuid::new_v4().to_string();
            let sql = if ignore {
                "INSERT OR IGNORE INTO raw_memory_index
                 (id, file_id, agent_id, session_id, path, source, is_processed, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            } else {
                "INSERT INTO raw_memory_index
                 (id, file_id, agent_id, session_id, path, source, is_processed, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            };
            let inserted = tx
                .execute(
                    sql,
                    params![
                        raw.id,
                        file_id,
                        raw.agent_id,
                        raw.session_id,
                        raw.path,
                        raw.source.as_str(),
                        raw.is_processed,
                        raw.created_at
                    ],
                )
                .map_err(storage_error)?;
            if inserted == 0 {
                return Ok(());
            }
            let path = memory_file(base, &raw.agent_id, &file_id)?;
            write_memory(&path, &raw)?;
            // Only publish the index after the complete Markdown file exists.
            // A crash/commit failure can leave an unindexed file, never a
            // committed reference to a partially written body. Orphan recovery
            // and index rebuild are intentionally outside this store phase.
            tx.commit().map_err(storage_error)
        })
        .await
    }

    /// Predicates are internal SQL constants; all caller input is bound data.
    /// Search streams newest-first files until enough actual body matches exist,
    /// rather than filtering an oldest-first, already-limited candidate window.
    async fn query_markdown_raw(
        &self,
        predicate: &str,
        mut values: Vec<Value>,
        newest_first: bool,
        limit: usize,
        needle: Option<String>,
    ) -> Result<Vec<RawMemory>, AlephError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let order = if newest_first { "DESC" } else { "ASC" };
        let mut sql = format!("SELECT id, agent_id, file_id, is_processed FROM raw_memory_index WHERE {predicate} ORDER BY created_at {order}, rowid {order}");
        if needle.is_none() {
            sql.push_str(" LIMIT ?");
            values.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        }
        self.with_raw_db(move |db, base| {
            // Serialize indexed file reads against deletion in other store instances.
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage_error)?;
            let mut stmt = tx.prepare(&sql).map_err(storage_error)?;
            let mut rows = stmt
                .query(params_from_iter(values))
                .map_err(storage_error)?;
            let mut results = Vec::new();
            while let Some(row) = rows.next().map_err(storage_error)? {
                let id: String = row.get(0).map_err(storage_error)?;
                let agent: String = row.get(1).map_err(storage_error)?;
                let file_id: String = row.get(2).map_err(storage_error)?;
                let processed: bool = row.get(3).map_err(storage_error)?;
                let raw = read_memory(base, &id, &agent, &file_id, processed)?;
                if needle
                    .as_ref()
                    .is_some_and(|needle| !raw.content.to_ascii_lowercase().contains(needle))
                {
                    continue;
                }
                results.push(raw);
                if results.len() == limit {
                    break;
                }
            }
            Ok(results)
        })
        .await
    }
}

#[async_trait]
impl RawMemoryStore for MarkdownSessionStore {
    async fn raw_dashboard(
        &self,
        agents: Option<&[String]>,
        query: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let (predicate, values, needle) = dashboard_filter(agents, query);
        self.with_raw_db(move |db, base| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage_error)?;
            // Match the historical newest-first dashboard. Equal timestamps
            // keep insertion order, as the old table scan did.
            let sql = format!("SELECT id, agent_id, file_id, is_processed FROM raw_memory_index WHERE {predicate} ORDER BY created_at DESC, rowid ASC");
            let mut stmt = tx.prepare(&sql).map_err(storage_error)?;
            let mut rows = stmt.query(params_from_iter(values)).map_err(storage_error)?;
            let mut results = Vec::new();
            let mut skipped = 0;
            while let Some(row) = rows.next().map_err(storage_error)? {
                let raw = read_memory(base, &row.get::<_, String>(0).map_err(storage_error)?, &row.get::<_, String>(1).map_err(storage_error)?, &row.get::<_, String>(2).map_err(storage_error)?, row.get(3).map_err(storage_error)?)?;
                if needle.as_ref().is_some_and(|q| !raw.content.to_ascii_lowercase().contains(q)) { continue; }
                if skipped < offset { skipped += 1; continue; }
                results.push(raw);
                if results.len() == limit { break; }
            }
            Ok(results)
        }).await
    }

    async fn raw_dashboard_count(
        &self,
        agents: Option<&[String]>,
        query: Option<&str>,
    ) -> Result<i64, AlephError> {
        let (predicate, values, needle) = dashboard_filter(agents, query);
        self.with_raw_db(move |db, base| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage_error)?;
            let Some(needle) = needle else {
                return tx.query_row(&format!("SELECT COUNT(*) FROM raw_memory_index WHERE {predicate}"), params_from_iter(values), |r| r.get(0)).map_err(storage_error);
            };
            let mut stmt = tx.prepare(&format!("SELECT id, agent_id, file_id, is_processed FROM raw_memory_index WHERE {predicate}")).map_err(storage_error)?;
            let mut rows = stmt.query(params_from_iter(values)).map_err(storage_error)?;
            let mut count = 0;
            while let Some(row) = rows.next().map_err(storage_error)? {
                let raw = read_memory(base, &row.get::<_, String>(0).map_err(storage_error)?, &row.get::<_, String>(1).map_err(storage_error)?, &row.get::<_, String>(2).map_err(storage_error)?, row.get(3).map_err(storage_error)?)?;
                if raw.content.to_ascii_lowercase().contains(&needle) { count += 1; }
            }
            Ok(count)
        }).await
    }

    async fn raw_owner(&self, id: &str) -> Result<Option<String>, AlephError> {
        let id = id.to_owned();
        self.with_raw_db(move |db, _| {
            db.query_row(
                "SELECT agent_id FROM raw_memory_index WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)
        })
        .await
    }

    async fn remove_raw(&self, id: &str, agent_id: &str) -> Result<bool, AlephError> {
        let id = id.to_owned();
        let agent = agent_id.to_owned();
        self.with_raw_db(move |db, base| {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage_error)?;
            let file_id: Option<String> = tx
                .query_row(
                    "SELECT file_id FROM raw_memory_index WHERE id = ?1 AND agent_id = ?2",
                    params![id, agent],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage_error)?;
            let Some(file_id) = file_id else {
                return Ok(false);
            };
            let path = memory_file(base, &agent, &file_id)?;
            let backup = recovery_file(base, &file_id)?;
            std::fs::create_dir_all(base.join("raw/recovery")).map_err(storage_error)?;
            // Never overwrite a pending backup from an earlier interrupted delete.
            if backup.try_exists().map_err(storage_error)? {
                return Err(storage_error("raw deletion recovery pending; reopen store"));
            }
            tx.execute(
                "DELETE FROM raw_memory_index WHERE id = ?1 AND agent_id = ?2",
                params![id, agent],
            )
            .map_err(storage_error)?;
            // Atomic move on the same filesystem: before commit, the index
            // still owns this complete body; after commit it is safe to discard.
            std::fs::rename(&path, &backup).map_err(storage_error)?;
            if let Err(error) = tx.commit() {
                // Reacquire SQLite's write lock before restoration/cleanup, using
                // the actual committed index state. A failed restore leaves the
                // stable tombstone available for the next initialization.
                recover_raw_deletions(db, base).map_err(|restore| {
                    storage_error(format!(
                        "commit failed: {error}; file restoration failed: {restore}"
                    ))
                })?;
                return Err(storage_error(error));
            }
            if let Err(error) = std::fs::remove_file(&backup) {
                // Another initializer may already have cleaned this tombstone.
                // Other cleanup errors are retryable at startup, not a failed
                // deletion: the index removal has already committed.
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %backup.display(), %error, "Raw deletion cleanup deferred");
                }
            }
            Ok(true)
        })
        .await
    }

    async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError> {
        self.insert_markdown_raw(raw, false).await
    }

    async fn insert_raw_memory_or_ignore(&self, raw: &RawMemory) -> Result<(), AlephError> {
        self.insert_markdown_raw(raw, true).await
    }

    async fn get_unprocessed_raw_memories(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        self.query_markdown_raw(
            "agent_id = ?1 AND is_processed = 0",
            vec![agent_id.to_owned().into()],
            false,
            limit,
            None,
        )
        .await
    }

    async fn mark_raw_as_processed(&self, ids: &[String]) -> Result<usize, AlephError> {
        if ids.is_empty() {
            return Ok(0);
        }
        // json_each avoids SQLite's host-parameter cap and deduplicates IDs
        // within one atomic UPDATE, even for large compression batches.
        let ids = serde_json::to_string(ids).map_err(storage_error)?;
        self.with_raw_db(move |db, _| {
            db.execute("UPDATE raw_memory_index SET is_processed = 1 WHERE id IN (SELECT value FROM json_each(?1))", [ids]).map_err(storage_error)
        }).await
    }

    async fn count_unprocessed(&self, agent_id: &str) -> Result<usize, AlephError> {
        let agent = agent_id.to_owned();
        self.with_raw_db(move |db, _| {
            db.query_row(
                "SELECT count(*) FROM raw_memory_index WHERE agent_id = ?1 AND is_processed = 0",
                [agent],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n as usize)
            .map_err(storage_error)
        })
        .await
    }

    async fn unprocessed_agent_ids(&self) -> Result<Vec<String>, AlephError> {
        self.with_raw_db(|db, _| {
            let mut stmt = db.prepare("SELECT DISTINCT agent_id FROM raw_memory_index WHERE is_processed = 0 ORDER BY agent_id ASC").map_err(storage_error)?;
            let rows = stmt.query_map([], |row| row.get(0)).map_err(storage_error)?;
            rows.collect::<Result<Vec<String>, _>>().map_err(storage_error)
        }).await
    }

    async fn get_raw_by_path_prefix(
        &self,
        path_prefix: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        // substr + NOCASE matches SQLite LIKE's ASCII folding without giving
        // percent, underscore, or backslash any wildcard meaning.
        self.query_markdown_raw(
            "agent_id = ?1 AND substr(path, 1, length(?2)) = ?2 COLLATE NOCASE",
            vec![agent_id.to_owned().into(), path_prefix.to_owned().into()],
            false,
            limit,
            None,
        )
        .await
    }

    async fn get_raw_by_path_prefix_since(
        &self,
        path_prefix: &str,
        agent_id: &str,
        since_created_at: i64,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        self.query_markdown_raw(
            "agent_id = ?1 AND substr(path, 1, length(?2)) = ?2 COLLATE NOCASE AND created_at > ?3",
            vec![
                agent_id.to_owned().into(),
                path_prefix.to_owned().into(),
                since_created_at.into(),
            ],
            false,
            limit,
            None,
        )
        .await
    }

    async fn search_raw_by_path_prefix(
        &self,
        path_prefix: &str,
        agent_id: &str,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        self.query_markdown_raw(
            "agent_id = ?1 AND substr(path, 1, length(?2)) = ?2 COLLATE NOCASE",
            vec![agent_id.to_owned().into(), path_prefix.to_owned().into()],
            true,
            limit,
            Some(needle.to_ascii_lowercase()),
        )
        .await
    }

    async fn find_by_path(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Option<RawMemory>, AlephError> {
        Ok(self
            .query_markdown_raw(
                "agent_id = ?1 AND path = ?2",
                vec![agent_id.to_owned().into(), path.to_owned().into()],
                false,
                1,
                None,
            )
            .await?
            .into_iter()
            .next())
    }

    async fn get_raws_by_ids(
        &self,
        agent_id: &str,
        ids: &[String],
    ) -> Result<Vec<RawMemory>, AlephError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.query_markdown_raw(
            "agent_id = ?1 AND id IN (SELECT value FROM json_each(?2))",
            vec![
                agent_id.to_owned().into(),
                serde_json::to_string(ids).map_err(storage_error)?.into(),
            ],
            false,
            usize::MAX,
            None,
        )
        .await
    }

    async fn get_raws_by_session(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<Vec<RawMemory>, AlephError> {
        self.query_markdown_raw(
            "agent_id = ?1 AND session_id = ?2",
            vec![agent_id.to_owned().into(), session_id.to_owned().into()],
            false,
            usize::MAX,
            None,
        )
        .await
    }

    async fn get_raw_by_source(
        &self,
        source: RawMemorySource,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        // Source queries historically match the discriminator, not its detail.
        self.query_markdown_raw(
            "agent_id = ?1 AND source = ?2",
            vec![
                agent_id.to_owned().into(),
                source.as_str().to_owned().into(),
            ],
            true,
            limit,
            None,
        )
        .await
    }
}
