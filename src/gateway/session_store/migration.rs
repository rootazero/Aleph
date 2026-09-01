use std::path::Path;

use rusqlite::Connection;
use tracing::{debug, info};

use crate::gateway::session_store::file_backend::FileSessionStore;
use crate::gateway::session_store::types::{MessageRecord, SessionMetadata};

const MIGRATION_MARKER: &str = ".migrated_from_sqlite";

/// Check whether a legacy `SQLite` sessions database exists and has not yet been migrated.
#[must_use]
pub fn migration_needed(base_dir: &Path) -> bool {
    let marker = base_dir.join(MIGRATION_MARKER);
    if marker.exists() {
        return false;
    }
    let legacy_db = crate::utils::paths::get_sessions_db_path()
        .unwrap_or_else(|_| std::env::temp_dir().join("aleph_data").join("sessions.db"));
    legacy_db.exists()
}

/// Heal on-disk session directory names that don't match this platform's
/// `session_dir(key)` form, renaming each to the canonical name.
///
/// The file backend names every session dir by sanitizing its key
/// ([`crate::gateway::session_store::file_backend::sanitize_key_for_dir`]).
/// macOS keeps `:`; Windows replaces `:*?"<>|` with `_`. Copying a `~/.aleph`
/// data dir across platforms (or a transfer tool transliterating chars illegal
/// on the target FS) leaves dirs whose names no longer resolve here, so history
/// lookups miss — even though the session *list* still renders (it reads dir
/// names + `metadata.json` directly). This pass reads each session's canonical
/// `key` from its `metadata.json` and renames the dir to the expected name.
///
/// Best-effort and idempotent: dirs already canonical, dirs without a readable
/// `metadata.json` (`.archive`, channel scratch dirs, …), and renames whose
/// target already exists are all left untouched. All errors are logged, never
/// returned — a migration hiccup must never block server startup. Returns the
/// number of directories renamed.
pub async fn normalize_session_dir_names(base_dir: &Path) -> usize {
    /// Minimal view of `metadata.json` — only the canonical key is needed, and
    /// a narrow struct tolerates schema drift in the rest of the file.
    #[derive(serde::Deserialize)]
    struct KeyOnly {
        key: String,
    }

    let mut entries = match tokio::fs::read_dir(base_dir).await {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut renamed = 0usize;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let path = entry.path();
        let contents = match tokio::fs::read_to_string(path.join("metadata.json")).await {
            Ok(c) => c,
            // No metadata.json ⇒ not a normal session dir; leave it.
            Err(_) => continue,
        };
        let key = match serde_json::from_str::<KeyOnly>(&contents) {
            Ok(k) if !k.key.is_empty() => k.key,
            _ => continue,
        };
        let expected = crate::gateway::session_store::file_backend::sanitize_key_for_dir(&key);
        let current = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if current == expected {
            continue;
        }
        let target = base_dir.join(&expected);
        if target.exists() {
            info!(
                from = %current,
                to = %expected,
                "Session dir normalize: target already exists, skipping to avoid clobber"
            );
            continue;
        }
        match tokio::fs::rename(&path, &target).await {
            Ok(()) => {
                renamed += 1;
                debug!(from = %current, to = %expected, "Normalized session dir name");
            }
            Err(e) => {
                info!(from = %current, to = %expected, error = %e, "Session dir normalize failed");
            }
        }
    }

    if renamed > 0 {
        info!(
            renamed,
            "Normalized {renamed} migrated session directory name(s)"
        );
    }
    renamed
}

/// Export legacy `SQLite` sessions and messages into the file backend.
/// Writes a marker file on success so the migration is not re-run.
///
/// Resolves the legacy DB path from `get_sessions_db_path()` and delegates to
/// [`export_legacy_messages_from`]. Integration tests should call the `_from`
/// variant directly with an explicit fixture path.
pub async fn export_legacy_messages(
    store: &FileSessionStore,
) -> Result<usize, crate::error::AlephError> {
    let legacy_db = crate::utils::paths::get_sessions_db_path().map_err(|e| {
        crate::error::AlephError::ConfigError {
            message: format!("Failed to resolve legacy DB path: {e}"),
            suggestion: None,
        }
    })?;
    export_legacy_messages_from(&legacy_db, store).await
}

