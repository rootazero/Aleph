//! Export module: SQLite → JSON

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportData {
    pub version: String,
    pub exported_at: i64,
    pub sessions: Vec<ExportSession>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportSession {
    pub session_id: String,
    pub agent_id: String,
    pub workspace_id: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub messages: Vec<ExportMessage>,
    pub tool_outputs: Vec<ExportToolOutput>,
    pub attachments: Vec<ExportAttachment>,
    pub tags: Vec<String>,
    pub compression_status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportMessage {
    pub timestamp: i64,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportToolOutput {
    pub timestamp: i64,
    pub tool_name: String,
    pub args: String,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportAttachment {
    pub attachment_id: String,
    pub mime_type: String,
    pub size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
}

// 1:1 with `migrate Export` CLI fields — keeping them as positional args
// preserves the destructured pattern in main.rs without forcing a builder
// here for a one-shot batch tool. Bump the threshold (or pack into a
// `ExportConfig` struct) before adding the ninth option.
#[allow(clippy::too_many_arguments)]
pub fn run_export(
    db_path: PathBuf,
    output_path: PathBuf,
    agent_id: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
    compression_status: Option<String>,
    include_attachments: bool,
    force: bool,
) -> Result<()> {
    log::info!("Starting export from SQLite database...");
    log::info!("  Database: {}", db_path.display());
    log::info!("  Output: {}", output_path.display());

    if !db_path.exists() {
        anyhow::bail!("Database file not found: {}", db_path.display());
    }
    if output_path.exists() && !force {
        anyhow::bail!(
            "Output file already exists: {}. Use --force to overwrite.",
            output_path.display()
        );
    }

    let conn = rusqlite::Connection::open(&db_path).context("Failed to open SQLite database")?;
    let columns = table_columns(&conn, "raw_memories")?;
    if !columns.contains("session_id")
        || !columns.contains("agent_id")
        || !columns.contains("content")
    {
        anyhow::bail!(
            "raw_memories is missing one of the required columns: session_id, agent_id, content"
        );
    }
    let timestamp_column = if columns.contains("created_at") {
        "created_at"
    } else if columns.contains("started_at") {
        "started_at"
    } else {
        anyhow::bail!("raw_memories is missing a timestamp column (created_at or started_at)");
    };

    let workspace_expr = optional_column(&columns, "workspace_id");
    let ended_expr = optional_column(&columns, "ended_at");
    let status_expr = optional_column(&columns, "compression_status");
    let tags_expr = optional_column(&columns, "tags");
    // The legacy store records session rows under several source values
    // (transcript, tool_invocation, session_compressed, and others). The
    // durable session boundary is session_id, so filtering by a synthetic
    // source = 'session' would silently export nothing from real databases.
    let source_clause = " WHERE session_id IS NOT NULL";

    let mut query = format!(
        "SELECT session_id, agent_id, {workspace_expr}, {timestamp_column}, {ended_expr}, content, {status_expr}, {tags_expr} FROM raw_memories{source_clause}"
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(ref agent) = agent_id {
        query.push_str(" AND agent_id = ?");
        params.push(Box::new(agent.clone()));
    }
    if let Some(ref start) = start_date {
        query.push_str(&format!(" AND {timestamp_column} >= ?"));
        params.push(Box::new(parse_date(start)?));
    }
    if let Some(ref end) = end_date {
        query.push_str(&format!(" AND {timestamp_column} <= ?"));
        params.push(Box::new(parse_date(end)?));
    }
    if let Some(ref status) = compression_status {
        if !columns.contains("compression_status") {
            anyhow::bail!("Cannot filter compression_status: raw_memories has no such column");
        }
        query.push_str(" AND compression_status = ?");
        params.push(Box::new(status.clone()));
    }
    query.push_str(&format!(" ORDER BY {timestamp_column} ASC"));

    log::info!("Querying sessions...");
    let mut stmt = conn.prepare(&query)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        let session_id: Option<String> = row.get(0)?;
        let agent_id: Option<String> = row.get(1)?;
        Ok((
            session_id,
            agent_id,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;

    let mut grouped: BTreeMap<String, ExportSession> = BTreeMap::new();
    for row in rows {
        let (session_id, agent, workspace_id, created_at, ended_at, content, status, tags) = row?;
        let Some(session_id) = session_id else {
            log::warn!("Skipping raw memory row without session_id");
            continue;
        };
        let agent_id = agent.unwrap_or_else(|| "unknown".to_string());
        let entry = grouped
            .entry(session_id.clone())
            .or_insert_with(|| ExportSession {
                session_id,
                agent_id: agent_id.clone(),
                workspace_id: workspace_id.clone(),
                started_at: created_at,
                ended_at,
                messages: Vec::new(),
                tool_outputs: Vec::new(),
                attachments: Vec::new(),
                tags: tags.as_deref().map(parse_tags).unwrap_or_default(),
                compression_status: status.unwrap_or_else(|| "pending".to_string()),
            });
        entry.started_at = entry.started_at.min(created_at);
        entry.ended_at = entry.ended_at.or(ended_at);
        entry
            .messages
            .extend(parse_messages_from_content(&content, created_at)?);
        entry
            .tool_outputs
            .extend(parse_tool_outputs_from_content(&content)?);
    }
    drop(stmt);

    let mut export_sessions: Vec<_> = grouped.into_values().collect();
    for session in &mut export_sessions {
        if include_attachments {
            session.attachments = load_attachments(&conn, &db_path, &session.session_id)?;
        }
    }

    log::info!("Exported {} sessions total", export_sessions.len());
    let export_data = ExportData {
        version: "1.0".to_string(),
        exported_at: chrono::Utc::now().timestamp(),
        sessions: export_sessions,
    };

    let file = std::fs::File::create(&output_path).context("Failed to create output file")?;
    serde_json::to_writer_pretty(file, &export_data).context("Failed to write JSON")?;
    log::info!("✓ Export completed successfully");
    Ok(())
}

pub fn session_dir_name(session_id: &str) -> Result<String> {
    if session_id.is_empty() {
        anyhow::bail!("session_id must not be empty");
    }

    // Session IDs are opaque values and commonly contain `:` on Windows.
    // Encode every byte outside a conservative portable filename alphabet so
    // the same export can be transformed on Windows, macOS, and Linux.
    let mut encoded = String::with_capacity(session_id.len());
    for byte in session_id.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            encoded.push(*byte as char);
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }

    // Avoid Windows device names even when the original ID contains only
    // portable characters.
    let upper = encoded.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && matches!(&upper[..3], "COM" | "LPT")
            && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        encoded.insert_str(0, "%00");
    }
    Ok(encoded)
}

fn table_columns(conn: &rusqlite::Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<HashSet<_>, _>>()?;
    if columns.is_empty() {
        anyhow::bail!("SQLite table not found or has no columns: {table}");
    }
    Ok(columns)
}

fn optional_column(columns: &HashSet<String>, name: &'static str) -> &'static str {
    if columns.contains(name) {
        name
    } else {
        "NULL"
    }
}

fn parse_tags(value: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(value).unwrap_or_else(|_| {
        value
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect()
    })
}

fn parse_date(date_str: &str) -> Result<i64> {
    use chrono::NaiveDate;
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .context("Invalid date format. Use YYYY-MM-DD")?;
    Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
}

fn parse_messages_from_content(content: &str, timestamp: i64) -> Result<Vec<ExportMessage>> {
    // Legacy raw_memories stores an opaque content blob. Preserve it as one
    // user message and preserve the source row timestamp; inventing assistant
    // turns or tool boundaries would create false history.
    Ok(vec![ExportMessage {
        timestamp,
        role: "user".to_string(),
        content: content.to_string(),
    }])
}

fn parse_tool_outputs_from_content(_content: &str) -> Result<Vec<ExportToolOutput>> {
    // No structured tool-output columns exist in the legacy raw-memory schema.
    Ok(vec![])
}

fn load_attachments(
    conn: &rusqlite::Connection,
    db_path: &Path,
    session_id: &str,
) -> Result<Vec<ExportAttachment>> {
    let columns = match table_columns(conn, "attachments") {
        Ok(columns) => columns,
        Err(_) => return Ok(vec![]),
    };
    for required in ["attachment_id", "mime_type", "size_bytes"] {
        if !columns.contains(required) {
            anyhow::bail!("attachments is missing required column: {required}");
        }
    }
    let has_data = columns.contains("data");
    let has_file_path = columns.contains("file_path");
    if !has_data && !has_file_path {
        anyhow::bail!("attachments has neither data nor file_path column");
    }

    let payload_expr = if has_data { "data" } else { "NULL" };
    let path_expr = if has_file_path { "file_path" } else { "NULL" };
    let query = format!(
        "SELECT attachment_id, mime_type, size_bytes, {payload_expr}, {path_expr} FROM attachments WHERE session_id = ?"
    );
    let mut stmt = conn.prepare(&query)?;
    let base_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let attachments = stmt
        .query_map([session_id], |row| {
            let data: Option<Vec<u8>> = row.get(3)?;
            let file_path: Option<String> = row.get(4)?;
            let data = match data {
                Some(data) => Some(data),
                None => match file_path {
                    Some(path) => {
                        let path = PathBuf::from(path);
                        let resolved = if path.is_absolute() {
                            path
                        } else {
                            base_dir.join(path)
                        };
                        Some(std::fs::read(resolved).map_err(|error| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                        })?)
                    }
                    None => None,
                },
            };
            Ok(ExportAttachment {
                attachment_id: row.get(0)?,
                mime_type: row.get(1)?,
                size_bytes: row.get(2)?,
                data_base64: data.map(|bytes| BASE64.encode(bytes)),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(attachments)
}
