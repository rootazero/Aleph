//! Persistent security audit log.
//!
//! Records security events to `SQLite` for post-incident analysis.
//! Uses async channel for non-blocking writes from hot paths.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuditEventType {
    AuthFailure,
    RateLimited,
    SsrfBlocked,
    ExecBlocked,
    ExecApprovalDenied,
    InvisibleCharsDetected,
    TokenizerMarkerScrubbed,
    InjectionPatternDetected,
    EnvInjectionDetected,
    PathTraversalBlocked,
    PermissionDenied,
    PiiDetected,
    LeakWarning,
}

impl fmt::Display for AuditEventType {
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::AuthFailure => "auth_failure",
            Self::RateLimited => "rate_limited",
            Self::SsrfBlocked => "ssrf_blocked",
            Self::ExecBlocked => "exec_blocked",
            Self::ExecApprovalDenied => "exec_approval_denied",
            Self::InvisibleCharsDetected => "invisible_chars",
            Self::TokenizerMarkerScrubbed => "tokenizer_marker_scrubbed",
            Self::InjectionPatternDetected => "injection_pattern",
            Self::EnvInjectionDetected => "env_injection",
            Self::PathTraversalBlocked => "path_traversal_blocked",
            Self::PermissionDenied => "permission_denied",
            Self::PiiDetected => "pii_detected",
            Self::LeakWarning => "leak_warning",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditSeverity {
    Critical,
    Warn,
    Info,
}

impl fmt::Display for AuditSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::Warn => write!(f, "warn"),
            Self::Info => write!(f, "info"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub event_type: AuditEventType,
    pub severity: AuditSeverity,
    pub source_ip: Option<String>,
    pub session_id: Option<String>,
    pub detail: String,
}

impl AuditEntry {
    /// A remote connection failed the Gateway-token login wall at `connect`.
    /// `source_ip` is the socket peer; `detail` names the rejected path.
    #[must_use]
    pub fn auth_failure(source_ip: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            event_type: AuditEventType::AuthFailure,
            severity: AuditSeverity::Warn,
            source_ip: Some(source_ip.into()),
            session_id: None,
            detail: detail.into(),
        }
    }

    /// A remote connection was closed for excessive requests (flood guard).
    #[must_use]
    pub fn rate_limited(source_ip: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            event_type: AuditEventType::RateLimited,
            severity: AuditSeverity::Warn,
            source_ip: Some(source_ip.into()),
            session_id: None,
            detail: detail.into(),
        }
    }
}

#[derive(Clone)]
pub struct SecurityAuditLog {
    sender: mpsc::Sender<AuditEntry>,
    dropped_count: Arc<AtomicU64>,
}

impl SecurityAuditLog {
    #[must_use]
    pub fn new(buffer_size: usize) -> (Self, mpsc::Receiver<AuditEntry>) {
        let (sender, receiver) = mpsc::channel(buffer_size);
        (
            Self {
                sender,
                dropped_count: Arc::new(AtomicU64::new(0)),
            },
            receiver,
        )
    }

    pub fn log(&self, entry: AuditEntry) {
        if let Err(e) = self.sender.try_send(entry) {
            let count = self.dropped_count.fetch_add(1, Ordering::AcqRel) + 1;
            if count == 1 || count.is_multiple_of(100) {
                error!(
                    "Security audit log channel full, dropping entry (total dropped: {}): {}",
                    count, e
                );
            }
        }
    }

    pub fn log_event(&self, event_type: AuditEventType, severity: AuditSeverity, detail: String) {
        self.log(AuditEntry {
            event_type,
            severity,
            source_ip: None,
            session_id: None,
            detail,
        });
    }

    /// Returns the number of audit entries dropped due to channel backpressure.
    #[must_use]
    pub fn dropped_count(&self) -> u64 {
        self.dropped_count.load(Ordering::Acquire)
    }
}

pub const AUDIT_LOG_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS security_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    event_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    source_ip TEXT,
    session_id TEXT,
    detail TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON security_audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_event_type ON security_audit_log(event_type);
"#;

pub const AUDIT_CLEANUP_SQL: &str =
    "DELETE FROM security_audit_log WHERE timestamp < strftime('%s', 'now') - ?1";

pub const DEFAULT_RETENTION_SECS: i64 = 30 * 24 * 3600;

pub const AUDIT_INSERT_SQL: &str = r#"
INSERT INTO security_audit_log (event_type, severity, source_ip, session_id, detail)
VALUES (?1, ?2, ?3, ?4, ?5)
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_log_send_receive() {
        let (log, mut rx) = SecurityAuditLog::new(100);
        log.log_event(
            AuditEventType::SsrfBlocked,
            AuditSeverity::Warn,
            "Blocked request to 10.0.0.1".to_string(),
        );
        let entry = rx.recv().await.unwrap();
        assert_eq!(entry.event_type, AuditEventType::SsrfBlocked);
        assert_eq!(entry.severity, AuditSeverity::Warn);
        assert!(entry.detail.contains("10.0.0.1"));
    }

    #[tokio::test]
    async fn test_audit_log_drops_when_full() {
        let (log, _rx) = SecurityAuditLog::new(1);
        log.log_event(
            AuditEventType::AuthFailure,
            AuditSeverity::Critical,
            "first".into(),
        );
        // Channel has capacity 1; second message should be dropped.
        log.log_event(
            AuditEventType::AuthFailure,
            AuditSeverity::Critical,
            "second".into(),
        );
        assert_eq!(log.dropped_count(), 1);
    }

    #[test]
    fn test_event_type_display() {
        assert_eq!(AuditEventType::SsrfBlocked.to_string(), "ssrf_blocked");
        assert_eq!(
            AuditEventType::InvisibleCharsDetected.to_string(),
            "invisible_chars"
        );
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(AuditSeverity::Critical.to_string(), "critical");
    }

    #[test]
    fn auth_failure_entry_stamps_ip_and_type() {
        let e = AuditEntry::auth_failure("10.0.0.6", "remote connect rejected");
        assert_eq!(e.event_type, AuditEventType::AuthFailure);
        assert_eq!(e.severity, AuditSeverity::Warn);
        assert_eq!(e.source_ip.as_deref(), Some("10.0.0.6"));
        assert!(e.session_id.is_none());
        assert!(e.detail.contains("rejected"));
    }

    #[test]
    fn rate_limited_entry_stamps_ip_and_type() {
        let e = AuditEntry::rate_limited("10.0.0.6", "flood guard: 10 unauthorized");
        assert_eq!(e.event_type, AuditEventType::RateLimited);
        assert_eq!(e.severity, AuditSeverity::Warn);
        assert_eq!(e.source_ip.as_deref(), Some("10.0.0.6"));
    }
}
