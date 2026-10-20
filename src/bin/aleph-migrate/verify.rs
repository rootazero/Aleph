//! Verify module: Check migration integrity

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashSet;
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
    let conn = Connection::open(&metadata_db).context("Failed to open metadata database")?;

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
    check_content_hashes(&conn, &sessions_dir, fix, &mut report)?;

    // Check 5: Attachment references
    log::info!("[5/6] Checking attachment references...");
    check_attachments(&conn, &sessions_dir, remove_orphans, &mut report)?;

    // Check 6: FTS5 index
    log::info!("[6/6] Checking FTS5 index...");
    check_fts5_index(&conn, &mut report)?;

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
    invalid_metadata: Vec<String>,
    invalid_hashes: Vec<String>,
    missing_attachments: Vec<String>,
    warnings: Vec<String>,
    errors: usize,
    fixes_applied: usize,
}

fn check_session_count(
    conn: &Connection,
    sessions_dir: &Path,
    original_db: &Option<PathBuf>,
    report: &mut VerificationReport,
) -> Result<()> {
    // Count in metadata.db
    report.session_count_db =
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;

    // Count on filesystem
    report.session_count_fs = WalkDir::new(sessions_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_entry(|e| e.file_type().is_dir())
        .count();

    // Count in original database if provided
    if let Some(orig_path) = original_db {
        let orig_conn = Connection::open(orig_path)?;
        report.session_count_original = Some(orig_conn.query_row(
            "SELECT COUNT(DISTINCT session_id) FROM raw_memories WHERE source = 'session' AND session_id IS NOT NULL",
            [],
            |row| row.get(0),
        )?);
    }

    // Check consistency
    if report.session_count_db != report.session_count_fs as i64 {
        report.warnings.push(format!(
            "Session count mismatch: metadata.db has {}, filesystem has {}",
            report.session_count_db, report.session_count_fs
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
        let session_dir = sessions_dir.join(&session_id);

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

            if fix {
                // Try to regenerate missing files
                if !metadata.exists() {
                    regenerate_metadata(conn, &session_id, &session_dir)?;
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
    let mut stmt = conn.prepare("SELECT session_id FROM sessions")?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for session_id in session_ids {
        let metadata_path = sessions_dir.join(&session_id).join("metadata.json");

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
                    regenerate_metadata(conn, &session_id, metadata_path.parent().unwrap())?;
                    report.fixes_applied += 1;
                }
                continue;
            }
        };
        if let Err(message) = validate_metadata(&metadata, &session_id) {
            report.errors += 1;
            report
                .invalid_metadata
                .push(format!("{}: {}", session_id, message));
            if fix {
                regenerate_metadata(conn, &session_id, metadata_path.parent().unwrap())?;
                report.fixes_applied += 1;
            }
        }
    }

    Ok(())
}

fn validate_metadata(metadata: &serde_json::Value, session_id: &str) -> Result<()> {
    let object = metadata
        .as_object()
        .context("metadata must be a JSON object")?;
    for field in [
        "session_id",
        "agent_id",
        "started_at",
        "message_count",
        "tool_call_count",
        "attachment_count",
    ] {
        if !object.contains_key(field) {
            anyhow::bail!("metadata is missing required field {field}");
        }
    }
    if object.get("session_id").and_then(|v| v.as_str()) != Some(session_id) {
        anyhow::bail!("metadata session_id does not match directory");
    }
    for field in ["session_id", "agent_id"] {
        if object.get(field).and_then(|v| v.as_str()).is_none() {
            anyhow::bail!("metadata field {field} must be a string");
        }
    }
    for field in [
        "started_at",
        "message_count",
        "tool_call_count",
        "attachment_count",
    ] {
        if object.get(field).and_then(|v| v.as_i64()).is_none() {
            anyhow::bail!("metadata field {field} must be an integer");
        }
    }
    Ok(())
}
fn check_content_hashes(
    conn: &Connection,
    sessions_dir: &Path,
    fix: bool,
    report: &mut VerificationReport,
) -> Result<()> {
    let mut stmt = conn
        .prepare("SELECT session_id, content_hash FROM sessions WHERE content_hash IS NOT NULL")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    for (session_id, stored_hash) in rows {
        let transcript_path = sessions_dir.join(&session_id).join("transcript.md");

        if !transcript_path.exists() {
            continue;
        }

        let content = fs::read_to_string(&transcript_path)?;
        let computed_hash = compute_sha256(&content);

        if computed_hash != stored_hash {
            report.errors += 1;
            let stored_preview = stored_hash.get(..8).unwrap_or(&stored_hash);
            report.warnings.push(format!(
                "{}: content hash mismatch (stored: {}, computed: {})",
                session_id,
                stored_preview,
                &computed_hash[..8]
            ));

            if fix {
                conn.execute(
                    "UPDATE sessions SET content_hash = ?1 WHERE session_id = ?2",
                    rusqlite::params![computed_hash, session_id],
                )?;
                report.fixes_applied += 1;
            }
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

    for entry in WalkDir::new(sessions_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_entry(|e| e.file_type().is_dir())
    {
        let session_dir = entry?.into_path();
        let metadata_path = session_dir.join("metadata.json");
        if !metadata_path.exists() {
            continue;
        }
        let metadata: serde_json::Value = match fs::read_to_string(&metadata_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
        {
            Some(metadata) => metadata,
            None => continue, // Reported by check_metadata_consistency.
        };
        let count = metadata["attachment_count"].as_i64().unwrap_or(0);
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
        }

        for attachment in manifest {
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
            let indexed: Option<(String, i64)> = match conn
                .query_row(
                    "SELECT file_path, size_bytes FROM attachments WHERE content_hash = ?1",
                    [&attachment.content_hash],
                    |row| Ok((row.get(0)?, row.get(1)?)),
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
                Some((indexed_path, indexed_size))
                    if indexed_path == attachment.file_path
                        && indexed_size == bytes.len() as i64 => {}
                Some((indexed_path, indexed_size)) => {
                    report.errors += 1;
                    report.missing_attachments.push(format!(
                        "{}: indexed attachment metadata mismatch (path={}, size={})",
                        metadata["session_id"].as_str().unwrap_or("unknown"),
                        indexed_path,
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
        for entry in WalkDir::new(&attachments_dir)
            .into_iter()
            .filter_entry(|e| e.file_type().is_file())
        {
            let path = entry?.into_path();
            let bytes = fs::read(&path)?;
            let hash = compute_sha256_bytes(&bytes);
            if !referenced_hashes.contains(&hash) && remove_orphans {
                fs::remove_file(path)?;
                report.fixes_applied += 1;
            }
        }
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct AttachmentManifestEntry {
    content_hash: String,
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

fn check_fts5_index(conn: &Connection, report: &mut VerificationReport) -> Result<()> {
    // Check if FTS5 table exists and has data
    let count: Result<i64, _> =
        conn.query_row("SELECT COUNT(*) FROM session_transcripts_fts", [], |row| {
            row.get(0)
        });

    match count {
        Ok(n) if n > 0 => {
            // Try a simple search to verify index works
            let search_result: Result<i64, _> = conn.query_row(
                "SELECT COUNT(*) FROM session_transcripts_fts WHERE content MATCH 'session'",
                [],
                |row| row.get(0),
            );

            if search_result.is_err() {
                report.errors += 1;
                report.warnings.push("FTS5 index is corrupted".to_string());
            }
        }
        Ok(0) => {
            report.warnings.push("FTS5 index is empty".to_string());
        }
        Ok(_) => {
            // Negative counts shouldn't happen, but handle them gracefully
            report
                .warnings
                .push("FTS5 index has invalid count".to_string());
        }
        Err(e) => {
            report.errors += 1;
            report.warnings.push(format!("FTS5 index error: {}", e));
        }
    }

    Ok(())
}

fn regenerate_metadata(conn: &Connection, session_id: &str, session_dir: &Path) -> Result<()> {
    use serde_json::json;

    // Get session data from database
    let mut stmt = conn.prepare(
        "SELECT agent_id, workspace_id, started_at, ended_at, \
         message_count, tool_call_count, attachment_count, \
         compression_status, tags \
         FROM sessions WHERE session_id = ?",
    )?;

    let metadata = stmt.query_row([session_id], |row| {
        Ok(json!({
            "session_id": session_id,
            "agent_id": row.get::<_, String>(0)?,
            "workspace_id": row.get::<_, Option<String>>(1)?,
            "started_at": row.get::<_, i64>(2)?,
            "ended_at": row.get::<_, Option<i64>>(3)?,
            "message_count": row.get::<_, i64>(4)?,
            "tool_call_count": row.get::<_, i64>(5)?,
            "attachment_count": row.get::<_, i64>(6)?,
            "compression_status": row.get::<_, String>(7)?,
            "tags": row.get::<_, Option<String>>(8)?
                .map(|s| s.split(',').map(|t| t.trim().to_string()).collect::<Vec<_>>())
                .unwrap_or_default(),
        }))
    })?;

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
