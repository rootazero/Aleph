//! Verify module: Check migration integrity

use alephcore::utils::sqlite_open::open_sqlite_safe;
use crate::export::session_dir_name;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn run_verify(
    sessions_dir: PathBuf,
    metadata_db: PathBuf,
    original_db: Option<PathBuf>,
    fix: bool,
    remove_orphans: bool,
) -> Result<()> {
    log::info!("Starting verification...");
    log::info!("  Sessions directory: {}", sessions_dir.display());
    log::info!("  Metadata database: {}", metadata_db.display());
    if let Some(ref orig) = original_db {
        log::info!("  Original database: {}", orig.display());
    }

    let mut report = VerificationReport::default();

    // Open metadata database
    let conn = open_sqlite_safe(&metadata_db).context("Failed to open metadata database")?;

    // Check 1: Session count
    log::info!("[1/6] Checking session count...");
    check_session_count(&conn, &sessions_dir, &original_db, &mut report)?;

    // Check 2: File existence
    log::info!("[2/6] Checking session files...");
    check_session_files(&conn, &sessions_dir, fix, &mut report)?;

    // Check 3: Metadata consistency
    log::info!("[3/6] Checking metadata consistency...");
    check_metadata_consistency(&conn, &sessions_dir, fix, &mut report)?;

    // Check 4: Content hashes
    log::info!("[4/6] Checking content hashes...");
    check_content_hashes(&conn, &sessions_dir, &mut report)?;

    // Check 5: Attachment references
    log::info!("[5/6] Checking attachment references...");
    check_attachments(&conn, &sessions_dir, remove_orphans, &mut report)?;

    // Check 6: FTS5 index
    log::info!("[6/6] Checking FTS5 index...");
    check_fts5_index(&conn, &sessions_dir, &mut report)?;

    // Print report
    print_report(&report);

    if report.errors > 0 {
        anyhow::bail!("Verification failed with {} errors", report.errors);
    }

    Ok(())
}

#[derive(Default)]
struct VerificationReport {
    session_count_db: i64,
    session_count_fs: usize,
    session_count_original: Option<i64>,
    missing_files: Vec<String>,
    session_mismatches: Vec<String>,
    invalid_metadata: Vec<String>,
    invalid_hashes: Vec<String>,
    missing_attachments: Vec<String>,
    warnings: Vec<String>,
    errors: usize,
    fixes_applied: usize,
}

impl VerificationReport {
    /// Record a repair that resolved one previously recorded error.
    ///
    /// The error counter is decremented so `--fix` can actually turn a
    /// failing verification into a passing one; `fixes_applied` keeps the
    /// count of repairs for the report.
    fn record_fix(&mut self) {
        self.errors = self.errors.saturating_sub(1);
        self.fixes_applied += 1;
    }
}