/// Migrate sessions/messages from an explicit legacy `SQLite` path into `store`.
/// Split out from [`export_legacy_messages`] so integration tests can drive
/// migration against fixture databases without depending on `$HOME`.
pub async fn export_legacy_messages_from(
    legacy_db: &Path,
    store: &FileSessionStore,
) -> Result<usize, crate::error::AlephError> {
    if !legacy_db.exists() {
        info!(
            "No legacy SQLite session database found at {:?}; skipping migration.",
            legacy_db
        );
        return Ok(0);
    }

    info!("Starting session migration from {:?} ...", legacy_db);

    let conn = crate::utils::sqlite_open::open_sqlite_safe(legacy_db).map_err(|e| {
        crate::error::AlephError::ConfigError {
            message: format!("Failed to open legacy session DB: {e}"),
            suggestion: None,
        }
    })?;

    // Defensive: older legacy DBs may be missing columns the SELECT below
    // expects. PRAGMA-guard each ADD COLUMN so the migration succeeds against
    // any historical schema. SQLite's ALTER TABLE ADD COLUMN is fast, has no
    // table-rewrite cost, and is idempotent here because we check first.
    ensure_sessions_columns(&conn)?;

    // -----------------------------------------------------------------------
    // 1. Migrate sessions -> metadata.json
    // -----------------------------------------------------------------------
    let mut stmt = conn
        .prepare(
            "SELECT key, agent_id, session_type, created_at, last_active_at, message_count, total_tokens, auto_reset_at, state, metadata, label, input_tokens, output_tokens, model, model_provider, parent_session_key, compaction_count FROM sessions",
        )
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Prepare failed: {e}"),
            suggestion: None,
        })?;

    let session_rows = stmt
        .query_map([], |row| {
            let state_str: Option<String> = row.get(8)?;
            let state = state_str
                .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok())
                .unwrap_or_default();
            let metadata_json: Option<String> = row.get(9)?;
            let (topic, status, identity_meta) =
                SessionMetadata::parse_legacy_metadata_json(metadata_json.as_deref());
            Ok(SessionMetadata {
                key: row.get(0)?,
                agent_id: row.get(1)?,
                session_type: row.get(2)?,
                created_at: row.get(3)?,
                last_active_at: row.get(4)?,
                message_count: row.get(5)?,
                total_tokens: row.get(6)?,
                auto_reset_at: row.get(7)?,
                state: Some(state),
                topic,
                status,
                identity_meta,
                label: row.get(10)?,
                // Legacy rows: ALTER TABLE ADD COLUMN without DEFAULT leaves
                // NULL for pre-migration data. Coerce to 0 so the SQLite →
                // file-based migration doesn't abort on the first legacy row.
                input_tokens: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
                output_tokens: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
                model: row.get(13)?,
                model_provider: row.get(14)?,
                parent_session_key: row.get(15)?,
                // Same NULL coercion as input_tokens/output_tokens above:
                // legacy ALTER TABLE ADD COLUMN without DEFAULT left NULL for
                // pre-migration rows. Coerce to 0 so a single legacy row
                // doesn't abort the entire SQLite → file-based migration.
                compaction_count: row.get::<_, Option<i64>>(16)?.unwrap_or(0),
                ..Default::default()
            })
        })
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Query failed: {e}"),
            suggestion: None,
        })?;

    let mut migrated_count = 0usize;
    for meta in session_rows {
        let meta = meta.map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Row error: {e}"),
            suggestion: None,
        })?;
        let key_str = meta.key.clone();
        let mut guard = store.lock_metadata(&key_str).await.map_err(|e| {
            crate::error::AlephError::ConfigError {
                message: format!("Lock metadata failed for {key_str}: {e}"),
                suggestion: None,
            }
        })?;
        guard.insert(meta);
        guard
            .commit()
            .await
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("Write metadata failed for {key_str}: {e}"),
                suggestion: None,
            })?;
        migrated_count += 1;
    }
    drop(stmt);

    // -----------------------------------------------------------------------
    // 2. Migrate messages -> transcript.jsonl
    // -----------------------------------------------------------------------
    let mut msg_stmt = conn
        .prepare(
            "SELECT session_key, role, content, timestamp, metadata, input_tokens, output_tokens FROM messages ORDER BY id ASC",
        )
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Prepare messages failed: {e}"),
            suggestion: None,
        })?;

    let mut current_key: Option<String> = None;
    let mut current_batch: Vec<MessageRecord> = Vec::new();
    let mut total_messages = 0usize;

    let message_rows = msg_stmt
        .query_map([], |row| {
            let metadata_str: Option<String> = row.get(4)?;
            let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
            Ok((
                row.get::<usize, String>(0)?,
                MessageRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    role: row.get(1)?,
                    content: row.get(2)?,
                    timestamp: row.get(3)?,
                    metadata,
                    // Same NULL coercion as the sessions table — legacy
                    // messages may have NULL token columns.
                    input_tokens: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    output_tokens: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                    tool_call_id: None,
                    tool_name: None,
                },
            ))
        })
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Query messages failed: {e}"),
            suggestion: None,
        })?;

    for row in message_rows {
        let (key, msg) = row.map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Message row error: {e}"),
            suggestion: None,
        })?;

        if current_key.as_ref() != Some(&key) {
            if let Some(ref k) = current_key {
                if !current_batch.is_empty() {
                    write_batch(store, k, &current_batch).await?;
                    total_messages += current_batch.len();
                }
            }
            current_key = Some(key);
            current_batch.clear();
        }
        current_batch.push(msg);
    }

    if let Some(ref k) = current_key {
        if !current_batch.is_empty() {
            write_batch(store, k, &current_batch).await?;
            total_messages += current_batch.len();
        }
    }
    drop(msg_stmt);

    // -----------------------------------------------------------------------
    // 3. Post-migration: re-derive titles and previews from transcripts
    // -----------------------------------------------------------------------
    let keys: Vec<String> = conn
        .prepare("SELECT key FROM sessions")
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Prepare post-migration failed: {e}"),
            suggestion: None,
        })?
        .query_map([], |row| row.get::<usize, String>(0))
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Query post-migration failed: {e}"),
            suggestion: None,
        })?
        .filter_map(|r| r.ok())
        .collect();

    for key in keys {
        let Ok(mut guard) = store.lock_metadata(&key).await else {
            continue;
        };
        if let Some(meta) = guard.existing_mut() {
            if let Ok(messages) = store.read_transcript(&key, None).await {
                for msg in &messages {
                    if meta.derived_title.is_none() && msg.role == "user" {
                        let title = msg.content.trim();
                        let title = if title.chars().count() > 60 {
                            title.chars().take(60).collect::<String>() + "..."
                        } else {
                            title.to_string()
                        };
                        if !title.is_empty() {
                            meta.derived_title = Some(title);
                        }
                    }
                }
                if let Some(last) = messages.last() {
                    let preview = last.content.trim();
                    meta.last_message_preview = Some(if preview.chars().count() > 120 {
                        preview.chars().take(120).collect::<String>() + "..."
                    } else {
                        preview.to_string()
                    });
                }
                meta.message_count = messages.len() as i64;
                let _ = guard.commit().await;
            }
        }
    }

    // -----------------------------------------------------------------------
    // 4. Write marker
    // -----------------------------------------------------------------------
    let marker = store.config().base_dir.join(MIGRATION_MARKER);
    let marker_content = format!(
        "migrated_at={}\nsessions={}\nmessages={}",
        chrono::Utc::now().to_rfc3339(),
        migrated_count,
        total_messages
    );
    tokio::fs::write(&marker, marker_content)
        .await
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Failed to write migration marker: {e}"),
            suggestion: None,
        })?;

    info!(
        sessions = migrated_count,
        messages = total_messages,
        "Session migration from SQLite to file backend completed successfully"
    );

    Ok(total_messages)
}

