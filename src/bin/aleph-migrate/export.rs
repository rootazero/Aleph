//! Export module: SQLite → JSON

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

    // Check if database exists
    if !db_path.exists() {
        anyhow::bail!("Database file not found: {}", db_path.display());
    }

    // Check if output exists and force flag
    if output_path.exists() && !force {
        anyhow::bail!(
            "Output file already exists: {}. Use --force to overwrite.",
            output_path.display()
        );
    }

    // Open database connection
    let conn = rusqlite::Connection::open(&db_path).context("Failed to open SQLite database")?;

    // Build SQL query with filters
    let mut query = String::from(
        "SELECT session_id, agent_id, workspace_id, started_at, ended_at, \
         content, compression_status, tags FROM raw_memories WHERE source = 'session'",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![];

    if let Some(ref agent) = agent_id {
        query.push_str(" AND agent_id = ?");
        params.push(Box::new(agent.clone()));
    }

    if let Some(ref start) = start_date {
        let timestamp = parse_date(start)?;
        query.push_str(" AND started_at >= ?");
        params.push(Box::new(timestamp));
    }

    if let Some(ref end) = end_date {
        let timestamp = parse_date(end)?;
        query.push_str(" AND started_at <= ?");
        params.push(Box::new(timestamp));
    }

    if let Some(ref status) = compression_status {
        query.push_str(" AND compression_status = ?");
        params.push(Box::new(status.clone()));
    }

    query.push_str(" ORDER BY started_at ASC");

    log::info!("Querying sessions...");

    // Execute query
    let mut stmt = conn.prepare(&query)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let sessions_iter = stmt.query_map(param_refs.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,         // session_id
            row.get::<_, String>(1)?,         // agent_id
            row.get::<_, Option<String>>(2)?, // workspace_id
            row.get::<_, i64>(3)?,            // started_at
            row.get::<_, Option<i64>>(4)?,    // ended_at
            row.get::<_, String>(5)?,         // content
            row.get::<_, String>(6)?,         // compression_status
            row.get::<_, Option<String>>(7)?, // tags
        ))
    })?;

    let mut export_sessions = Vec::new();
    let mut session_count = 0;

    for session_result in sessions_iter {
        let (session_id, agent_id, workspace_id, started_at, ended_at, content, status, tags_str) =
            session_result?;

        session_count += 1;

        // Parse content (assume JSON structure for now)
        let messages = parse_messages_from_content(&content)?;
        let tool_outputs = parse_tool_outputs_from_content(&content)?;

        // Parse tags
        let tags: Vec<String> = tags_str
            .as_ref()
            .map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
            .unwrap_or_default();

        // Load attachments if requested
        let attachments = if include_attachments {
            load_attachments(&conn, &session_id)?
        } else {
            vec![]
        };

        export_sessions.push(ExportSession {
            session_id,
            agent_id,
            workspace_id,
            started_at,
            ended_at,
            messages,
            tool_outputs,
            attachments,
            tags,
            compression_status: status,
        });

        if session_count % 10 == 0 {
            log::info!("  Exported {} sessions...", session_count);
        }
    }

    log::info!("Exported {} sessions total", session_count);

    // Create export data
    let export_data = ExportData {
        version: "1.0".to_string(),
        exported_at: chrono::Utc::now().timestamp(),
        sessions: export_sessions,
    };

    // Write JSON
    log::info!("Writing JSON to {}...", output_path.display());
    let file = std::fs::File::create(&output_path).context("Failed to create output file")?;
    serde_json::to_writer_pretty(file, &export_data).context("Failed to write JSON")?;

    log::info!("✓ Export completed successfully");
    log::info!("  Total sessions: {}", session_count);
    log::info!("  Output file: {}", output_path.display());

    Ok(())
}

fn parse_date(date_str: &str) -> Result<i64> {
    use chrono::NaiveDate;

    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .context("Invalid date format. Use YYYY-MM-DD")?;

    Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
}

fn parse_messages_from_content(content: &str) -> Result<Vec<ExportMessage>> {
    // For now, assume content is a simple text blob
    // In production, parse actual stored format

    // Placeholder: split by common patterns
    let messages = vec![ExportMessage {
        timestamp: chrono::Utc::now().timestamp(),
        role: "user".to_string(),
        content: content.to_string(),
    }];

    Ok(messages)
}

fn parse_tool_outputs_from_content(_content: &str) -> Result<Vec<ExportToolOutput>> {
    // Placeholder: extract tool outputs from content
    Ok(vec![])
}

fn load_attachments(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<ExportAttachment>> {
    let mut stmt = conn.prepare(
        "SELECT attachment_id, mime_type, size_bytes, data \
         FROM attachments WHERE session_id = ?",
    )?;

    let attachments = stmt
        .query_map([session_id], |row| {
            let data: Option<Vec<u8>> = row.get(3)?;
            let data_base64 = data.map(|bytes| base64::encode(&bytes));

            Ok(ExportAttachment {
                attachment_id: row.get(0)?,
                mime_type: row.get(1)?,
                size_bytes: row.get(2)?,
                data_base64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(attachments)
}