fn check_session_count(
    conn: &Connection,
    sessions_dir: &Path,
    original_db: &Option<PathBuf>,
    report: &mut VerificationReport,
) -> Result<()> {
    report.session_count_db =
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;

    let mut stmt = conn.prepare("SELECT session_id FROM sessions")?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let expected_sessions = session_ids
        .iter()
        .map(|session_id| Ok((session_dir_name(session_id)?, session_id.clone())))
        .collect::<Result<HashMap<_, _>>>()?;
    let expected_dirs: HashSet<String> = expected_sessions.keys().cloned().collect();

    let mut actual_dirs = HashSet::new();
    for entry in WalkDir::new(sessions_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_entry(|entry| entry.file_type().is_dir())
    {
        actual_dirs.insert(entry?.file_name().to_string_lossy().into_owned());
    }
    report.session_count_fs = actual_dirs.len();

    if let Some(orig_path) = original_db {
        let orig_conn = open_sqlite_safe(orig_path)?;
        report.session_count_original = Some(orig_conn.query_row(
            "SELECT COUNT(DISTINCT session_id) FROM raw_memories WHERE session_id IS NOT NULL",
            [],
            |row| row.get(0),
        )?);
        check_original_timestamps(&orig_conn, report)?;
    }

    if report.session_count_db != report.session_count_fs as i64 {
        report.warnings.push(format!(
            "Session count mismatch: metadata.db has {}, filesystem has {}",
            report.session_count_db, report.session_count_fs
        ));
    }

    for (directory, session_id) in &expected_sessions {
        if !actual_dirs.contains(directory) {
            report.errors += 1;
            report.session_mismatches.push(format!(
                "{}: session directory {} is missing from the filesystem",
                session_id, directory
            ));
        }
    }
    for directory in actual_dirs.difference(&expected_dirs) {
        report.errors += 1;
        report.session_mismatches.push(format!(
            "filesystem-only session directory {} is absent from metadata.db",
            directory
        ));
    }

    if let Some(orig_count) = report.session_count_original {
        if report.session_count_db != orig_count {
            report.errors += 1;
            report.warnings.push(format!(
                "Session count doesn't match original: expected {}, got {}",
                orig_count, report.session_count_db
            ));
        }
    }

    Ok(())
}

fn check_original_timestamps(conn: &Connection, report: &mut VerificationReport) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(raw_memories)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get(1))?
        .collect::<Result<Vec<_>, _>>()?;
    let timestamp_column = if columns.iter().any(|column| column == "created_at") {
        "created_at"
    } else if columns.iter().any(|column| column == "started_at") {
        "started_at"
    } else {
        return Ok(());
    };

    let query = format!(
        "SELECT session_id, rowid FROM raw_memories \
         WHERE session_id IS NOT NULL AND {timestamp_column} IS NULL"
    );
    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (session_id, row_id) = row?;
        report.errors += 1;
        report.warnings.push(format!(
            "Original database row {} (session {}) has null {}; export would drop this row",
            row_id, session_id, timestamp_column
        ));
    }
    Ok(())
}

fn check_session_files(
    conn: &Connection,
    sessions_dir: &Path,
    fix: bool,
    report: &mut VerificationReport,
) -> Result<()> {
    let mut stmt = conn.prepare("SELECT session_id FROM sessions")?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for session_id in session_ids {
        let session_dir = sessions_dir.join(session_dir_name(&session_id)?);

        // Check required files
        let transcript = session_dir.join("transcript.md");
        let tool_outputs = session_dir.join("tool_outputs.md");
        let metadata = session_dir.join("metadata.json");

        let mut missing = Vec::new();
        if !transcript.exists() {
            missing.push("transcript.md");
        }
        if !tool_outputs.exists() {
            missing.push("tool_outputs.md");
        }
        if !metadata.exists() {
            missing.push("metadata.json");
        }

        if !missing.is_empty() {
            report.errors += 1;
            report
                .missing_files
                .push(format!("{}: missing {}", session_id, missing.join(", ")));

            if fix && !metadata.exists() {
                // Only a missing metadata.json can be regenerated; a missing
                // transcript or tool-output file stays an error.
                regenerate_metadata(conn, &session_id, &session_dir)?;
                if missing.len() == 1 {
                    report.record_fix();
                } else {
                    report.fixes_applied += 1;
                }
            }
        }
    }

    Ok(())
}