/// Add columns the migration SELECT depends on, if missing. PRAGMA-guarded
/// per column so this is safe to run on any historical legacy schema.
fn ensure_sessions_columns(conn: &Connection) -> Result<(), crate::error::AlephError> {
    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if table_exists == 0 {
        return Ok(());
    }

    let existing: std::collections::HashSet<String> = conn
        .prepare("PRAGMA table_info(sessions)")
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("PRAGMA table_info(sessions) prepare failed: {e}"),
            suggestion: None,
        })?
        .query_map([], |row| row.get::<usize, String>(1))
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("PRAGMA table_info(sessions) query failed: {e}"),
            suggestion: None,
        })?
        .filter_map(|r| r.ok())
        .collect();

    let needed: &[(&str, &str)] = &[
        ("auto_reset_at", "INTEGER"),
        ("state", "TEXT"),
        ("metadata", "TEXT"),
        ("label", "TEXT"),
        ("input_tokens", "INTEGER"),
        ("output_tokens", "INTEGER"),
        ("model", "TEXT"),
        ("model_provider", "TEXT"),
        ("parent_session_key", "TEXT"),
        // DEFAULT 0 here means SQLite backfills existing rows with 0 instead
        // of NULL when this column is added by ALTER TABLE on a legacy DB.
        // Required because session_metadata.compaction_count is i64 (not
        // Option<i64>); without the default, the migration SELECT fails on
        // every legacy row.
        ("compaction_count", "INTEGER NOT NULL DEFAULT 0"),
    ];
    for (col, ty) in needed {
        if !existing.contains(*col) {
            let sql = format!("ALTER TABLE sessions ADD COLUMN {col} {ty}");
            conn.execute(&sql, [])
                .map_err(|e| crate::error::AlephError::ConfigError {
                    message: format!("ALTER TABLE sessions ADD COLUMN {col} failed: {e}"),
                    suggestion: None,
                })?;
        }
    }
    Ok(())
}

