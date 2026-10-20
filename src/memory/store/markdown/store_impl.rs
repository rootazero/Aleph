//! Markdown-based session storage implementation.

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;

use crate::error::AlephError;
use crate::memory::store::session::*;
use crate::memory::store::sqlite::schema::session_ddl;

use super::utils::{
    append_markdown, atomic_write_markdown, compute_sha256, format_timestamp,
    format_timestamp_full, hash_prefix,
};

/// Markdown-based session storage implementation.
pub struct MarkdownSessionStore {
    base_dir: PathBuf,
    db: Arc<Mutex<Connection>>,
}

impl MarkdownSessionStore {
    /// Create a new Markdown session store.
    ///
    /// This will:
    /// 1. Create the base directory if it doesn't exist
    /// 2. Create or open `metadata.db` at `~/.aleph/metadata.db`
    /// 3. Initialize all tables
    pub async fn new(base_dir: &Path) -> Result<Self, AlephError> {
        // Database at ~/.aleph/metadata.db (NOT ~/Library/Application Support/)
        let db_path = base_dir.join("metadata.db");

        // Create directory structure
        fs::create_dir_all(base_dir).await.map_err(|e| {
            AlephError::other(format!(
                "Failed to create base directory {:?}: {}",
                base_dir, e
            ))
        })?;

        fs::create_dir_all(base_dir.join("raw/sessions"))
            .await
            .map_err(|e| {
                AlephError::other(format!("Failed to create sessions directory: {}", e))
            })?;

        fs::create_dir_all(base_dir.join("raw/attachments"))
            .await
            .map_err(|e| {
                AlephError::other(format!("Failed to create attachments directory: {}", e))
            })?;

        fs::create_dir_all(base_dir.join("archive"))
            .await
            .map_err(|e| AlephError::other(format!("Failed to create archive directory: {}", e)))?;

        // Open database connection
        let conn = Connection::open(&db_path).map_err(|e| {
            AlephError::other(format!("Failed to open database {:?}: {}", db_path, e))
        })?;

        // Initialize tables
        session_ddl::init_session_tables(&conn)?;

        Ok(Self {
            base_dir: base_dir.to_path_buf(),
            db: Arc::new(Mutex::new(conn)),
        })
    }

    /// Get the session directory path.
    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.base_dir.join("raw/sessions").join(session_id)
    }

    /// Get the attachment file path for a given hash and MIME type.
    fn attachment_path(&self, hash: &str, mime_type: &str) -> PathBuf {
        let ext = mime_type_to_ext(mime_type);
        let prefix = hash_prefix(hash);
        let type_dir = mime_type_to_dir(mime_type);

        self.base_dir
            .join("raw/attachments")
            .join(type_dir)
            .join(format!("{}.{}", prefix, ext))
    }

    /// Write initial metadata.json for a session.
    async fn write_metadata(&self, metadata: &SessionMetadata) -> Result<(), AlephError> {
        let session_dir = self.session_dir(&metadata.session_id);
        let metadata_path = session_dir.join("metadata.json");

        let json = serde_json::to_string_pretty(metadata)
            .map_err(|e| AlephError::other(format!("Failed to serialize metadata: {}", e)))?;

        atomic_write_markdown(&metadata_path, &json).await
    }

    /// Read metadata.json for a session.
    async fn read_metadata(&self, session_id: &str) -> Result<SessionMetadata, AlephError> {
        let metadata_path = self.session_dir(session_id).join("metadata.json");

        let content = fs::read_to_string(&metadata_path).await.map_err(|e| {
            AlephError::other(format!(
                "Failed to read metadata {:?}: {}",
                metadata_path, e
            ))
        })?;

        serde_json::from_str(&content)
            .map_err(|e| AlephError::other(format!("Failed to parse metadata: {}", e)))
    }

    /// Update metadata.json for a session.
    async fn update_metadata(&self, metadata: &SessionMetadata) -> Result<(), AlephError> {
        self.write_metadata(metadata).await
    }
}