fn check_metadata_consistency(
    conn: &Connection,
    sessions_dir: &Path,
    fix: bool,
    report: &mut VerificationReport,
) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT session_id, agent_id, workspace_id, started_at, ended_at, \
         message_count, tool_call_count, attachment_count, compression_status, tags \
         FROM sessions",
    )?;
    let sessions: Vec<SessionMetadataRow> = stmt
        .query_map([], |row| {
            Ok(SessionMetadataRow {
                session_id: row.get(0)?,
                agent_id: row.get(1)?,
                workspace_id: row.get(2)?,
                started_at: row.get(3)?,
                ended_at: row.get(4)?,
                message_count: row.get(5)?,
                tool_call_count: row.get(6)?,
                attachment_count: row.get(7)?,
                compression_status: row.get(8)?,
                tags: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    for session in sessions {
        let session_id = &session.session_id;
        let metadata_path = sessions_dir
            .join(session_dir_name(session_id)?)
            .join("metadata.json");

        if !metadata_path.exists() {
            continue; // Already reported in check_session_files
        }

        let content = match fs::read_to_string(&metadata_path) {
            Ok(content) => content,
            Err(error) => {
                report.errors += 1;
                report.invalid_metadata.push(format!(
                    "{}: cannot read metadata.json - {}",
                    session_id, error
                ));
                continue;
            }
        };
        let metadata: serde_json::Value = match serde_json::from_str(&content) {
            Ok(metadata) => metadata,
            Err(error) => {
                report.errors += 1;
                report
                    .invalid_metadata
                    .push(format!("{}: invalid JSON - {}", session_id, error));
                if fix {
                    regenerate_metadata(conn, session_id, metadata_path.parent().unwrap())?;
                    report.record_fix();
                }
                continue;
            }
        };
        let mismatches = validate_metadata(&metadata, &session);
        if !mismatches.is_empty() {
            report.errors += 1;
            for message in mismatches {
                report
                    .invalid_metadata
                    .push(format!("{}: {}", session_id, message));
            }
            if fix {
                regenerate_metadata(conn, session_id, metadata_path.parent().unwrap())?;
                report.record_fix();
            }
        }
    }

    Ok(())
}

struct SessionMetadataRow {
    session_id: String,
    agent_id: String,
    workspace_id: Option<String>,
    started_at: i64,
    ended_at: Option<i64>,
    message_count: i64,
    tool_call_count: i64,
    attachment_count: i64,
    compression_status: String,
    tags: Option<String>,
}

fn validate_metadata(
    metadata: &serde_json::Value,
    session: &SessionMetadataRow,
) -> Vec<String> {
    let Some(object) = metadata.as_object() else {
        return vec!["metadata must be a JSON object".to_string()];
    };
    let expected = serde_json::json!({
        "session_id": session.session_id.as_str(),
        "agent_id": session.agent_id.as_str(),
        "workspace_id": session.workspace_id.as_deref(),
        "started_at": session.started_at,
        "ended_at": session.ended_at,
        "message_count": session.message_count,
        "tool_call_count": session.tool_call_count,
        "attachment_count": session.attachment_count,
        "compression_status": session.compression_status.as_str(),
    });
    let mut errors = Vec::new();
    for field in [
        "session_id",
        "agent_id",
        "workspace_id",
        "started_at",
        "ended_at",
        "message_count",
        "tool_call_count",
        "attachment_count",
        "compression_status",
    ] {
        match object.get(field) {
            None => errors.push(format!("metadata is missing required field {field}")),
            Some(actual) if actual != &expected[field] => errors.push(format!(
                "metadata field {field} does not match database (expected {}, got {})",
                expected[field], actual
            )),
            Some(_) => {}
        }
    }
    for field in ["message_count", "tool_call_count", "attachment_count"] {
        if object
            .get(field)
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|count| count < 0)
        {
            errors.push(format!("metadata field {field} must be nonnegative"));
        }
    }

    match object.get("tags") {
        None => errors.push("metadata is missing required field tags".to_string()),
        Some(serde_json::Value::Array(values))
            if values.iter().all(serde_json::Value::is_string) =>
        {
            let actual = values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(",");
            let expected = session.tags.as_deref().unwrap_or("");
            if actual != expected {
                errors.push(format!(
                    "metadata field tags does not match database (expected {:?}, got {:?})",
                    expected, actual
                ));
            }
        }
        Some(_) => errors.push("metadata field tags must be an array of strings".to_string()),
    }
    errors
}
fn check_content_hashes(
    conn: &Connection,
    sessions_dir: &Path,
    report: &mut VerificationReport,
) -> Result<()> {
    let mut stmt = conn
        .prepare("SELECT session_id, content_hash FROM sessions WHERE content_hash IS NOT NULL")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    for (session_id, stored_hash) in rows {
        let transcript_path = sessions_dir
            .join(session_dir_name(&session_id)?)
            .join("transcript.md");

        if !transcript_path.exists() {
            continue;
        }

        let content = fs::read_to_string(&transcript_path)?;
        let computed_hash = compute_sha256(&content);

        if computed_hash != stored_hash {
            report.errors += 1;
            let stored_preview = stored_hash.get(..8).unwrap_or(&stored_hash);
            report.invalid_hashes.push(format!(
                "{}: content hash mismatch (stored: {}, computed: {})",
                session_id,
                stored_preview,
                &computed_hash[..8]
            ));
            report.warnings.push(format!(
                "{}: content hash mismatch (stored: {}, computed: {})",
                session_id,
                stored_preview,
                &computed_hash[..8]
            ));
            // The stored hash is the expected value. Overwriting it with the
            // hash of the current file would launder a truncated or edited
            // transcript: the only evidence of the corruption would be
            // destroyed and every later run would report the file as valid.
            // Regenerating the transcript needs the source export, so the
            // mismatch is left for the operator to resolve.
            log::warn!(
                "Session {}: content hash mismatch; the stored hash is preserved (not auto-repaired)",
                session_id
            );
        }
    }

    Ok(())
}

fn check_attachments(
    conn: &Connection,
    sessions_dir: &Path,
    remove_orphans: bool,
    report: &mut VerificationReport,
) -> Result<()> {
    // Every transformed session with attachment_count > 0 must have a
    // manifest. The manifest is the portable link between a session and the
    // content-addressed attachment row in metadata.db.
    let migration_root = migration_root_for(sessions_dir)
        .context("sessions directory must have a migration root")?;
    let mut referenced_hashes = HashSet::new();
    let mut referenced_paths = HashSet::new();
    let mut attachment_identities: HashMap<String, (String, String)> = HashMap::new();
    // A session whose metadata or attachment manifest cannot be read may
    // still own attachment files on disk. While any session is in that
    // state the set of referenced hashes is incomplete, and sweeping files
    // that are not in the set would delete live attachments.
    let mut manifests_complete = true;

    // Database references are authoritative ownership evidence even when the
    // corresponding session directory or manifest has disappeared.
    let mut stmt = conn.prepare("SELECT content_hash, file_path FROM attachments")?;
    let database_references: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (content_hash, file_path) in database_references {
        referenced_hashes.insert(content_hash.clone());
        match resolve_manifest_path(migration_root, &file_path) {
            Ok(path) => {
                if let Ok(canonical_path) = fs::canonicalize(&path) {
                    referenced_paths.insert(canonical_path);
                }
                referenced_paths.insert(path);
            }
            Err(error) => {
                report.errors += 1;
                report.missing_attachments.push(format!(
                    "database attachment {} has invalid path {}: {}",
                    content_hash, file_path, error
                ));
                manifests_complete = false;
            }
        }
    }

    for entry in WalkDir::new(sessions_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_entry(|e| e.file_type().is_dir())
    {
        let session_dir = entry?.into_path();
        let metadata_path = session_dir.join("metadata.json");
        if !metadata_path.exists() {
            manifests_complete = false;
            continue;
        }
        let metadata: serde_json::Value = match fs::read_to_string(&metadata_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
        {
            Some(metadata) => metadata,
            None => {
                // Reported by check_metadata_consistency.
                manifests_complete = false;
                continue;
            }
        };
        let Some(count) = metadata["attachment_count"].as_i64().filter(|count| *count >= 0) else {
            // Without a trustworthy attachment count, this session may still
            // own blobs and the orphan reference set is incomplete.
            manifests_complete = false;
            continue;
        };
        let manifest_path = session_dir.join("attachments.json");
        if count == 0 {
            if manifest_path.exists() {
                let manifest: Vec<AttachmentManifestEntry> =
                    match fs::read_to_string(&manifest_path)
                        .ok()
                        .and_then(|content| serde_json::from_str(&content).ok())
                    {
                        Some(manifest) => manifest,
                        None => {
                            report.errors += 1;
                            report.missing_attachments.push(format!(
                                "{}: invalid attachments.json",
                                metadata["session_id"].as_str().unwrap_or("unknown")
                            ));
                            manifests_complete = false;
                            continue;
                        }
                    };
                if !manifest.is_empty() {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: attachment_count is 0 but manifest has {} entries",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        manifest.len()
                    ));
                }
            }
            continue;
        }

        if !manifest_path.exists() {
            report.errors += 1;
            report.missing_attachments.push(format!(
                "{}: attachment_count={} but attachments.json is missing",
                metadata["session_id"].as_str().unwrap_or("unknown"),
                count
            ));
            manifests_complete = false;
            continue;
        }

        let manifest: Vec<AttachmentManifestEntry> = match fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
        {
            Some(manifest) => manifest,
            None => {
                report.errors += 1;
                report.missing_attachments.push(format!(
                    "{}: invalid attachments.json",
                    metadata["session_id"].as_str().unwrap_or("unknown")
                ));
                manifests_complete = false;
                continue;
            }
        };
        if manifest.len() as i64 != count {
            report.errors += 1;
            report.missing_attachments.push(format!(
                "{}: metadata declares {} attachments but manifest has {}",
                metadata["session_id"].as_str().unwrap_or("unknown"),
                count,
                manifest.len()
            ));
            manifests_complete = false;
        }

        for attachment in manifest {
            if let Some((first_path, first_mime)) =
                attachment_identities.get(&attachment.content_hash)
            {
                if first_path != &attachment.file_path || first_mime != &attachment.mime_type {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: attachment hash {} has conflicting identities ({}, {}) and ({}, {})",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        attachment.content_hash,
                        first_path,
                        first_mime,
                        attachment.file_path,
                        attachment.mime_type
                    ));
                }
            } else {
                attachment_identities.insert(
                    attachment.content_hash.clone(),
                    (attachment.file_path.clone(), attachment.mime_type.clone()),
                );
            }
            referenced_hashes.insert(attachment.content_hash.clone());
            let path = match resolve_manifest_path(migration_root, &attachment.file_path) {
                Ok(path) => path,
                Err(error) => {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: invalid attachment path {}: {}",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        attachment.file_path,
                        error
                    ));
                    continue;
                }
            };
            let canonical_root = match fs::canonicalize(migration_root) {
                Ok(root) => root,
                Err(error) => {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: cannot resolve migration root: {}",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        error
                    ));
                    continue;
                }
            };
            let canonical_path = match fs::canonicalize(&path) {
                Ok(path) => path,
                Err(error) => {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: cannot resolve attachment {}: {}",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        attachment.file_path,
                        error
                    ));
                    continue;
                }
            };
            if !canonical_path.starts_with(&canonical_root) {
                report.errors += 1;
                report.missing_attachments.push(format!(
                    "{}: attachment path resolves outside migration root: {}",
                    metadata["session_id"].as_str().unwrap_or("unknown"),
                    attachment.file_path
                ));
                continue;
            }
            let bytes = match fs::read(&canonical_path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: cannot read attachment {}: {}",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        attachment.file_path,
                        error
                    ));
                    continue;
                }
            };
            let hash = compute_sha256_bytes(&bytes);
            if hash != attachment.content_hash {
                report.errors += 1;
                report.missing_attachments.push(format!(
                    "{}: SHA256 mismatch for {} (expected {}, got {})",
                    metadata["session_id"].as_str().unwrap_or("unknown"),
                    attachment.file_path,
                    attachment.content_hash,
                    hash
                ));
            }
            let indexed: Option<(String, String, i64)> = match conn
                .query_row(
                    "SELECT file_path, mime_type, size_bytes FROM attachments \
                     WHERE content_hash = ?1",
                    [&attachment.content_hash],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
            {
                Ok(indexed) => indexed,
                Err(error) => {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: cannot query attachment index: {}",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        error
                    ));
                    continue;
                }
            };
            match indexed {
                Some((indexed_path, indexed_mime, indexed_size))
                    if indexed_path == attachment.file_path
                        && indexed_mime == attachment.mime_type
                        && indexed_size == bytes.len() as i64 => {}
                Some((indexed_path, indexed_mime, indexed_size)) => {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: indexed attachment metadata mismatch (path={}, MIME={}, size={})",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        indexed_path,
                        indexed_mime,
                        indexed_size
                    ));
                }
                None => {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: attachment {} is not indexed in metadata.db",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        attachment.content_hash
                    ));
                }
            }
        }
    }

    let attachments_dir = if sessions_dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        == Some("raw")
    {
        sessions_dir
            .parent()
            .context("sessions directory has no raw parent")?
            .join("attachments")
    } else {
        migration_root.join("attachments")
    };
    if attachments_dir.exists() {
        let can_sweep = manifests_complete && report.errors == 0;
        if remove_orphans && !can_sweep {
            // Unknown references or any verification mismatch make "not
            // referenced" indistinguishable from "owned by damaged state".
            report.warnings.push(format!(
                "Orphan sweep skipped: attachment references are incomplete or verification \
                 has errors ({}). Fix the verification errors first.",
                attachments_dir.display()
            ));
        }
        for entry in WalkDir::new(&attachments_dir)
            .into_iter()
            .filter_entry(|e| e.file_type().is_file())
        {
            let path = entry?.into_path();
            let bytes = fs::read(&path)?;
            let hash = compute_sha256_bytes(&bytes);
            let canonical_path = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            let is_referenced = referenced_hashes.contains(&hash)
                || referenced_paths.contains(&path)
                || referenced_paths.contains(&canonical_path);
            if !is_referenced && remove_orphans && can_sweep {
                fs::remove_file(&path)?;
                report.fixes_applied += 1;
            }
        }
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct AttachmentManifestEntry {
    content_hash: String,
    mime_type: String,
    file_path: String,
}

fn migration_root_for(sessions_dir: &Path) -> Option<&Path> {
    let parent = sessions_dir.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) == Some("raw") {
        parent.parent()
    } else {
        Some(parent)
    }
}