async fn write_batch(
    store: &FileSessionStore,
    key: &str,
    messages: &[MessageRecord],
) -> Result<(), crate::error::AlephError> {
    for msg in messages {
        store.append_transcript(key, msg).await.map_err(|e| {
            crate::error::AlephError::ConfigError {
                message: format!("Append transcript failed for {key}: {e}"),
                suggestion: None,
            }
        })?;
    }
    Ok(())
}

/// Outcome summary of [`repair_session_metadata`], surfaced in the startup log
/// and asserted on in tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MetadataRepairReport {
    /// `last_active_at` values found in milliseconds (the pre-2026-08-09 raw
    /// `msg.timestamp` assignment) and rewritten in seconds.
    pub timestamps_normalized: usize,
    /// Unparseable `metadata.json` documents rebuilt from their intact
    /// `transcript.jsonl` (the pre-2026-08-12 torn-write damage).
    pub rebuilt_from_transcript: usize,
    /// Unparseable documents with no usable transcript — renamed to
    /// `metadata.json.corrupt` so the listing stops warning on every poll.
    /// The conversation was already invisible; quarantine changes nothing the
    /// user can see and keeps the evidence on disk.
    pub quarantined: usize,
}

impl MetadataRepairReport {
    /// Anything to report? The startup log line is emitted only when true — a
    /// clean install stays silent.
    #[must_use]
    pub fn did_work(self) -> bool {
        self.timestamps_normalized + self.rebuilt_from_transcript + self.quarantined > 0
    }
}