#[async_trait]
impl RawSessionStore for MarkdownSessionStore {
    async fn create_session(&self, metadata: SessionMetadata) -> Result<(), AlephError> {
        let session_dir = self.session_dir(&metadata.session_id);
        let metadata_path = session_dir.join("metadata.json");

        // RunStarted can be replayed. A fully initialized session must remain
        // untouched so replay never truncates an existing transcript.
        let already_exists = if metadata_path.exists() {
            let db = self.db.lock().await;
            db.query_row(
                "SELECT 1 FROM sessions WHERE session_id = ? LIMIT 1",
                [metadata.session_id.as_str()],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| AlephError::other(format!("Failed to check existing session: {}", e)))?
            .is_some()
        } else {
            false
        };
        if already_exists {
            let mut existing = self.read_metadata(&metadata.session_id).await?;

            // A UserMessage can precede RunStarted in the event stream. In that
            // case ensure_session creates a provisional row without a workspace;
            // merge the authoritative RunStarted scope without rewriting raw
            // transcript files. Conflicting scopes indicate session-key reuse
            // and must fail closed.
            if existing.agent_id != metadata.agent_id {
                return Err(AlephError::other(format!(
                    "Session {} is already owned by agent {}",
                    metadata.session_id, existing.agent_id
                )));
            }
            if let (Some(existing_workspace), Some(new_workspace)) =
                (&existing.workspace_id, &metadata.workspace_id)
            {
                if existing_workspace != new_workspace {
                    return Err(AlephError::other(format!(
                        "Session {} is already bound to workspace {}",
                        metadata.session_id, existing_workspace
                    )));
                }
            }

            if existing.workspace_id.is_none() && metadata.workspace_id.is_some() {
                existing.workspace_id = metadata.workspace_id.clone();

                let db = self.db.lock().await;
                if let Some(workspace_id) = existing.workspace_id.as_ref() {
                    db.execute(
                        "INSERT OR IGNORE INTO workspaces (workspace_id, name, created_at) VALUES (?, ?, ?)",
                        rusqlite::params![workspace_id, workspace_id, metadata.started_at],
                    )
                    .map_err(|e| {
                        AlephError::other(format!("Failed to insert workspace: {}", e))
                    })?;
                }
                db.execute(
                    "UPDATE sessions SET workspace_id = ?, updated_at = ? WHERE session_id = ?",
                    rusqlite::params![
                        existing.workspace_id,
                        metadata.started_at,
                        metadata.session_id
                    ],
                )
                .map_err(|e| AlephError::other(format!("Failed to update session scope: {}", e)))?;
                drop(db);

                self.update_metadata(&existing).await?;
            }
            return Ok(());
        }

        // Create session directory
        fs::create_dir_all(&session_dir).await.map_err(|e| {
            AlephError::other(format!(
                "Failed to create session directory {:?}: {}",
                session_dir, e
            ))
        })?;

        // Create empty transcript and tool outputs files
        let transcript_path = session_dir.join("transcript.md");
        let tool_outputs_path = session_dir.join("tool_outputs.md");

        fs::write(
            &transcript_path,
            format!(
                "# Session {}\n\nStarted: {}\n\n",
                metadata.session_id,
                format_timestamp_full(metadata.started_at)
            ),
        )
        .await
        .map_err(|e| {
            AlephError::other(format!(
                "Failed to create transcript {:?}: {}",
                transcript_path, e
            ))
        })?;

        fs::write(
            &tool_outputs_path,
            format!("# Tool Outputs - Session {}\n\n", metadata.session_id),
        )
        .await
        .map_err(|e| {
            AlephError::other(format!(
                "Failed to create tool outputs {:?}: {}",
                tool_outputs_path, e
            ))
        })?;

        // Write metadata.json
        self.write_metadata(&metadata).await?;

        // Insert into database
        let db = self.db.lock().await;
        db.execute(
            "INSERT OR IGNORE INTO agents (agent_id, created_at, last_active_at) VALUES (?, ?, ?)",
            rusqlite::params![metadata.agent_id, metadata.started_at, metadata.started_at],
        )
        .map_err(|e| AlephError::other(format!("Failed to insert agent: {}", e)))?;

        if let Some(ref workspace_id) = metadata.workspace_id {
            db.execute(
                "INSERT OR IGNORE INTO workspaces (workspace_id, name, created_at) VALUES (?, ?, ?)",
                rusqlite::params![workspace_id, workspace_id, metadata.started_at],
            )
            .map_err(|e| AlephError::other(format!("Failed to insert workspace: {}", e)))?;
        }

        db.execute(
            "INSERT INTO sessions (session_id, agent_id, workspace_id, started_at, transcript_path, tool_outputs_path, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                metadata.session_id,
                metadata.agent_id,
                metadata.workspace_id,
                metadata.started_at,
                format!(
                    "raw/sessions/{}/transcript.md",
                    metadata.session_id
                ),
                format!(
                    "raw/sessions/{}/tool_outputs.md",
                    metadata.session_id
                ),
                metadata.started_at,
                metadata.started_at,
            ],
        )
        .map_err(|e| AlephError::other(format!("Failed to insert session: {}", e)))?;

