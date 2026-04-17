//! Persistent security audit log.
//!
//! Records security events to SQLite for post-incident analysis.
//! Uses async channel for non-blocking writes from hot paths.

use std::fmt;
use tokio::sync::mpsc;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEventType {
    AuthFailure,
    RateLimited,
    SsrfBlocked,
    ExecBlocked,
    ExecApprovalDenied,
    InvisibleCharsDetected,
    InjectionPatternDetected,
    EnvInjectionDetected,
    PathTraversalBlocked,
    PairingAttempt,
    PairingBruteForce,
    PermissionDenied,
    GuestSessionCreated,
    PiiDetected,
    LeakWarning,
}

impl fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::AuthFailure => "auth_failure",
            Self::RateLimited => "rate_limited",
            Self::SsrfBlocked => "ssrf_blocked",
            Self::ExecBlocked => "exec_blocked",
            Self::ExecApprovalDenied => "exec_approval_denied",
            Self::InvisibleCharsDetected => "invisible_chars",
            Self::InjectionPatternDetected => "injection_pattern",
            Self::EnvInjectionDetected => "env_injection",
            Self::PathTraversalBlocked => "path_traversal_blocked",
            Self::PairingAttempt => "pairing_attempt",
            Self::PairingBruteForce => "pairing_brute_force",
            Self::PermissionDenied => "permission_denied",
            Self::GuestSessionCreated => "guest_session_created",
            Self::PiiDetected => "pii_detected",
            Self::LeakWarning => "leak_warning",
        };
        write!(f, "{}", s)
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

#[derive(Clone)]
pub struct SecurityAuditLog {
    sender: mpsc::Sender<AuditEntry>,
}

impl SecurityAuditLog {
    pub fn new(buffer_size: usize) -> (Self, mpsc::Receiver<AuditEntry>) {
        let (sender, receiver) = mpsc::channel(buffer_size);
        (Self { sender }, receiver)
    }

    pub fn log(&self, entry: AuditEntry) {
        if let Err(e) = self.sender.try_send(entry) {
            warn!("Security audit log channel full, dropping entry: {}", e);
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
        // This should drop without panic
        log.log_event(
            AuditEventType::AuthFailure,
            AuditSeverity::Critical,
            "second".into(),
        );
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
}
