//! Session Storage - JSONL Backend
//!
//! Provides persistent storage for session messages using JSONL (JSON Lines) format.
//! Each session is stored in a separate `.jsonl` file, with one JSON object per line.
//!
//! # Format
//!
//! ```jsonl
//! {"type":"meta","session_key":"agent:main:main","created_at":"2026-01-28T12:00:00Z"}
//! {"type":"message","role":"user","content":"Hello","timestamp":"2026-01-28T12:00:01Z"}
//! {"type":"message","role":"assistant","content":"Hi!","timestamp":"2026-01-28T12:00:02Z"}
//! ```
//!
//! # Features
//!
//! - Append-only writes for crash safety
//! - Automatic session file discovery
//! - Lazy loading on first access
//! - Session archiving on reset

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use super::agent_instance::{MessageRole, SessionMessage};

/// JSONL record types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionRecord {
    /// Session metadata (first line of file)
    Meta {
        session_key: String,
        agent_id: String,
        created_at: DateTime<Utc>,
    },
    /// A message in the session
    Message {
        role: String,
        content: String,
        timestamp: DateTime<Utc>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<HashMap<String, String>>,
    },
    /// Session reset marker
    Reset {
        timestamp: DateTime<Utc>,
    },
}

/// Session storage backend using JSONL files
pub struct SessionStorage {
    /// Base directory for session files
    sessions_dir: PathBuf,
}

impl SessionStorage {
    /// Create a new session storage backend
    ///
    /// # Arguments
    ///
    /// * `agent_dir` - The agent's directory (sessions will be stored in `{agent_dir}/sessions/`)
    pub fn new(agent_dir: &Path) -> std::io::Result<Self> {
        let sessions_dir = agent_dir.join("sessions");
        fs::create_dir_all(&sessions_dir)?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o700);
            let _ = fs::set_permissions(&sessions_dir, perms);
        }

        info!("Session storage initialized at {:?}", sessions_dir);

        Ok(Self {
            sessions_dir,
        })
    }

    /// Get the file path for a session
    fn session_file_path(&self, session_key: &str) -> PathBuf {
        // Sanitize the session key for use as filename
        let safe_name = session_key
            .replace([':', '/', '\\', '\0'], "_");
        self.sessions_dir.join(format!("{}.jsonl", safe_name))
    }

    /// Load a session from disk
    ///
    /// Returns the session metadata and messages, or None if session doesn't exist.
    pub fn load_session(&self, session_key: &str) -> Option<LoadedSession> {
        let file_path = self.session_file_path(session_key);

        if !file_path.exists() {
            return None;
        }

        let file = match File::open(&file_path) {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open session file {:?}: {}", file_path, e);
                return None;
            }
        };

        let reader = BufReader::new(file);
        let mut meta: Option<SessionMeta> = None;
        let mut messages: Vec<SessionMessage> = Vec::new();
        let mut last_reset_idx: Option<usize> = None;

        for (line_num, line_result) in reader.lines().enumerate() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    warn!("Error reading line {} of {:?}: {}", line_num, file_path, e);
                    continue;
                }
            };

            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<SessionRecord>(&line) {
                Ok(SessionRecord::Meta { session_key, agent_id, created_at }) => {
                    meta = Some(SessionMeta {
                        session_key,
                        agent_id,
                        created_at,
                    });
                }
                Ok(SessionRecord::Message { role, content, timestamp, metadata }) => {
                    let msg_role = match role.as_str() {
                        "user" => MessageRole::User,
                        "assistant" => MessageRole::Assistant,
                        "system" => MessageRole::System,
                        "tool" => MessageRole::Tool,
                        _ => MessageRole::User,
                    };
                    messages.push(SessionMessage {
                        role: msg_role,
                        content,
                        timestamp,
                        metadata,
                    });
                }
                Ok(SessionRecord::Reset { .. }) => {
                    // Mark this position - we'll discard messages before this
                    last_reset_idx = Some(messages.len());
                }
                Err(e) => {
                    warn!("Failed to parse line {} of {:?}: {}", line_num, file_path, e);
                }
            }
        }

        // If there was a reset, only keep messages after the reset
        if let Some(reset_idx) = last_reset_idx {
            messages = messages.split_off(reset_idx);
        }

        meta.map(|m| LoadedSession {
            meta: m,
            messages,
        })
    }

    /// Create a new session file
    pub fn create_session(&self, session_key: &str, agent_id: &str) -> std::io::Result<()> {
        let file_path = self.session_file_path(session_key);

        // Don't overwrite existing session
        if file_path.exists() {
            debug!("Session file already exists: {:?}", file_path);
            return Ok(());
        }

        let mut file = File::create(&file_path)?;

        let meta = SessionRecord::Meta {
            session_key: session_key.to_string(),
            agent_id: agent_id.to_string(),
            created_at: Utc::now(),
        };

        let line = serde_json::to_string(&meta)?;
        writeln!(file, "{}", line)?;
        file.flush()?;

        debug!("Created session file: {:?}", file_path);
        Ok(())
    }

    /// Append a message to a session
    pub fn append_message(
        &self,
        session_key: &str,
        role: MessageRole,
        content: &str,
        metadata: Option<HashMap<String, String>>,
    ) -> std::io::Result<()> {
        let file_path = self.session_file_path(session_key);

        // Open file in append mode
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;

        let role_str = match role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        };

        let record = SessionRecord::Message {
            role: role_str.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
            metadata,
        };

        let line = serde_json::to_string(&record)?;
        writeln!(file, "{}", line)?;
        file.flush()?;

        debug!("Appended message to session: {}", session_key);
        Ok(())
    }

    /// Reset a session (mark all previous messages as cleared)
    pub fn reset_session(&self, session_key: &str) -> std::io::Result<()> {
        let file_path = self.session_file_path(session_key);

        if !file_path.exists() {
            return Ok(());
        }

        // Append a reset marker
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;

        let record = SessionRecord::Reset {
            timestamp: Utc::now(),
        };

        let line = serde_json::to_string(&record)?;
        writeln!(file, "{}", line)?;
        file.flush()?;

        info!("Reset session: {}", session_key);
        Ok(())
    }

    /// List all session keys stored on disk
    pub fn list_sessions(&self) -> Vec<String> {
        let mut sessions = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.sessions_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                    if let Some(stem) = path.file_stem() {
                        // Convert sanitized filename back to session key
                        let key = stem.to_string_lossy().replace('_', ":");
                        sessions.push(key);
                    }
                }
            }
        }

        sessions
    }

    /// Archive a session (move to archive directory)
    pub fn archive_session(&self, session_key: &str) -> std::io::Result<()> {
        let file_path = self.session_file_path(session_key);

        if !file_path.exists() {
            return Ok(());
        }

        let archive_dir = self.sessions_dir.join("archive");
        fs::create_dir_all(&archive_dir)?;

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let safe_name = session_key
            .replace([':', '/', '\\'], "_");
        let archive_name = format!("{}_{}.jsonl", safe_name, timestamp);
        let archive_path = archive_dir.join(archive_name);

        fs::rename(&file_path, &archive_path)?;
        info!("Archived session {} to {:?}", session_key, archive_path);

        Ok(())
    }

    /// Delete a session file permanently
    pub fn delete_session(&self, session_key: &str) -> std::io::Result<()> {
        let file_path = self.session_file_path(session_key);

        if file_path.exists() {
            fs::remove_file(&file_path)?;
            info!("Deleted session file: {:?}", file_path);
        }

        Ok(())
    }

    /// Get the sessions directory path
    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }
}