        Ok(())
    }

    async fn append_transcript(
        &self,
        session_id: &str,
        timestamp: i64,
        role: MessageRole,
        content: &str,
    ) -> Result<(), AlephError> {
        // Serialize the file append, metadata read/modify/write, and index refresh.
        // The projector has one worker, but direct store callers can overlap.
        let mut db = self.db.lock().await;
        let mut metadata = self.read_metadata(session_id).await?;
        let transcript_path = self.session_dir(session_id).join("transcript.md");

        let entry = format!(
            "## {} - {}\n\n{}\n",
            role.as_str(),
            format_timestamp(timestamp),
            content
        );

        append_markdown(&transcript_path, &entry).await?;

        let transcript = self.read_transcript(session_id).await?;

        // Keep the SQLite count and full-transcript FTS replacement atomic.
        // Do not hold a rusqlite transaction across an await.
        let (message_count, tool_call_count) = {
            let tx = db.transaction().map_err(|e| {
                AlephError::other(format!("Failed to start transcript update: {}", e))
            })?;
            let counts: (i64, i64) = tx
                .query_row(
                    "UPDATE sessions SET message_count = message_count + 1, updated_at = ? WHERE session_id = ? \
                     RETURNING message_count, tool_call_count",
                    rusqlite::params![timestamp, session_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| AlephError::other(format!("Failed to update message count: {}", e)))?;
            tx.execute(
                "DELETE FROM session_transcripts_fts WHERE session_id = ?",
                [session_id],
            )
            .map_err(|e| AlephError::other(format!("Failed to clear transcript index: {}", e)))?;
            tx.execute(
                "INSERT INTO session_transcripts_fts (session_id, content) VALUES (?, ?)",
                rusqlite::params![session_id, transcript],
            )
            .map_err(|e| AlephError::other(format!("Failed to index transcript: {}", e)))?;
            tx.commit().map_err(|e| {
                AlephError::other(format!("Failed to commit transcript update: {}", e))
            })?;
            counts
        };

        metadata.message_count = message_count;
        metadata.tool_call_count = tool_call_count;
        self.update_metadata(&metadata).await
    }

    async fn append_tool_output(
        &self,
        session_id: &str,
        timestamp: i64,
        tool_name: &str,
        args: &str,
        result: &ToolResult,
    ) -> Result<(), AlephError> {
        let db = self.db.lock().await;
        let mut metadata = self.read_metadata(session_id).await?;
        let tool_outputs_path = self.session_dir(session_id).join("tool_outputs.md");

        let entry = format!(
            "## {} - {}\n\n**Args**: {}\n\n**Exit Code**: {:?}\n\n**Duration**: {}ms\n\n**Stdout**:\n```\n{}\n```\n\n**Stderr**:\n```\n{}\n```\n",
            tool_name,
            format_timestamp(timestamp),
            args,
            result.exit_code,
            result.duration_ms,
            result.stdout,
            result.stderr
        );

        append_markdown(&tool_outputs_path, &entry).await?;

        let (message_count, tool_call_count): (i64, i64) = db
            .query_row(
                "UPDATE sessions SET tool_call_count = tool_call_count + 1, updated_at = ? WHERE session_id = ? \
                 RETURNING message_count, tool_call_count",
                rusqlite::params![timestamp, session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| AlephError::other(format!("Failed to update tool call count: {}", e)))?;

        metadata.message_count = message_count;
        metadata.tool_call_count = tool_call_count;
        self.update_metadata(&metadata).await
    }

    async fn store_attachment(
        &self,
        session_id: &str,
        attachment: Attachment,
    ) -> Result<AttachmentRef, AlephError> {
        let hash = compute_sha256(&attachment.data);
        let attachment_path = self.attachment_path(&hash, &attachment.mime_type);

        // Create type directory
        if let Some(parent) = attachment_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                AlephError::other(format!(
                    "Failed to create attachment directory {:?}: {}",
                    parent, e
                ))
            })?;
        }

        // Write attachment file (if not exists)
        if !attachment_path.exists() {
            fs::write(&attachment_path, &attachment.data)
                .await
                .map_err(|e| {
                    AlephError::other(format!(
                        "Failed to write attachment {:?}: {}",
                        attachment_path, e
                    ))
                })?;
        }

        let attachment_id = format!(
            "att_{}_{}",
            hash_prefix(&hash),
            hash_prefix(&compute_sha256(session_id.as_bytes())),
        );
        let size_bytes = attachment.data.len() as u64;
        let uploaded_at = chrono::Utc::now().timestamp();

        // Insert into database
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO attachments (attachment_id, session_id, file_path, content_hash, mime_type, size_bytes, uploaded_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                attachment_id,
                session_id,
                attachment_path
                    .strip_prefix(&self.base_dir)
                    .unwrap_or(&attachment_path)
                    .to_string_lossy()
                    .to_string(),
                hash,
                attachment.mime_type,
                size_bytes as i64,
                uploaded_at,
            ],
        )
        .map_err(|e| AlephError::other(format!("Failed to insert attachment: {}", e)))?;

        Ok(AttachmentRef {
            attachment_id,
            file_path: attachment_path,
            content_hash: hash,
            mime_type: attachment.mime_type,
            size_bytes,
            uploaded_at,
        })
    }

    async fn finalize_session(&self, session_id: &str, ended_at: i64) -> Result<(), AlephError> {
        // Serialize with appends so finalization cannot overwrite newer counts.
        let db = self.db.lock().await;
        let mut metadata = self.read_metadata(session_id).await?;

        // Update metadata
        metadata.ended_at = Some(ended_at);
        if let Some(started) = Some(metadata.started_at) {
            metadata.duration_seconds = Some(ended_at - started);
        }

        // Compute content hash
        let transcript = self.read_transcript(session_id).await?;
        metadata.content_hash = Some(compute_sha256(transcript.as_bytes()));

        // Write updated metadata
        self.update_metadata(&metadata).await?;

        // Update database
        db.execute(
            "UPDATE sessions SET ended_at = ?, duration_seconds = ?, content_hash = ?, updated_at = ? WHERE session_id = ?",
            rusqlite::params![
                ended_at,
                metadata.duration_seconds,
                metadata.content_hash,
                ended_at,
                session_id,
            ],
        )
        .map_err(|e| AlephError::other(format!("Failed to finalize session: {}", e)))?;

        Ok(())
    }

    async fn read_transcript(&self, session_id: &str) -> Result<String, AlephError> {
        let transcript_path = self.session_dir(session_id).join("transcript.md");

        fs::read_to_string(&transcript_path).await.map_err(|e| {
            AlephError::other(format!(
                "Failed to read transcript {:?}: {}",
                transcript_path, e
            ))
        })
    }

    async fn read_tool_outputs(&self, session_id: &str) -> Result<String, AlephError> {
        let tool_outputs_path = self.session_dir(session_id).join("tool_outputs.md");

        fs::read_to_string(&tool_outputs_path).await.map_err(|e| {
            AlephError::other(format!(
                "Failed to read tool outputs {:?}: {}",
                tool_outputs_path, e
            ))
        })
    }

    async fn list_sessions(
        &self,
        agent_id: Option<&str>,
        workspace_id: Option<&str>,
        start_date: Option<i64>,
        end_date: Option<i64>,
        compression_status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SessionMetadata>, AlephError> {
        let db = self.db.lock().await;

        let mut query = "SELECT session_id, agent_id, workspace_id, started_at, ended_at, duration_seconds, \
                         transcript_path, tool_outputs_path, message_count, tool_call_count, content_hash, \
                         compression_status, tags FROM sessions WHERE 1=1".to_string();

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(aid) = agent_id {
            query.push_str(" AND agent_id = ?");
            params.push(Box::new(aid.to_string()));
        }

        if let Some(wid) = workspace_id {
            query.push_str(" AND workspace_id = ?");
            params.push(Box::new(wid.to_string()));
        }

        if let Some(start) = start_date {
            query.push_str(" AND started_at >= ?");
            params.push(Box::new(start));
        }

        if let Some(end) = end_date {
            query.push_str(" AND started_at <= ?");
            params.push(Box::new(end));
        }

        if let Some(status) = compression_status {
            query.push_str(" AND compression_status = ?");
            params.push(Box::new(status.to_string()));
        }

        query.push_str(" ORDER BY started_at DESC LIMIT ? OFFSET ?");
        params.push(Box::new(limit as i64));
        params.push(Box::new(offset as i64));

        let mut stmt = db
            .prepare(&query)
            .map_err(|e| AlephError::other(format!("Failed to prepare query: {}", e)))?;

        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(&param_refs[..], |row| {
                let tags_str: String = row.get(12)?;
                let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();

                Ok(SessionMetadata {
                    session_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    started_at: row.get(3)?,
                    ended_at: row.get(4)?,
                    duration_seconds: row.get(5)?,
                    transcript_path: PathBuf::from(row.get::<_, String>(6)?),
                    tool_outputs_path: PathBuf::from(row.get::<_, String>(7)?),
                    message_count: row.get(8)?,
                    tool_call_count: row.get(9)?,
                    content_hash: row.get(10)?,
                    compression_status: CompressionStatus::from_label(&row.get::<_, String>(11)?),
                    tags,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to query sessions: {}", e)))?;

        let sessions: Result<Vec<_>, _> = rows.collect();
        sessions.map_err(|e| AlephError::other(format!("Failed to collect sessions: {}", e)))
    }

    async fn get_session_metadata(&self, session_id: &str) -> Result<SessionMetadata, AlephError> {
        self.read_metadata(session_id).await
    }

    async fn list_attachments(&self, session_id: &str) -> Result<Vec<AttachmentInfo>, AlephError> {
        let db = self.db.lock().await;

        let mut stmt = db
            .prepare(
                "SELECT attachment_id, session_id, file_path, content_hash, mime_type, size_bytes, uploaded_at \
                 FROM attachments WHERE session_id = ? ORDER BY uploaded_at",
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([session_id], |row| {
                Ok(AttachmentInfo {
                    attachment_id: row.get(0)?,
                    session_id: row.get(1)?,
                    file_path: row.get(2)?,
                    content_hash: row.get(3)?,
                    mime_type: row.get(4)?,
                    size_bytes: row.get(5)?,
                    uploaded_at: row.get(6)?,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to query attachments: {}", e)))?;

        let attachments: Result<Vec<_>, _> = rows.collect();
        attachments.map_err(|e| AlephError::other(format!("Failed to collect attachments: {}", e)))
    }

    async fn get_attachment(
        &self,
        session_id: &str,
        attachment_id: &str,
    ) -> Result<Attachment, AlephError> {
        // Query database for file path and MIME type
        let (file_path, mime_type): (String, String) = {
            let db = self.db.lock().await;

            let mut stmt = db
                .prepare(
                    "SELECT file_path, mime_type FROM attachments WHERE session_id = ? AND attachment_id = ?",
                )
                .map_err(|e| AlephError::other(format!("Failed to prepare query: {}", e)))?;

            stmt.query_row([session_id, attachment_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| AlephError::other(format!("Failed to query attachment: {}", e)))?
        }; // Drop db lock here

        // Attachment paths are portable relative paths under ~/.aleph. Reject
        // legacy absolute paths and traversal before resolving any file.
        let relative_path = Path::new(&file_path);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(AlephError::other(format!(
                "Attachment path escapes storage root: {}",
                file_path
            )));
        }
        let resolved_path = self.base_dir.join(relative_path);
        let canonical_base = fs::canonicalize(&self.base_dir).await.map_err(|e| {
            AlephError::other(format!("Failed to resolve attachment storage root: {}", e))
        })?;
        let canonical_path = fs::canonicalize(&resolved_path).await.map_err(|e| {
            AlephError::other(format!(
                "Failed to resolve attachment {:?}: {}",
                resolved_path, e
            ))
        })?;
        if !canonical_path.starts_with(&canonical_base) {
            return Err(AlephError::other(format!(
                "Attachment path escapes storage root: {}",
                file_path
            )));
        }

        // Now read file without holding the lock.
        let data = fs::read(&canonical_path).await.map_err(|e| {
            AlephError::other(format!(
                "Failed to read attachment {:?}: {}",
                canonical_path, e
            ))
        })?;

        Ok(Attachment {
            data,
            mime_type,
            original_name: None,
        })
    }

    async fn search_transcripts(
        &self,
        query: &str,
        agent_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SessionSearchResult>, AlephError> {
        let Some(query) = normalize_fts_query(query) else {
            return Ok(Vec::new());
        };
        let db = self.db.lock().await;

        let mut sql = "SELECT f.session_id, s.agent_id, s.started_at, snippet(session_transcripts_fts, 1, '<mark>', '</mark>', '...', 64) as snippet, \
                       rank FROM session_transcripts_fts f \
                       JOIN sessions s ON f.session_id = s.session_id \
                       WHERE session_transcripts_fts MATCH ? ".to_string();

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(query)];

        if let Some(aid) = agent_id {
            sql.push_str("AND s.agent_id = ? ");
            params.push(Box::new(aid.to_string()));
        }

        sql.push_str("ORDER BY rank LIMIT ?");
        params.push(Box::new(limit as i64));

        let mut stmt = db
            .prepare(&sql)
            .map_err(|e| AlephError::other(format!("Failed to prepare FTS query: {}", e)))?;

        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(&param_refs[..], |row| {
                Ok(SessionSearchResult {
                    session_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    started_at: row.get(2)?,
                    snippet: row.get(3)?,
                    score: row.get::<_, f64>(4)? as f32,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to query FTS: {}", e)))?;

        let results: Result<Vec<_>, _> = rows.collect();
        results.map_err(|e| AlephError::other(format!("Failed to collect search results: {}", e)))
    }

    async fn update_compression_status(
        &self,
        session_id: &str,
        status: CompressionStatus,
    ) -> Result<(), AlephError> {
        let db = self.db.lock().await;

        db.execute(
            "UPDATE sessions SET compression_status = ?, updated_at = ? WHERE session_id = ?",
            rusqlite::params![status.as_str(), chrono::Utc::now().timestamp(), session_id],
        )
        .map_err(|e| AlephError::other(format!("Failed to update compression status: {}", e)))?;

        Ok(())
    }

    async fn archive_session(&self, session_id: &str) -> Result<(), AlephError> {
        // Move session directory to archive
        let metadata = self.read_metadata(session_id).await?;
        let started = chrono::DateTime::from_timestamp(metadata.started_at, 0)
            .ok_or_else(|| AlephError::other("Invalid timestamp"))?;

        let year = started.format("%Y").to_string();
        let month = started.format("%m").to_string();

        let archive_dir = self.base_dir.join("archive").join(&year).join(&month);

        fs::create_dir_all(&archive_dir).await.map_err(|e| {
            AlephError::other(format!(
                "Failed to create archive directory {:?}: {}",
                archive_dir, e
            ))
        })?;

        let src = self.session_dir(session_id);
        let dst = archive_dir.join(session_id);

        fs::rename(&src, &dst).await.map_err(|e| {
            AlephError::other(format!(
                "Failed to move session {:?} to {:?}: {}",
                src, dst, e
            ))
        })?;

        // Update compression status
        self.update_compression_status(session_id, CompressionStatus::Archived)
            .await?;

        Ok(())
    }
}

// Helper functions

/// Treat ordinary user text as one literal FTS5 phrase, not query syntax.
fn normalize_fts_query(query: &str) -> Option<String> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    Some(format!("\"{}\"", query.replace('"', "\"\"")))
}

fn mime_type_to_ext(mime_type: &str) -> &str {
    match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "application/json" => "json",
        _ => "bin",
    }
}

fn mime_type_to_dir(mime_type: &str) -> &str {
    if mime_type.starts_with("image/") {
        "images"
    } else if mime_type.starts_with("video/") {
        "videos"
    } else if mime_type.starts_with("audio/") {
        "audio"
    } else {
        "documents"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = MarkdownSessionStore::new(temp_dir.path()).await.unwrap();

        let metadata = SessionMetadata {
            session_id: "test_session_001".to_string(),
            agent_id: "agent_001".to_string(),
            workspace_id: Some("ws_001".to_string()),
            started_at: 1729267535,
            ended_at: None,
            duration_seconds: None,
            transcript_path: PathBuf::from("transcript.md"),
            tool_outputs_path: PathBuf::from("tool_outputs.md"),
            message_count: 0,
            tool_call_count: 0,
            content_hash: None,
            compression_status: CompressionStatus::Pending,
            tags: vec![],
        };

        store.create_session(metadata.clone()).await.unwrap();

        // Verify directory structure
        let session_dir = temp_dir.path().join("raw/sessions/test_session_001");
        assert!(session_dir.exists());
        assert!(session_dir.join("transcript.md").exists());
        assert!(session_dir.join("tool_outputs.md").exists());
        assert!(session_dir.join("metadata.json").exists());
    }

    #[tokio::test]
    async fn test_append_transcript() {
        let temp_dir = TempDir::new().unwrap();
        let store = MarkdownSessionStore::new(temp_dir.path()).await.unwrap();

        let metadata = SessionMetadata {
            session_id: "test_session_002".to_string(),
            agent_id: "agent_001".to_string(),
            workspace_id: None,
            started_at: 1729267535,
            ended_at: None,
            duration_seconds: None,
            transcript_path: PathBuf::from("transcript.md"),
            tool_outputs_path: PathBuf::from("tool_outputs.md"),
            message_count: 0,
            tool_call_count: 0,
            content_hash: None,
            compression_status: CompressionStatus::Pending,
            tags: vec![],
        };

        store.create_session(metadata).await.unwrap();

        store
            .append_transcript(
                "test_session_002",
                1729267536,
                MessageRole::User,
                "Hello, world!",
            )
            .await
            .unwrap();

        let transcript = store.read_transcript("test_session_002").await.unwrap();
        assert!(transcript.contains("Hello, world!"));
        assert!(transcript.contains("User"));

        // A later append must preserve earlier searchable content and share
        // the metadata read/modify/write lock with tool output appends.
        let tool_result = ToolResult {
            exit_code: Some(0),
            stdout: "tool-only-marker".to_string(),
            stderr: String::new(),
            duration_ms: 1,
        };
        let (message, tool) = tokio::join!(
            store.append_transcript(
                "test_session_002",
                1729267537,
                MessageRole::Assistant,
                "Goodbye, moon!",
            ),
            store.append_tool_output("test_session_002", 1729267538, "echo", "{}", &tool_result,),
        );
        message.unwrap();
        tool.unwrap();

        let metadata = store
            .get_session_metadata("test_session_002")
            .await
            .unwrap();
        assert_eq!(metadata.message_count, 2);
        assert_eq!(metadata.tool_call_count, 1);
        let sessions = store
            .list_sessions(None, None, None, None, None, 10, 0)
            .await
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].message_count, metadata.message_count);
        assert_eq!(sessions[0].tool_call_count, metadata.tool_call_count);

        for query in ["Hello", "Goodbye", "Hello, world!"] {
            let results = store
                .search_transcripts(query, Some("agent_001"), 10)
                .await
                .unwrap();
            assert_eq!(results.len(), 1, "query: {query}");
            assert_eq!(results[0].session_id, "test_session_002");
        }
        assert!(store
            .search_transcripts("Hello", Some("other_agent"), 10)
            .await
            .unwrap()
            .is_empty());
        assert!(store
            .search_transcripts("\"tool-only-marker\"", None, 10)
            .await
            .unwrap()
            .is_empty());

        store
            .finalize_session("test_session_002", 1729267539)
            .await
            .unwrap();
        let finalized = store
            .get_session_metadata("test_session_002")
            .await
            .unwrap();
        assert_eq!(finalized.message_count, 2);
        assert_eq!(finalized.tool_call_count, 1);
        assert_eq!(finalized.ended_at, Some(1729267539));
    }

    #[test]
    fn test_normalize_fts_query() {
        for query in ["", " \t\n", "\u{2003}"] {
            assert_eq!(normalize_fts_query(query), None);
        }
        assert_eq!(
            normalize_fts_query("  Session agent:fts-test Turn  "),
            Some("\"Session agent:fts-test Turn\"".to_string())
        );
        assert_eq!(
            normalize_fts_query("say \"hello\" OR goodbye"),
            Some("\"say \"\"hello\"\" OR goodbye\"".to_string())
        );
    }

    #[tokio::test]
    async fn test_search_transcripts_literal_punctuation() {
        let temp_dir = TempDir::new().unwrap();
        let store = MarkdownSessionStore::new(temp_dir.path()).await.unwrap();
        let session_id = "agent:fts-test";
        store
            .create_session(SessionMetadata {
                session_id: session_id.to_string(),
                agent_id: "fts-agent".to_string(),
                workspace_id: None,
                started_at: 1729267535,
                ended_at: None,
                duration_seconds: None,
                transcript_path: PathBuf::from("transcript.md"),
                tool_outputs_path: PathBuf::from("tool_outputs.md"),
                message_count: 0,
                tool_call_count: 0,
                content_hash: None,
                compression_status: CompressionStatus::Pending,
                tags: vec![],
            })
            .await
            .unwrap();
        store
            .append_transcript(
                session_id,
                1729267536,
                MessageRole::User,
                "Session agent:fts-test Turn says \"hello\" OR goodbye (notes) *",
            )
            .await
            .unwrap();

        for query in [
            format!("Session {session_id} Turn"),
            "  Session agent:fts-test Turn  ".to_string(),
            "\"hello\"".to_string(),
            "hello\" OR goodbye".to_string(),
            "(notes) *".to_string(),
        ] {
            for agent in [None, Some("fts-agent")] {
                let results = store.search_transcripts(&query, agent, 10).await.unwrap();
                assert_eq!(results.len(), 1, "query: {query}");
                assert_eq!(results[0].session_id, session_id);
                assert_eq!(results[0].agent_id, "fts-agent");
                assert!(results[0].snippet.contains("<mark>"));
                assert!(results[0].score.is_finite());
            }
        }
        for query in ["", " \t\n", ":", "\"", "missing OR hello"] {
            assert!(
                store
                    .search_transcripts(query, None, 10)
                    .await
                    .unwrap()
                    .is_empty(),
                "query: {query}"
            );
        }
        assert!(store
            .search_transcripts("Session agent:fts-test Turn", Some("other-agent"), 10)
            .await
            .unwrap()
            .is_empty());
        assert!(store
            .search_transcripts("Session agent:fts-test Turn", None, 0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_store_attachment() {
        let temp_dir = TempDir::new().unwrap();
        let store = MarkdownSessionStore::new(temp_dir.path()).await.unwrap();

        let metadata = SessionMetadata {
            session_id: "test_session_003".to_string(),
            agent_id: "agent_001".to_string(),
            workspace_id: None,
            started_at: 1729267535,
            ended_at: None,
            duration_seconds: None,
            transcript_path: PathBuf::from("transcript.md"),
            tool_outputs_path: PathBuf::from("tool_outputs.md"),
            message_count: 0,
            tool_call_count: 0,
            content_hash: None,
            compression_status: CompressionStatus::Pending,
            tags: vec![],
        };

        store.create_session(metadata).await.unwrap();

        let attachment = Attachment {
            data: b"test image data".to_vec(),
            mime_type: "image/png".to_string(),
            original_name: Some("test.png".to_string()),
        };

        let att_ref = store
            .store_attachment("test_session_003", attachment)
            .await
            .unwrap();

        assert!(att_ref.file_path.exists());
        assert_eq!(att_ref.mime_type, "image/png");
    }
}
