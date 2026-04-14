//! Append-only log writer for `log.md`. Rotates at 2000 lines.

use crate::error::AlephError;
use crate::memory::wiki::types::LogEntry;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

pub const LOG_FILENAME: &str = "log.md";
pub const LOG_ROTATE_LINES: usize = 2000;

pub struct LogMdWriter {
    agent_dir: PathBuf,
}

impl LogMdWriter {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self {
            agent_dir: agent_dir.into(),
        }
    }

    fn log_path(&self) -> PathBuf {
        self.agent_dir.join(LOG_FILENAME)
    }

    /// Append a single entry. Creates the file with a header on first write.
    pub async fn append(&self, entry: &LogEntry) -> Result<(), AlephError> {
        tokio::fs::create_dir_all(&self.agent_dir)
            .await
            .map_err(|e| AlephError::other(format!("create log dir: {e}")))?;

        let path = self.log_path();
        let exists = tokio::fs::try_exists(&path)
            .await
            .map_err(|e| AlephError::other(format!("stat log: {e}")))?;

        let mut buf = String::new();
        if !exists {
            buf.push_str("# Aleph Wiki Log\n\n");
            buf.push_str("> Append-only activity timeline. Rotates at 2000 lines.\n\n");
        }

        let ts: DateTime<Utc> =
            DateTime::<Utc>::from_timestamp(entry.timestamp_utc, 0).unwrap_or_else(Utc::now);
        buf.push_str(&format!(
            "## [{ts}] {action} | {summary}\n",
            ts = ts.format("%Y-%m-%d %H:%M:%SZ"),
            action = entry.action.as_str(),
            summary = sanitize_single_line(&entry.summary),
        ));
        for line in &entry.detail_lines {
            buf.push_str(&format!("- {}\n", sanitize_single_line(line)));
        }
        buf.push('\n');

        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .await
            .map_err(|e| AlephError::other(format!("open log: {e}")))?;
        f.write_all(buf.as_bytes())
            .await
            .map_err(|e| AlephError::other(format!("write log: {e}")))?;
        Ok(())
    }

    /// Count lines in log.md (returns 0 if missing).
    pub async fn line_count(&self) -> Result<usize, AlephError> {
        let path = self.log_path();
        if !tokio::fs::try_exists(&path)
            .await
            .map_err(|e| AlephError::other(format!("stat log: {e}")))?
        {
            return Ok(0);
        }
        let s = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| AlephError::other(format!("read log: {e}")))?;
        Ok(s.lines().count())
    }

    /// Rotate if over threshold. Renames to `log-YYYY-MM-DD.md`; new log.md
    /// gets a "continued from …" header line.
    pub async fn rotate_if_needed(&self) -> Result<bool, AlephError> {
        if self.line_count().await? <= LOG_ROTATE_LINES {
            return Ok(false);
        }
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let new_name = format!("log-{today}.md");
        let from = self.log_path();
        let to = self.agent_dir.join(&new_name);
        tokio::fs::rename(&from, &to)
            .await
            .map_err(|e| AlephError::other(format!("rotate log: {e}")))?;

        let header = format!("# Aleph Wiki Log\n\n<!-- continued from {new_name} -->\n\n");
        tokio::fs::write(self.log_path(), header)
            .await
            .map_err(|e| AlephError::other(format!("write new log: {e}")))?;
        Ok(true)
    }

    /// Read the last `n` lines (or the whole file if shorter).
    pub async fn tail(&self, n: usize) -> Result<String, AlephError> {
        let path = self.log_path();
        if !tokio::fs::try_exists(&path)
            .await
            .map_err(|e| AlephError::other(format!("stat log: {e}")))?
        {
            return Ok(String::new());
        }
        let s = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| AlephError::other(format!("read log: {e}")))?;
        let lines: Vec<&str> = s.lines().filter(|l| !l.is_empty()).collect();
        let start = lines.len().saturating_sub(n);
        Ok(lines[start..].join("\n"))
    }
}

fn sanitize_single_line(s: &str) -> String {
    // Replace \r\n as a unit first, then lone \r or \n, then collapse runs of spaces.
    let replaced = s.replace("\r\n", " ").replace(['\n', '\r'], " ");
    // Collapse multiple spaces into one.
    let mut result = String::with_capacity(replaced.len());
    let mut prev_space = false;
    for ch in replaced.chars() {
        if ch == ' ' {
            if !prev_space {
                result.push(' ');
            }
            prev_space = true;
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::wiki::types::LogAction;

    fn entry(action: LogAction, summary: &str) -> LogEntry {
        LogEntry {
            timestamp_utc: 1_744_600_000,
            action,
            summary: summary.into(),
            detail_lines: vec!["detail-a".into(), "detail-b".into()],
        }
    }

    #[tokio::test]
    async fn first_append_creates_header_and_entry() {
        let dir = tempfile::tempdir().unwrap();
        let w = LogMdWriter::new(dir.path());
        w.append(&entry(LogAction::Bootstrap, "init"))
            .await
            .unwrap();
        let body = tokio::fs::read_to_string(dir.path().join("log.md"))
            .await
            .unwrap();
        assert!(body.starts_with("# Aleph Wiki Log"));
        assert!(body.contains("bootstrap | init"));
        assert!(body.contains("- detail-a"));
    }

    #[tokio::test]
    async fn multiline_summary_sanitised() {
        let dir = tempfile::tempdir().unwrap();
        let w = LogMdWriter::new(dir.path());
        w.append(&entry(LogAction::Ingest, "a\nb\r\nc"))
            .await
            .unwrap();
        let body = tokio::fs::read_to_string(dir.path().join("log.md"))
            .await
            .unwrap();
        assert!(body.contains("ingest | a b c"));
        assert!(!body.contains('\r'));
    }

    #[tokio::test]
    async fn tail_returns_last_n_lines() {
        let dir = tempfile::tempdir().unwrap();
        let w = LogMdWriter::new(dir.path());
        for i in 0..5 {
            w.append(&entry(LogAction::Ingest, &format!("#{i}")))
                .await
                .unwrap();
        }
        let tail = w.tail(3).await.unwrap();
        assert!(tail.contains("#4"));
        assert!(!tail.contains("#0"));
    }

    #[tokio::test]
    async fn rotate_when_over_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let big = "line\n".repeat(LOG_ROTATE_LINES + 5);
        tokio::fs::write(dir.path().join("log.md"), big)
            .await
            .unwrap();
        let w = LogMdWriter::new(dir.path());
        let rotated = w.rotate_if_needed().await.unwrap();
        assert!(rotated);
        let new_log = tokio::fs::read_to_string(dir.path().join("log.md"))
            .await
            .unwrap();
        assert!(new_log.contains("continued from log-"));
        let dated = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("log-"));
        assert!(dated);
    }

    #[tokio::test]
    async fn rotate_noop_when_under_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let w = LogMdWriter::new(dir.path());
        w.append(&entry(LogAction::Ingest, "small")).await.unwrap();
        let rotated = w.rotate_if_needed().await.unwrap();
        assert!(!rotated);
    }
}