fn resolve_manifest_path(migration_root: &Path, file_path: &str) -> Result<PathBuf> {
    let relative = Path::new(file_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        anyhow::bail!("attachment manifest path escapes migration root: {file_path}");
    }
    let path = migration_root.join(relative);
    if !path.starts_with(migration_root) {
        anyhow::bail!("attachment manifest path escapes migration root: {file_path}");
    }
    Ok(path)
}

fn check_fts5_index(
    conn: &Connection,
    sessions_dir: &Path,
    report: &mut VerificationReport,
) -> Result<()> {
    let mut stmt = conn.prepare("SELECT session_id FROM sessions")?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut expected_transcripts = HashMap::new();
    for session_id in session_ids {
        let transcript_path = sessions_dir
            .join(session_dir_name(&session_id)?)
            .join("transcript.md");
        if transcript_path.exists() {
            expected_transcripts.insert(session_id, fs::read_to_string(transcript_path)?);
        }
    }

    let mut stmt = match conn.prepare("SELECT session_id, content FROM session_transcripts_fts") {
        Ok(stmt) => stmt,
        Err(error) => {
            report.errors += 1;
            report.warnings.push(format!("FTS5 index error: {}", error));
            return Ok(());
        }
    };
    let rows = match stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) {
        Ok(rows) => rows,
        Err(error) => {
            report.errors += 1;
            report.warnings.push(format!("FTS5 index error: {}", error));
            return Ok(());
        }
    };
    let mut indexed_transcripts: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        match row {
            Ok((session_id, content)) => indexed_transcripts
                .entry(session_id)
                .or_default()
                .push(content),
            Err(error) => {
                report.errors += 1;
                report.warnings.push(format!("FTS5 index error: {}", error));
            }
        }
    }

    for (session_id, contents) in &indexed_transcripts {
        let Some(expected) = expected_transcripts.get(session_id) else {
            report.errors += 1;
            report.warnings.push(format!(
                "FTS5 index contains unexpected session ID {}",
                session_id
            ));
            continue;
        };
        if contents.len() != 1 {
            report.errors += 1;
            report.warnings.push(format!(
                "FTS5 index contains {} rows for session {}; expected exactly one",
                contents.len(), session_id
            ));
        }
        if contents.first() != Some(expected) {
            report.errors += 1;
            report.warnings.push(format!(
                "FTS5 content mismatch for session {}",
                session_id
            ));
        }
    }
    for session_id in expected_transcripts.keys() {
        if !indexed_transcripts.contains_key(session_id) {
            report.errors += 1;
            report.warnings.push(format!(
                "FTS5 index is missing transcript for session {}",
                session_id
            ));
        }
    }

    Ok(())
}