/// Metadata for a loaded session
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub session_key: String,
    pub agent_id: String,
    pub created_at: DateTime<Utc>,
}

/// A fully loaded session from disk
#[derive(Debug, Clone)]
pub struct LoadedSession {
    pub meta: SessionMeta,
    pub messages: Vec<SessionMessage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_session_file_path() {
        let temp = tempdir().unwrap();
        let storage = SessionStorage::new(temp.path()).unwrap();

        let path = storage.session_file_path("agent:main:main");
        assert!(path.to_string_lossy().contains("agent_main_main.jsonl"));
    }

    #[test]
    fn test_create_and_load_session() {
        let temp = tempdir().unwrap();
        let storage = SessionStorage::new(temp.path()).unwrap();

        // Create session
        storage.create_session("test:session", "test-agent").unwrap();

        // Add messages
        storage.append_message("test:session", MessageRole::User, "Hello", None).unwrap();
        storage.append_message("test:session", MessageRole::Assistant, "Hi there!", None).unwrap();

        // Load session
        let loaded = storage.load_session("test:session").unwrap();
        assert_eq!(loaded.meta.session_key, "test:session");
        assert_eq!(loaded.meta.agent_id, "test-agent");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content, "Hello");
        assert_eq!(loaded.messages[1].content, "Hi there!");
    }

    #[test]
    fn test_reset_session() {
        let temp = tempdir().unwrap();
        let storage = SessionStorage::new(temp.path()).unwrap();

        // Create session with messages
        storage.create_session("test:reset", "agent").unwrap();
        storage.append_message("test:reset", MessageRole::User, "Old message", None).unwrap();

        // Reset
        storage.reset_session("test:reset").unwrap();

        // Add new message
        storage.append_message("test:reset", MessageRole::User, "New message", None).unwrap();

        // Load - should only have new message
        let loaded = storage.load_session("test:reset").unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].content, "New message");
    }

    #[test]
    fn test_list_sessions() {
        let temp = tempdir().unwrap();
        let storage = SessionStorage::new(temp.path()).unwrap();

        storage.create_session("agent:main:session1", "agent").unwrap();
        storage.create_session("agent:main:session2", "agent").unwrap();

        let sessions = storage.list_sessions();
        assert_eq!(sessions.len(), 2);
    }
}
