//! Transform module: JSON → Markdown

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::export::{session_dir_name, ExportData, ExportSession};

pub fn run_transform(
    input_path: PathBuf,
    output_dir: PathBuf,
    attachments_dir: PathBuf,
    parallel: usize,
    overwrite: bool,
) -> Result<()> {
    log::info!("Starting transformation: JSON → Markdown");
    log::info!("  Input: {}", input_path.display());
    log::info!("  Output sessions: {}", output_dir.display());
    log::info!("  Attachments: {}", attachments_dir.display());
    log::info!("  Parallel workers: {}", parallel);

    let output_dir = absolutize(&output_dir).context("Failed to resolve sessions directory")?;
    let attachments_dir =
        absolutize(&attachments_dir).context("Failed to resolve attachments directory")?;

    // Read JSON export
    log::info!("Reading export data...");
    let file = fs::File::open(&input_path).context("Failed to open input file")?;
    let export_data: ExportData = serde_json::from_reader(file).context("Failed to parse JSON")?;

    log::info!("  Found {} sessions", export_data.sessions.len());

    let migration_root = migration_root(&output_dir)?;
    fs::create_dir_all(&output_dir).context("Failed to create sessions directory")?;
    fs::create_dir_all(&attachments_dir).context("Failed to create attachments directory")?;
    let canonical_root =
        fs::canonicalize(&migration_root).context("Failed to resolve migration root")?;
    let canonical_attachments =
        fs::canonicalize(&attachments_dir).context("Failed to resolve attachments directory")?;
    if !canonical_attachments.starts_with(&canonical_root) {
        anyhow::bail!(
            "attachments directory resolves outside migration root {}",
            migration_root.display()
        );
    }
    if !attachments_dir.starts_with(&migration_root) {
        anyhow::bail!(
            "attachments directory must be inside migration root {}",
            migration_root.display()
        );
    };

    // Configure thread pool
    rayon::ThreadPoolBuilder::new()
        .num_threads(parallel)
        .build_global()
        .ok();

    // Transform sessions in parallel
    log::info!("Transforming sessions...");

    let results: Vec<Result<()>> = export_data
        .sessions
        .par_iter()
        .enumerate()
        .map(|(idx, session)| {
            if idx % 10 == 0 {
                log::info!("  Progress: {}/{}", idx, export_data.sessions.len());
            }
            transform_session(
                session,
                &output_dir,
                &attachments_dir,
                &migration_root,
                overwrite,
            )
        })
        .collect();

    // Check for errors
    let mut error_count = 0;
    for (idx, result) in results.iter().enumerate() {
        if let Err(e) = result {
            log::error!(
                "Session {} failed: {}",
                export_data.sessions[idx].session_id,
                e
            );
            error_count += 1;
        }
    }

    if error_count > 0 {
        anyhow::bail!("{} sessions failed to transform", error_count);
    }

    log::info!("✓ Transformation completed successfully");
    log::info!("  Total sessions: {}", export_data.sessions.len());
    log::info!("  Output directory: {}", output_dir.display());

    Ok(())
}

fn transform_session(
    session: &ExportSession,
    output_dir: &Path,
    attachments_dir: &Path,
    migration_root: &Path,
    overwrite: bool,
) -> Result<()> {
    let session_dir = output_dir.join(session_dir_name(&session.session_id)?);
    let existed = session_dir.exists();
    if existed && !overwrite {
        anyhow::bail!(
            "Session directory already exists: {}. Use --overwrite to replace.",
            session_dir.display()
        );
    }

    let staging = tempfile::Builder::new()
        .prefix(".session-staging-")
        .tempdir_in(output_dir)
        .context("Failed to create temporary session directory")?;
    let result: Result<()> = (|| {
        write_transcript(staging.path(), session)?;
        write_tool_outputs(staging.path(), session)?;
        write_metadata(staging.path(), session)?;

        // Store attachments and persist a manifest so the indexer can reconstruct
        // attachment rows without reopening the legacy database.
        let mut attachment_manifest = Vec::with_capacity(session.attachments.len());
        for attachment in &session.attachments {
            attachment_manifest.push(store_attachment(
                attachment,
                attachments_dir,
                migration_root,
            )?);
        }
        write_attachment_manifest(staging.path(), &attachment_manifest)?;
        Ok(())
    })();
    result?;

    if !existed {
        fs::rename(staging.path(), &session_dir).context("Failed to publish transformed session")?;
        return Ok(());
    }

    let backup = tempfile::Builder::new()
        .prefix(".session-backup-")
        .tempdir_in(output_dir)
        .context("Failed to create session backup directory")?;
    let backup_path = backup.path().join("previous");
    fs::rename(&session_dir, &backup_path).context("Failed to back up existing session")?;
    if let Err(error) = fs::rename(staging.path(), &session_dir) {
        if let Err(restore_error) = fs::rename(&backup_path, &session_dir) {
            let retained_backup = backup.keep();
            anyhow::bail!(
                "Failed to publish transformed session: {error}; \
                 failed to restore previous session: {restore_error}; backup retained at {}",
                retained_backup.display()
            );
        }
        return Err(error)
            .context("Failed to publish transformed session; previous session restored");
    }
    Ok(())
}

fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

fn migration_root(output_dir: &Path) -> Result<PathBuf> {
    let parent = output_dir
        .parent()
        .context("Output sessions directory must have a parent migration root")?;
    let root = if parent.file_name().and_then(|name| name.to_str()) == Some("raw") {
        parent
            .parent()
            .map(Path::to_path_buf)
            .context("raw sessions directory must have a migration root")?
    } else {
        parent.to_path_buf()
    };
    Ok(root)
}

#[derive(Debug, Serialize, Deserialize)]
struct TransformedAttachment {
    attachment_id: String,
    content_hash: String,
    mime_type: String,
    size_bytes: i64,
    /// Relative to the migration root (the parent of `raw/sessions`).
    file_path: String,
}

fn write_attachment_manifest(
    session_dir: &Path,
    attachments: &[TransformedAttachment],
) -> Result<()> {
    let path = session_dir.join("attachments.json");
    let file = fs::File::create(path).context("Failed to create attachments.json")?;
    serde_json::to_writer_pretty(file, attachments).context("Failed to write attachments.json")?;
    Ok(())
}

fn write_transcript(session_dir: &Path, session: &ExportSession) -> Result<()> {
    let transcript_path = session_dir.join("transcript.md");
    let mut content = String::new();

    // Header
    content.push_str(&format!("# Session {}\n\n", session.session_id));
    content.push_str(&format!(
        "Started: {}\n\n",
        format_timestamp(session.started_at).with_context(|| {
            format!(
                "Invalid started_at timestamp for session {}",
                session.session_id
            )
        })?
    ));

    // Messages
    for msg in &session.messages {
        content.push_str(&format!(
            "## {} - {}\n\n",
            capitalize(&msg.role),
            format_timestamp(msg.timestamp).with_context(|| {
                format!(
                    "Invalid message timestamp for session {}",
                    session.session_id
                )
            })?
        ));
        content.push_str(&msg.content);
        content.push_str("\n\n");
    }

    fs::write(&transcript_path, content).context("Failed to write transcript.md")?;

    Ok(())
}

fn write_tool_outputs(session_dir: &Path, session: &ExportSession) -> Result<()> {
    let tool_outputs_path = session_dir.join("tool_outputs.md");
    let mut content = String::new();

    content.push_str(&format!(
        "# Tool Outputs - Session {}\n\n",
        session.session_id
    ));

    for output in &session.tool_outputs {
        content.push_str(&format!(
            "## {} - {}\n\n",
            output.tool_name,
            format_timestamp(output.timestamp).with_context(|| {
                format!(
                    "Invalid tool output timestamp for session {}",
                    session.session_id
                )
            })?
        ));

        content.push_str(&format!("**Args**: `{}`\n\n", output.args));

        if let Some(exit_code) = output.exit_code {
            content.push_str(&format!("**Exit Code**: {}\n\n", exit_code));
        }

        if let Some(ref stdout) = output.stdout {
            content.push_str("### stdout\n\n```\n");
            content.push_str(stdout);
            content.push_str("\n```\n\n");
        }

        if let Some(ref stderr) = output.stderr {
            content.push_str("### stderr\n\n```\n");
            content.push_str(stderr);
            content.push_str("\n```\n\n");
        }

        if let Some(duration) = output.duration_ms {
            content.push_str(&format!("**Duration**: {}ms\n\n", duration));
        }
    }

    fs::write(&tool_outputs_path, content).context("Failed to write tool_outputs.md")?;

    Ok(())
}

#[derive(Serialize, Deserialize)]
struct SessionMetadata {
    session_id: String,
    agent_id: String,
    workspace_id: Option<String>,
    started_at: i64,
    ended_at: Option<i64>,
    tags: Vec<String>,
    compression_status: String,
    message_count: usize,
    tool_call_count: usize,
    attachment_count: usize,
}

fn write_metadata(session_dir: &Path, session: &ExportSession) -> Result<()> {
    let metadata_path = session_dir.join("metadata.json");

    let metadata = SessionMetadata {
        session_id: session.session_id.clone(),
        agent_id: session.agent_id.clone(),
        workspace_id: session.workspace_id.clone(),
        started_at: session.started_at,
        ended_at: session.ended_at,
        tags: session.tags.clone(),
        compression_status: session.compression_status.clone(),
        message_count: session.messages.len(),
        tool_call_count: session.tool_outputs.len(),
        attachment_count: session.attachments.len(),
    };

    let file = fs::File::create(&metadata_path).context("Failed to create metadata.json")?;
    serde_json::to_writer_pretty(file, &metadata).context("Failed to write metadata.json")?;

    Ok(())
}