fn regenerate_metadata(conn: &Connection, session_id: &str, session_dir: &Path) -> Result<()> {
    use serde_json::json;

    let mut stmt = conn.prepare(
        "SELECT agent_id, workspace_id, started_at, ended_at, \
         message_count, tool_call_count, attachment_count, \
         compression_status, tags \
         FROM sessions WHERE session_id = ?",
    )?;
    let values = stmt.query_row([session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    })?;
    let tags = match values.8 {
        Some(tags) if tags.contains(',') => anyhow::bail!(
            "Cannot regenerate metadata for session {session_id}: \
             comma-separated tags are ambiguous"
        ),
        Some(tags) if tags.is_empty() => Vec::new(),
        Some(tag) => vec![tag],
        None => Vec::new(),
    };
    let metadata = json!({
        "session_id": session_id,
        "agent_id": values.0,
        "workspace_id": values.1,
        "started_at": values.2,
        "ended_at": values.3,
        "message_count": values.4,
        "tool_call_count": values.5,
        "attachment_count": values.6,
        "compression_status": values.7,
        "tags": tags,
    });

    let metadata_path = session_dir.join("metadata.json");
    let file = fs::File::create(&metadata_path)?;
    serde_json::to_writer_pretty(file, &metadata)?;

    Ok(())
}

