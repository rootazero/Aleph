//! Transform module: JSON → Markdown

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::export::{ExportData, ExportSession};

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
    validate_session_id(&session.session_id)?;
    let session_dir = output_dir.join(&session.session_id);
    let existed = session_dir.exists();

    let result = (|| {
        // Check if session exists
        if existed && !overwrite {
            anyhow::bail!(
                "Session directory already exists: {}. Use --overwrite to replace.",
                session_dir.display()
            );
        }

        // Create session directory
        fs::create_dir_all(&session_dir)?;

        // Write transcript.md
        write_transcript(&session_dir, session)?;

        // Write tool_outputs.md
        write_tool_outputs(&session_dir, session)?;

        // Write metadata.json
        write_metadata(&session_dir, session)?;

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
        write_attachment_manifest(&session_dir, &attachment_manifest)?;
        Ok(())
    })();

    if result.is_err() && !existed {
        let _ = fs::remove_dir_all(&session_dir);
    }
    result
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

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || Path::new(session_id).is_absolute()
        || Path::new(session_id).components().count() != 1
    {
        anyhow::bail!("Invalid session_id for filesystem path: {session_id:?}");
    }
    Ok(())
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
        format_timestamp(session.started_at)
    ));

    // Messages
    for msg in &session.messages {
        content.push_str(&format!(
            "## {} - {}\n\n",
            capitalize(&msg.role),
            format_timestamp(msg.timestamp)
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
            format_timestamp(output.timestamp)
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

    // Skip if already exists (content-addressed deduplication)
    if target_path.exists() {
        let canonical_target =
            fs::canonicalize(&target_path).context("Failed to resolve existing attachment file")?;
        let canonical_attachments =
            fs::canonicalize(attachments_dir).context("Failed to resolve attachments directory")?;
        if !canonical_target.starts_with(&canonical_attachments) {
            anyhow::bail!(
                "existing attachment path resolves outside attachments directory: {}",
                target_path.display()
            );
        }
    } else {
        fs::write(&target_path, &data).context("Failed to write attachment file")?;
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

fn format_timestamp(ts: i64) -> String {
    use chrono::{DateTime, Utc};
    let dt: DateTime<Utc> = DateTime::from_timestamp(ts, 0).unwrap_or_else(|| Utc::now());
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
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