/// Repair the two classes of on-disk damage left behind by already-fixed
/// writer bugs. The writers were fixed (seconds via `msg.instant()` on
/// 2026-08-09; atomic write + per-key lock on 2026-08-12), but neither fix
/// touched the damage already persisted, so:
///
/// 1. **Millisecond `last_active_at`** — 1.78e12 sorts numerically above every
///    seconds value (1.78e9) forever, so `list_sessions`' descending sort pins
///    every legacy row above every new conversation. From the user's seat,
///    new sessions "never appear in the sidebar". Normalized in place.
/// 2. **Torn `metadata.json`** — a hybrid of two documents parses as nothing,
///    and `list_sessions` skips it, so the conversation vanishes from every
///    surface while its transcript sits intact beside it. Rebuilt from that
///    transcript; the fields a transcript cannot answer (token counters,
///    model, cost) stay at their defaults rather than being invented.
///
/// Best-effort and idempotent, like [`normalize_session_dir_names`]: every
/// error is logged and swallowed, a repaired file is byte-stable on the next
/// run, and the pass never blocks startup.
pub async fn repair_session_metadata(base_dir: &Path) -> MetadataRepairReport {
    let mut report = MetadataRepairReport::default();
    let mut entries = match tokio::fs::read_dir(base_dir).await {
        Ok(e) => e,
        Err(_) => return report,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let dir = entry.path();
        let meta_path = dir.join("metadata.json");
        let contents = match tokio::fs::read_to_string(&meta_path).await {
            Ok(c) => c,
            Err(_) => continue,
        };
        let dir_name = match dir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        match serde_json::from_str::<SessionMetadata>(&contents) {
            Ok(mut meta) => {
                // Only `last_active_at` was ever assigned a raw millisecond
                // value (`= msg.timestamp`); `created_at` took `timestamp()`
                // on the same path. One field, one fix — do not widen this to
                // other i64 columns without an on-disk specimen proving they
                // carry ms too.
                if meta.last_active_at.abs()
                    >= crate::gateway::session_store::types::SECONDS_MILLIS_BOUNDARY
                {
                    meta.last_active_at /= 1000;
                    match persist_metadata(&meta_path, &meta).await {
                        Ok(()) => report.timestamps_normalized += 1,
                        Err(e) => info!(
                            path = %meta_path.display(),
                            error = %e,
                            "Metadata repair: timestamp normalization write failed"
                        ),
                    }
                }
            }
            Err(_) => {
                match rebuild_metadata_from_transcript(&dir, &dir_name).await {
                    Ok(Some(meta)) => match persist_metadata(&meta_path, &meta).await {
                        Ok(()) => {
                            report.rebuilt_from_transcript += 1;
                            info!(
                                path = %meta_path.display(),
                                "Metadata repair: rebuilt torn metadata.json from transcript"
                            );
                        }
                        Err(e) => info!(
                            path = %meta_path.display(),
                            error = %e,
                            "Metadata repair: rebuild write failed"
                        ),
                    },
                    // No usable transcript — nothing to rebuild from.
                    // Quarantine so `list_sessions` stops warning about this
                    // file on every single poll.
                    Ok(None) => {
                        let quarantine = dir.join("metadata.json.corrupt");
                        if !quarantine.exists()
                            && tokio::fs::rename(&meta_path, &quarantine).await.is_ok()
                        {
                            report.quarantined += 1;
                            info!(
                                path = %meta_path.display(),
                                "Metadata repair: no transcript to rebuild from; \
                                 quarantined as metadata.json.corrupt"
                            );
                        }
                    }
                    Err(e) => info!(
                        path = %meta_path.display(),
                        error = %e,
                        "Metadata repair: transcript read failed"
                    ),
                }
            }
        }
    }

    if report.did_work() {
        info!(
            timestamps_normalized = report.timestamps_normalized,
            rebuilt_from_transcript = report.rebuilt_from_transcript,
            quarantined = report.quarantined,
            "Repaired on-disk session metadata left by pre-fix writer bugs"
        );
    }
    report
}

/// Write a repaired document through the same atomic temp+rename the live
/// writers use — a repair that itself tore a write would recreate the exact
/// damage it exists to heal.
async fn persist_metadata(
    path: &Path,
    meta: &SessionMetadata,
) -> Result<(), crate::error::AlephError> {
    let contents =
        serde_json::to_string_pretty(meta).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Failed to serialize metadata: {e}"),
            suggestion: None,
        })?;
    crate::utils::atomic_write::atomic_write_file(path, &contents).await
}