fn compute_sha256_bytes(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

fn compute_sha256(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

fn print_report(report: &VerificationReport) {
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Verification Report");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Session counts
    println!("✓ Session count: {} (metadata.db)", report.session_count_db);
    println!("✓ Session count: {} (filesystem)", report.session_count_fs);
    if let Some(orig) = report.session_count_original {
        if orig == report.session_count_db {
            println!("✓ Session count matches original: {}", orig);
        } else {
            println!(
                "✗ Session count mismatch: expected {}, got {}",
                orig, report.session_count_db
            );
        }
    }
    if !report.session_mismatches.is_empty() {
        println!(
            "✗ Session directory mismatches: {}",
            report.session_mismatches.len()
        );
        for message in &report.session_mismatches {
            println!("    {}", message);
        }
    }

    // File checks
    if report.missing_files.is_empty() {
        println!("✓ All session files present");
    } else {
        println!("✗ Missing files: {}", report.missing_files.len());
        for msg in &report.missing_files {
            println!("    {}", msg);
        }
    }

    // Metadata checks
    if report.invalid_metadata.is_empty() {
        println!("✓ All metadata.json files valid");
    } else {
        println!("✗ Invalid metadata: {}", report.invalid_metadata.len());
        for msg in &report.invalid_metadata {
            println!("    {}", msg);
        }
    }

    if report.missing_attachments.is_empty() {
        println!("✓ All attachment files, hashes, and database references valid");
    } else {
        println!("✗ Attachment errors: {}", report.missing_attachments.len());
        for msg in &report.missing_attachments {
            println!("    {}", msg);
        }
    }

    // Hash checks
    if report.invalid_hashes.is_empty() {
        println!("✓ Content hashes valid");
    } else {
        println!("⚠ Invalid hashes: {}", report.invalid_hashes.len());
    }

    // Attachments
    if report.missing_attachments.is_empty() {
        println!("✓ Attachment references valid");
    } else {
        println!(
            "✗ Missing attachments: {}",
            report.missing_attachments.len()
        );
    }

    // Warnings
    if !report.warnings.is_empty() {
        println!("\nWarnings:");
        for warning in &report.warnings {
            println!("  ⚠ {}", warning);
        }
    }

    // Fixes
    if report.fixes_applied > 0 {
        println!("\n✓ Applied {} fixes", report.fixes_applied);
    }

    // Summary
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    if report.errors == 0 {
        println!("Migration Status: SUCCESS");
    } else {
        println!("Migration Status: FAILED");
        println!("Errors: {}", report.errors);
    }
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
}