fn store_attachment(
    attachment: &crate::export::ExportAttachment,
    attachments_dir: &Path,
    migration_root: &Path,
) -> Result<TransformedAttachment> {
    // Decode base64 if present
    let data = match &attachment.data_base64 {
        Some(b64) => {
            use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
            BASE64
                .decode(b64)
                .context("Failed to decode attachment data")?
        }
        None => anyhow::bail!(
            "Attachment {} has no exported bytes",
            attachment.attachment_id
        ),
    };
    if attachment.size_bytes >= 0 && attachment.size_bytes != data.len() as i64 {
        anyhow::bail!(
            "Attachment {} size mismatch: export says {}, decoded {}",
            attachment.attachment_id,
            attachment.size_bytes,
            data.len()
        );
    }

    // Compute SHA256 hash
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let hash = hex::encode(hasher.finalize());
    let hash_prefix = &hash[..16];

    // Determine directory by MIME type
    let subdir = match attachment.mime_type.split('/').next() {
        Some("image") => "images",
        Some("video") => "videos",
        Some("audio") => "audio",
        _ => "documents",
    };

    // Determine extension
    let ext = mime_to_extension(&attachment.mime_type);

    // Create target directory
    let target_dir = attachments_dir.join(subdir);
    fs::create_dir_all(&target_dir)?;

    // Write file
    let filename = format!("{}.{}", hash_prefix, ext);
    let target_path = target_dir.join(&filename);

    if target_path.exists() {
        validate_existing_attachment(&target_path, attachments_dir, data.len(), &hash)?;
    } else {
        // Verify the parent before staging, then publish without clobbering a
        // file another session may have completed concurrently.
        let canonical_dir = fs::canonicalize(&target_dir)
            .context("Failed to resolve attachments subdirectory")?;
        let canonical_attachments =
            fs::canonicalize(attachments_dir).context("Failed to resolve attachments directory")?;
        if !canonical_dir.starts_with(&canonical_attachments) {
            anyhow::bail!(
                "attachments subdirectory resolves outside attachments directory: {}",
                target_dir.display()
            );
        }
        let mut temp = tempfile::Builder::new()
            .prefix(".attachment-staging-")
            .tempfile_in(&target_dir)
            .context("Failed to create temporary attachment file")?;
        temp.write_all(&data)
            .context("Failed to write attachment file")?;
        temp.as_file_mut()
            .sync_all()
            .context("Failed to sync attachment file")?;
        verify_attachment_contents(temp.path(), data.len(), &hash)?;
        match temp.persist_noclobber(&target_path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                drop(error.file);
                validate_existing_attachment(&target_path, attachments_dir, data.len(), &hash)?;
            }
            Err(error) => {
                return Err(error.error).context("Failed to publish attachment file");
            }
        }
    }

    Ok(TransformedAttachment {
        attachment_id: attachment.attachment_id.clone(),
        content_hash: hash,
        mime_type: attachment.mime_type.clone(),
        size_bytes: data.len() as i64,
        file_path: target_path
            .strip_prefix(migration_root)
            .context("Attachment path escaped migration root")?
            .to_string_lossy()
            .replace('\\', "/"),
    })
}

fn validate_existing_attachment(
    target_path: &Path,
    attachments_dir: &Path,
    expected_size: usize,
    expected_hash: &str,
) -> Result<()> {
    let canonical_target =
        fs::canonicalize(target_path).context("Failed to resolve existing attachment file")?;
    let canonical_attachments =
        fs::canonicalize(attachments_dir).context("Failed to resolve attachments directory")?;
    if !canonical_target.starts_with(&canonical_attachments) {
        anyhow::bail!(
            "existing attachment path resolves outside attachments directory: {}",
            target_path.display()
        );
    }
    verify_attachment_contents(target_path, expected_size, expected_hash)
}

fn verify_attachment_contents(
    path: &Path,
    expected_size: usize,
    expected_hash: &str,
) -> Result<()> {
    let mut file = fs::File::open(path).context("Failed to open attachment for verification")?;
    let actual_size = file
        .metadata()
        .context("Failed to inspect attachment for verification")?
        .len();
    if actual_size != expected_size as u64 {
        anyhow::bail!(
            "Attachment {} size mismatch: expected {}, found {}",
            path.display(),
            expected_size,
            actual_size
        );
    }
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let bytes_read = file
            .read(&mut buffer)
            .context("Failed to read attachment for verification")?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != expected_hash {
        anyhow::bail!("Attachment {} hash mismatch", path.display());
    }
    Ok(())
}

fn format_timestamp(ts: i64) -> Result<String> {
    use chrono::{DateTime, Utc};
    let dt: DateTime<Utc> = DateTime::from_timestamp(ts, 0)
        .with_context(|| format!("Timestamp is out of range: {ts}"))?;
    Ok(dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn mime_to_extension(mime_type: &str) -> &str {
    match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/markdown" => "md",
        _ => "bin",
    }
}