/// Reconstruct a minimal but honest [`SessionMetadata`] from a session's
/// `transcript.jsonl`. Returns `Ok(None)` when the transcript is missing or
/// holds no parseable message — the caller quarantines instead.
///
/// `dir_name` becomes the key: on POSIX the dir name IS the key byte-for-byte
/// (`sanitize_key_for_dir` only touches `/`, `\\`, NUL), and a Windows dir
/// name is the key's sanitized form, which round-trips through
/// `session_dir()` to the same dir — either way the listing and lookups keep
/// resolving to this directory.
async fn rebuild_metadata_from_transcript(
    dir: &Path,
    dir_name: &str,
) -> Result<Option<SessionMetadata>, std::io::Error> {
    let transcript = match tokio::fs::read_to_string(dir.join("transcript.jsonl")).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let messages: Vec<MessageRecord> = transcript
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<MessageRecord>(l).ok())
        .collect();
    if messages.is_empty() {
        return Ok(None);
    }

    // Derive what the key encodes; a key that doesn't parse (shouldn't happen
    // for a dir the store itself created) still yields a listable session,
    // just with fallback agent/type labels.
    //
    // The directory name is not the key on every platform:
    // `sanitize_key_for_dir` maps `:` -> `_` on Windows, and `:` is the key's
    // own separator, so on Windows this parse used to fail for EVERY session
    // the store had ever written and a repaired `main` session came back
    // labelled `ephemeral`. `key_from_dir_name` is the guarded inverse — it
    // recovers the key or declines, and is the identity on POSIX.
    let key = crate::gateway::session_store::file_backend::key_from_dir_name(dir_name);
    let parsed = crate::gateway::router::SessionKey::from_key_string(&key);
    let (agent_id, session_type) = match &parsed {
        Some(crate::gateway::router::SessionKey::Main { agent_id, .. }) => {
            (agent_id.clone(), "main")
        }
        Some(crate::gateway::router::SessionKey::DirectMessage { agent_id, .. }) => {
            (agent_id.clone(), "peer")
        }
        Some(crate::gateway::router::SessionKey::Group { agent_id, .. }) => {
            (agent_id.clone(), "group")
        }
        Some(crate::gateway::router::SessionKey::Task { agent_id, .. }) => {
            (agent_id.clone(), "task")
        }
        Some(crate::gateway::router::SessionKey::Subagent { .. }) => {
            ("main".to_string(), "subagent")
        }
        Some(crate::gateway::router::SessionKey::Ephemeral { .. }) | None => {
            ("main".to_string(), "ephemeral")
        }
    };

    // `instant()` resolves the transcript's own mixed units — the same mixed
    // units this repair pass exists because of. Reading `timestamp` raw here
    // would rebuild the document with the damage baked back in.
    let created_at = messages
        .first()
        .and_then(MessageRecord::instant)
        .map_or_else(|| chrono::Utc::now().timestamp(), |dt| dt.timestamp());
    let last_active_at = messages
        .last()
        .and_then(MessageRecord::instant)
        .map_or(created_at, |dt| dt.timestamp());

    // Title and preview mirror the live writer (`append_message`): first user
    // message truncated to 60 chars, last message to 120.
    let derived_title = messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.trim())
        .filter(|t| !t.is_empty())
        .map(|t| {
            if t.chars().count() > 60 {
                t.chars().take(60).collect::<String>() + "..."
            } else {
                t.to_string()
            }
        });
    let preview = messages.last().map(|m| m.content.trim()).map(|p| {
        if p.chars().count() > 120 {
            p.chars().take(120).collect::<String>() + "..."
        } else {
            p.to_string()
        }
    });

    Ok(Some(SessionMetadata {
        key,
        agent_id,
        session_type: session_type.to_string(),
        created_at,
        last_active_at,
        message_count: messages.len() as i64,
        // Everything below is what a transcript cannot answer. Defaults, not
        // fabrications: a zero token count reads as "unknown", an invented one
        // would read as measured.
        total_tokens: 0,
        auto_reset_at: None,
        state: Some(crate::gateway::session_manager::SessionState::Idle),
        derived_title,
        last_message_preview: preview,
        ..Default::default()
    }))
}

#[cfg(test)]
mod normalize_tests {
    use super::*;
    use crate::gateway::session_store::file_backend::sanitize_key_for_dir;

