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
    ExecBlocked,
    EnvInjectionDetected,
    PiiDetected,
    LeakWarning,
    /// An operator read persisted content belonging to another user.
    ///
    /// Ratified by human ruling on 2026-08-07: `trace.list` / `trace.get`
    /// hand an operator any run's full transcript (prompts, tool inputs, tool
    /// outputs) and stay admin-gated rather than owner-scoped, because that is
    /// the operator's debugging surface. What was missing was the
    /// accountability half — the read left no trace of itself anywhere. This
    /// variant is that trace. Reading your OWN content is not an event, so on
    /// a single-user box nothing is ever recorded here.
    ScopedContentRead,
}

impl fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::AuthFailure => "auth_failure",
            Self::RateLimited => "rate_limited",
            Self::ExecBlocked => "exec_blocked",
            Self::EnvInjectionDetected => "env_injection",
            Self::PiiDetected => "pii_detected",
            Self::LeakWarning => "leak_warning",
            Self::ScopedContentRead => "scoped_content_read",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditSeverity {
    Critical,
    Warn,
}

impl fmt::Display for AuditSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::Warn => write!(f, "warn"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub event_type: AuditEventType,
    pub severity: AuditSeverity,
    pub source_ip: Option<String>,
    pub session_id: Option<String>,
    /// WHO acted — the authenticated `users.user_id` behind the request
    /// ([`crate::gateway::caller_identity::CALLER_USER`]), never the owner of
    /// whatever was acted upon. `None` for the events that predate any user
    /// model (a connection that never got past the login wall has no user to
    /// name) and for non-gateway producers.
    ///
    /// `source_ip` answered "from where" and `session_id` "about what"; until
    /// [`AuditEventType::ScopedContentRead`] there was nothing that answered
    /// "by whom", which is the only question a cross-user read raises.
    pub actor_user: Option<String>,
    pub detail: String,
}

impl AuditEntry {
    /// An operator read persisted content owned by somebody else —
    /// see [`AuditEventType::ScopedContentRead`].
    ///
    /// `actor_user` is the caller; `session_id` is the session the content
    /// belongs to (the thing whose ownership was checked), so the two columns
    /// together say who read whose. `detail` names the surface and the record;
    /// it must never carry the content itself, which is the whole point of not
    /// having read it into the log.
    ///
    /// Severity is `Warn` and not `Critical` because the read is ratified, not
    /// a violation. There is no informational rung on [`AuditSeverity`] and
    /// inventing one for a single producer is the abstraction R10 refuses.
    #[must_use]
    pub fn scoped_content_read(
        actor_user: impl Into<String>,
        session_id: Option<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            event_type: AuditEventType::ScopedContentRead,
            severity: AuditSeverity::Warn,
            source_ip: None,
            session_id,
            actor_user: Some(actor_user.into()),
            detail: detail.into(),
        }
    }

    /// A remote connection failed the Gateway-token login wall at `connect`.
    /// `source_ip` is the socket peer; `detail` names the rejected path.
    #[must_use]
    pub fn auth_failure(source_ip: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            event_type: AuditEventType::AuthFailure,
            severity: AuditSeverity::Warn,
            source_ip: Some(source_ip.into()),
            session_id: None,
            // A connection rejected AT the login wall has no resolved user.
            actor_user: None,
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
            actor_user: None,
            detail: detail.into(),
        }
    }
}

#[derive(Clone)]
pub struct SecurityAuditLog {
    sender: mpsc::Sender<AuditEntry>,
    pub(crate) dropped_count: Arc<AtomicU64>,
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
}

/// Shape of the sink. `actor_user` arrived with
/// [`AuditEventType::ScopedContentRead`]; a store created before it gets the
/// column from the v15 migration in `gateway::security::store`, which probes
/// for it rather than trusting the version gate alone.
pub const AUDIT_LOG_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS security_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    event_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    source_ip TEXT,
    session_id TEXT,
    actor_user TEXT,
    detail TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON security_audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_event_type ON security_audit_log(event_type);
"#;

/// `ALTER` half of the same fact as [`AUDIT_LOG_SCHEMA`]'s `actor_user`
/// column, for stores that already exist. Applied idempotently (the probe in
/// `SecurityStore::migrate`), because a fresh store gets the column from the
/// `CREATE` above and would otherwise fail this with `duplicate column name`.
pub const AUDIT_LOG_ADD_ACTOR_SQL: &str =
    "ALTER TABLE security_audit_log ADD COLUMN actor_user TEXT";

pub const AUDIT_CLEANUP_SQL: &str =
    "DELETE FROM security_audit_log WHERE timestamp < strftime('%s', 'now') - ?1";

pub const DEFAULT_RETENTION_SECS: i64 = 30 * 24 * 3600;

pub const AUDIT_INSERT_SQL: &str = r#"
INSERT INTO security_audit_log (event_type, severity, source_ip, session_id, actor_user, detail)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_log_send_receive() {
        let (log, mut rx) = SecurityAuditLog::new(100);
        log.log(AuditEntry {
            event_type: AuditEventType::ExecBlocked,
            severity: AuditSeverity::Warn,
            source_ip: None,
            session_id: None,
            actor_user: None,
            detail: "Blocked request to 10.0.0.1".to_string(),
        });
        let entry = rx.recv().await.unwrap();
        assert_eq!(entry.event_type, AuditEventType::ExecBlocked);
        assert_eq!(entry.severity, AuditSeverity::Warn);
        assert!(entry.detail.contains("10.0.0.1"));
    }

    #[tokio::test]
    async fn test_audit_log_drops_when_full() {
        let (log, _rx) = SecurityAuditLog::new(1);
        log.log(AuditEntry {
            event_type: AuditEventType::AuthFailure,
            severity: AuditSeverity::Critical,
            source_ip: None,
            session_id: None,
            actor_user: None,
            detail: "first".into(),
        });
        log.log(AuditEntry {
            event_type: AuditEventType::AuthFailure,
            severity: AuditSeverity::Critical,
            source_ip: None,
            session_id: None,
            actor_user: None,
            detail: "second".into(),
        });
        assert_eq!(log.dropped_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn test_event_type_display() {
        assert_eq!(AuditEventType::ExecBlocked.to_string(), "exec_blocked");
        assert_eq!(
            AuditEventType::EnvInjectionDetected.to_string(),
            "env_injection"
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

    /// The two columns that make this variant worth having: WHO read and
    /// WHOSE session. An entry that names the session but not the actor is
    /// the shape the log already had, and it cannot answer the only question
    /// a ratified cross-user read raises.
    #[test]
    fn scoped_content_read_entry_names_the_actor_and_the_session_it_read() {
        let e = AuditEntry::scoped_content_read(
            "u-bob",
            Some("main:conv-alice".to_string()),
            "trace.get task=run-a",
        );
        assert_eq!(e.event_type, AuditEventType::ScopedContentRead);
        assert_eq!(e.actor_user.as_deref(), Some("u-bob"));
        assert_eq!(e.session_id.as_deref(), Some("main:conv-alice"));
        assert_eq!(e.event_type.to_string(), "scoped_content_read");
        // The record names the surface, never the transcript it disclosed.
        assert!(e.detail.contains("trace.get"));
    }

    #[test]
    fn rate_limited_entry_stamps_ip_and_type() {
        let e = AuditEntry::rate_limited("10.0.0.6", "flood guard: 10 unauthorized");
        assert_eq!(e.event_type, AuditEventType::RateLimited);
        assert_eq!(e.severity, AuditSeverity::Warn);
        assert_eq!(e.source_ip.as_deref(), Some("10.0.0.6"));
    }
}