    /// Create `base/<dir_name>/{metadata.json,transcript.jsonl}` where the
    /// metadata records `key`. `dir_name` may differ from the canonical form on
    /// purpose (simulating a cross-platform-copied dir).
    async fn seed_dir(base: &Path, dir_name: &str, key: &str) {
        let d = base.join(dir_name);
        tokio::fs::create_dir_all(&d).await.unwrap();
        tokio::fs::write(d.join("metadata.json"), format!(r#"{{"key":"{key}"}}"#))
            .await
            .unwrap();
        tokio::fs::write(d.join("transcript.jsonl"), "{}\n")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn renames_stale_dir_to_canonical_form() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // "stale-name" never matches sanitize_key_for_dir(key) on any platform.
        seed_dir(base, "stale-name", "agent:main:main").await;

        let renamed = normalize_session_dir_names(base).await;
        assert_eq!(renamed, 1);

        let canonical = sanitize_key_for_dir("agent:main:main");
        assert!(
            base.join(&canonical).join("transcript.jsonl").exists(),
            "transcript must be reachable under the canonical dir name"
        );
        assert!(!base.join("stale-name").exists(), "stale dir must be gone");
    }

    #[tokio::test]
    async fn leaves_canonical_and_metaless_dirs_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let canonical = sanitize_key_for_dir("agent:x:main");
        seed_dir(base, &canonical, "agent:x:main").await;
        // No metadata.json ⇒ must be skipped (e.g. `.archive`, channel scratch).
        tokio::fs::create_dir_all(base.join(".archive"))
            .await
            .unwrap();

        let renamed = normalize_session_dir_names(base).await;
        assert_eq!(renamed, 0);
        assert!(base.join(&canonical).exists());
        assert!(base.join(".archive").exists());
    }

    #[tokio::test]
    async fn skips_when_canonical_target_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let canonical = sanitize_key_for_dir("agent:dup:main");
        // A stale dir and the canonical target both carry the same key — the
        // pass must not clobber the existing canonical dir.
        seed_dir(base, "stale-dup", "agent:dup:main").await;
        seed_dir(base, &canonical, "agent:dup:main").await;

        let renamed = normalize_session_dir_names(base).await;
        assert_eq!(renamed, 0, "must not clobber an existing target");
        assert!(base.join("stale-dup").exists());
        assert!(base.join(&canonical).exists());
    }
}

#[cfg(test)]
mod repair_tests {
    use super::*;
    use crate::gateway::session_store::file_backend::sanitize_key_for_dir;

    fn msg(id: &str, role: &str, content: &str, timestamp: i64) -> MessageRecord {
        MessageRecord {
            id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    async fn seed_session(
        base: &Path,
        dir_name: &str,
        metadata: &str,
        messages: &[MessageRecord],
    ) -> std::path::PathBuf {
        // `dir_name` is a session KEY, and production never joins one raw: on
        // Windows `:` is illegal in a filename (os error 123), so `session_dir`
        // routes every key through `sanitize_key_for_dir`. Seeding with the raw
        // key made these four tests fail on Windows only — the fixture was
        // building a path production cannot produce.
        let dir = base.join(sanitize_key_for_dir(dir_name));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("metadata.json"), metadata)
            .await
            .unwrap();
        if !messages.is_empty() {
            let mut body = String::new();
            for m in messages {
                body.push_str(&serde_json::to_string(m).unwrap());
                body.push('\n');
            }
            tokio::fs::write(dir.join("transcript.jsonl"), body)
                .await
                .unwrap();
        }
        dir
    }

    fn full_meta_json(last_active_at: i64) -> String {
        serde_json::to_string_pretty(&SessionMetadata {
            key: "agent:main:main:s1".to_string(),
            agent_id: "main".to_string(),
            session_type: "main".to_string(),
            created_at: 1_784_900_000,
            last_active_at,
            message_count: 3,
            ..Default::default()
        })
        .unwrap()
    }

    /// The headline regression: a legacy ms-stamped session outranks every
    /// seconds-stamped one in the descending sort, pinning new conversations
    /// below the fold. After repair it must read as seconds.
    #[tokio::test]
    async fn millisecond_last_active_at_is_normalized_to_seconds() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let dir = seed_session(
            base,
            "agent:main:main:s1",
            &full_meta_json(1_785_082_006_020), // ms, the legacy damage
            &[],
        )
        .await;

        let report = repair_session_metadata(base).await;
        assert_eq!(report.timestamps_normalized, 1);

        let repaired: SessionMetadata = serde_json::from_str(
            &tokio::fs::read_to_string(dir.join("metadata.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(repaired.last_active_at, 1_785_082_006);
        // Nothing else moves.
        assert_eq!(repaired.created_at, 1_784_900_000);
        assert_eq!(repaired.message_count, 3);
    }

    /// Seconds-stamped files are byte-stable: the pass is idempotent and does
    /// not touch healthy metadata.
    #[tokio::test]
    async fn seconds_last_active_at_is_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let original = full_meta_json(1_785_082_006);
        let dir = seed_session(base, "agent:main:main:s1", &original, &[]).await;

        let report = repair_session_metadata(base).await;
        assert_eq!(report, MetadataRepairReport::default());
        assert_eq!(
            tokio::fs::read_to_string(dir.join("metadata.json"))
                .await
                .unwrap(),
            original
        );
    }

    /// A torn document with an intact transcript gets a rebuilt metadata whose
    /// answers all come from the transcript — and the listing parse succeeds.
    #[tokio::test]
    async fn torn_metadata_is_rebuilt_from_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // The exact damage shape caught in production: a complete JSON
        // document followed by the tail of an older, longer one.
        let torn = format!(
            "{}\n  \"estimated_cost_usd\": 0.17\n}}",
            full_meta_json(1_785_082_006).trim_end_matches('}')
        );
        let messages = [
            msg("m1", "user", "帮我写一个贪吃蛇游戏", 1_785_082_000_000), // ms row
            msg(
                "m2",
                "assistant",
                "已完成，见 index.html",
                1_785_082_006_000,
            ),
        ];
        let dir = seed_session(base, "agent:main:main:s1", &torn, &messages).await;

        let report = repair_session_metadata(base).await;
        assert_eq!(report.rebuilt_from_transcript, 1);

        let repaired: SessionMetadata = serde_json::from_str(
            &tokio::fs::read_to_string(dir.join("metadata.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(repaired.key, "agent:main:main:s1");
        assert_eq!(repaired.agent_id, "main");
        assert_eq!(repaired.session_type, "main");
        assert_eq!(repaired.message_count, 2);
        assert_eq!(
            repaired.derived_title.as_deref(),
            Some("帮我写一个贪吃蛇游戏")
        );
        assert_eq!(
            repaired.last_message_preview.as_deref(),
            Some("已完成，见 index.html")
        );
        // Mixed-unit transcript rows resolve through `instant()` — seconds out.
        assert_eq!(repaired.last_active_at, 1_785_082_006);
        assert_eq!(repaired.created_at, 1_785_082_000);
        // Unrecoverable counters stay at defaults, not invented values.
        assert_eq!(repaired.total_tokens, 0);
    }

    /// A torn document with no transcript at all cannot be rebuilt — it is
    /// quarantined so `list_sessions` stops warning on every poll, and the
    /// evidence stays on disk.
    #[tokio::test]
    async fn torn_metadata_without_transcript_is_quarantined() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let dir = seed_session(base, "agent:main:heartbeat:hb-1", "{not json", &[]).await;

        let report = repair_session_metadata(base).await;
        assert_eq!(report.quarantined, 1);
        assert!(!dir.join("metadata.json").exists());
        assert_eq!(
            tokio::fs::read_to_string(dir.join("metadata.json.corrupt"))
                .await
                .unwrap(),
            "{not json"
        );

        // Idempotent: a second run has nothing left to quarantine.
        let second = repair_session_metadata(base).await;
        assert_eq!(second, MetadataRepairReport::default());
    }

    /// Directories without metadata.json (`.archive`, scratch dirs) are not
    /// the pass's business.
    #[tokio::test]
    async fn dirs_without_metadata_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        tokio::fs::create_dir_all(base.join(".archive"))
            .await
            .unwrap();

        let report = repair_session_metadata(base).await;
        assert_eq!(report, MetadataRepairReport::default());
    }
}
