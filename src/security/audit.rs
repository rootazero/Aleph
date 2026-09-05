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
    /// A write that changes who can do what: principal created, role
    /// promoted/demoted, principal deactivated/reactivated, room roster
    /// add/remove, `allowed_users` rewritten, channel sender approved with a
    /// user binding, device revoked, bootstrap ticket minted.
    ///
    /// `ScopedContentRead` made cross-user READS accountable; the writes that
    /// move the boundary itself were silent until this variant — every one of
    /// them succeeded, took effect immediately, and left no record anywhere
    /// (round-4 ledger item ⑦). One variant, not one per verb: the `detail`
    /// names the verb and its target, and a single `event_type` answers the
    /// post-incident question "what authority changed, in order" with one
    /// `WHERE` clause. Never carries secrets (ticket codes, device tokens) —
    /// it names the object of the change, not the credential.
    AuthorityChange,
    ScopedContentRead,
    /// The sandbox command hard-filter ([`crate::sandbox::command_policy`])
    /// refused a command, or matched a `Warn`-tier rule and let it through.
    ///
    /// Both dispositions share one variant so a single `WHERE event_type =
    /// 'command_policy'` answers the post-incident question — "what did the
    /// command filter see, in order" — with the severity column separating the
    /// refusals from the audited pass-throughs. Splitting them would mean two
    /// queries to reconstruct one timeline.
    ///
    /// [`Self::ExecBlocked`] is deliberately *not* reused: its producers are
    /// the inbound/outbound content-leak detector in
    /// [`crate::security::runtime_guard`], and folding a second, unrelated
    /// meaning into that column would leave it unable to answer either
    /// question cleanly.
    ///
    /// Until this variant the `Warn` tier — whose entire purpose is to audit
    /// rather than refuse — left nothing behind but a `tracing` line, i.e. the
    /// paper trail it advertises existed only for whoever was watching stdout
    /// at the time. `detail` names the matched rules and the program; it never
    /// carries the command text, which is where the secrets would be.
    CommandPolicy,
    /// The audit pipeline itself dropped entries because the channel was
    /// full (audit I-4). Synthesised by the drain task, never by a content
    /// producer: the drain observes `SecurityAuditLog::dropped_count`
    /// advancing and inserts one row recording the delta, so a degraded
    /// trail says so in the trail itself instead of failing silently open.
    ///
    /// Severity is `Critical`: an incomplete audit log is the one fact an
    /// operator must never have to infer from absence.
    AuditLogDropped,
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
            Self::AuthorityChange => "authority_change",
            Self::ScopedContentRead => "scoped_content_read",
            Self::CommandPolicy => "command_policy",
            Self::AuditLogDropped => "audit_log_dropped",
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

    /// An authenticated principal changed who can do what — see
    /// [`AuditEventType::AuthorityChange`].
    ///
    /// `actor_user` is the caller who performed the change (`None` only for
    /// in-process/test producers with no resolved caller — the gateway
    /// producers all run on the request task, where `CALLER_USER` is alive).
    /// `detail` is `"<verb>: <target> [<before>→<after>]"` — enough to
    /// reconstruct what moved, never the credential material itself.
    /// Severity is `Warn` for the same reason as `ScopedContentRead`: these
    /// are ratified operations being made accountable, not violations.
    #[must_use]
    pub fn authority_change(actor_user: Option<String>, detail: impl Into<String>) -> Self {
        Self {
            event_type: AuditEventType::AuthorityChange,
            severity: AuditSeverity::Warn,
            source_ip: None,
            session_id: None,
            actor_user,
            detail: detail.into(),
        }
    }

    /// The command hard-filter decided about one command — see
    /// [`AuditEventType::CommandPolicy`].
    ///
    /// `blocked` selects the severity: a refusal is `Critical`, an audited
    /// pass-through is `Warn`. `session_id` is the session whose tool call it
    /// was; `detail` is `"<disposition> <program>: <rule>, <rule>"`.
    ///
    /// `actor_user` is `None` by construction and that is honest rather than a
    /// gap: this producer runs on the sandbox execution task, downstream of the
    /// `tokio::spawn` that starts a run, where
    /// [`crate::gateway::caller_identity::CALLER_USER`] is dead — the same
    /// task-local hole every tool-face predicate has. The session key is the
    /// join column back to whoever owns the run.
    #[must_use]
    pub fn command_policy(
        blocked: bool,
        session_id: Option<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            event_type: AuditEventType::CommandPolicy,
            severity: if blocked {
                AuditSeverity::Critical
            } else {
                AuditSeverity::Warn
            },
            source_ip: None,
            session_id,
            actor_user: None,
            detail: detail.into(),
        }
    }

    /// The audit channel filled and entries were dropped — synthesised by
    /// the drain, see [`AuditEventType::AuditLogDropped`]. `dropped` is the
    /// newly observed delta, `total` the running counter. `detail` carries
    /// only the two numbers; there is nothing else honest to say — the
    /// dropped entries are gone by definition.
    #[must_use]
    pub fn audit_log_dropped(dropped: u64, total: u64) -> Self {
        Self {
            event_type: AuditEventType::AuditLogDropped,
            severity: AuditSeverity::Critical,
            source_ip: None,
            session_id: None,
            actor_user: None,
            detail: format!("audit channel full: {dropped} entries dropped (total {total})"),
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
    /// When `true`, [`SecurityAuditLog::log`] awaits channel capacity instead
    /// of dropping — the operator's fail-closed opt-in (`[security]
    /// audit_block_on_full`). Default `false`: a flooded audit pipeline
    /// degrades the trail, never the system the trail watches.
    block_on_full: bool,
}

/// Process-wide handle for the authority-change producers.
///
/// `GatewayServer::set_audit_log` + captured clones serve the two producers
/// that live next to the server (WS auth path, trace handlers). The
/// `AuthorityChange` producers are eight call sites across five handler
/// families (`users` / `projects` / `pairing` / `gateway_ticket` / agent
/// `allowed_users`) registered from different builder files — threading an
/// `Option` through each registration is the "seven construction points"
/// shape this repo has already paid for twice (the seventh `SecretMasker::
/// new()` site; the `install_operator_patterns` fix), so the handle is
/// installed once at boot instead, exactly like `identity::ledger::install`
/// and the `goal::global()` / `looping::global()` / `cron::global()` trio
/// `freeze_owned_background_work` already reads.
static GLOBAL_AUDIT: std::sync::RwLock<Option<SecurityAuditLog>> = std::sync::RwLock::new(None);

/// Install the process-wide handle. Called once at boot, immediately after
/// `GatewayServer::set_audit_log`, with a clone of the same log. Returns
/// `false` if already installed (a second server in one process keeps the
/// first handle rather than silently splitting the trail).
pub fn install_global(log: &SecurityAuditLog) -> bool {
    let mut guard = GLOBAL_AUDIT.write().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return false;
    }
    *guard = Some(log.clone());
    true
}

/// The process-wide handle, `None` before boot installs it (unit tests,
/// probe servers) — producers treat `None` as "no trail", matching the
/// `Option<SecurityAuditLog>` semantics of the threaded path.
#[must_use]
pub fn global() -> Option<SecurityAuditLog> {
    GLOBAL_AUDIT
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Serialises the audit-asserting tests: each swaps in its own log, so two
/// running concurrently would steal each other's entries. Take this lock,
/// then [`replace_global_for_test`].
#[cfg(test)]
pub(crate) static AUDIT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Swap the installed handle (test-only — production installs exactly once).
/// The caller must hold [`AUDIT_TEST_LOCK`] and MUST call
/// [`clear_global_for_test`] before releasing it, so a later non-audit test
/// never observes a handle whose receiver was dropped.
#[cfg(test)]
pub(crate) fn replace_global_for_test(log: &SecurityAuditLog) {
    *GLOBAL_AUDIT.write().unwrap_or_else(|e| e.into_inner()) = Some(log.clone());
}

/// See [`replace_global_for_test`].
#[cfg(test)]
pub(crate) fn clear_global_for_test() {
    *GLOBAL_AUDIT.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Maximum length of a persisted audit `detail`, after sanitisation.
///
/// `detail` is a `NOT NULL TEXT` column with no database-side bound; before
/// this cap, a producer handed a multi-kilobyte payload (a model response
/// fragment, a pasted document) inflated the audit table unboundedly — the
/// table whose entire value is being cheap to query after an incident.
const MAX_DETAIL_LEN: usize = 4 * 1024;

/// Marker appended when a `detail` exceeded [`MAX_DETAIL_LEN`], so a truncated
/// row reads as truncated rather than as a suspiciously clean cut.
const TRUNCATION_MARKER: &str = "…[truncated]";

/// Sanitise an audit `detail` in place (audit I-9).
///
/// Two transforms, both lossy-on-purpose:
///
/// 1. Control characters (newlines, carriage returns, tabs, ANSI escapes)
///    collapse to a single space each run. A multi-line payload would
///    otherwise break the one-row-one-event shape every consumer of the table
///    (`aleph audit`, SQL `WHERE` scans) relies on, and escape sequences are
///    how a logged string attacks the terminal that later prints it.
/// 2. The result is capped at [`MAX_DETAIL_LEN`] bytes on a char boundary,
///    with [`TRUNCATION_MARKER`] appended when anything was cut.
///
/// Applied centrally in [`SecurityAuditLog::log`] — the single chokepoint
/// every production entry flows through — so producers stay honest by
/// construction instead of by per-site discipline.
fn sanitize_detail(detail: &mut String) {
    let needs_fold = detail.chars().any(|c| c.is_control());
    if needs_fold {
        let mut folded = String::with_capacity(detail.len());
        let mut last_was_space = false;
        for c in detail.chars() {
            if c.is_control() {
                if !last_was_space {
                    folded.push(' ');
                    last_was_space = true;
                }
            } else {
                last_was_space = c == ' ';
                folded.push(c);
            }
        }
        *detail = folded;
    }
    if detail.len() > MAX_DETAIL_LEN {
        let mut cut = MAX_DETAIL_LEN;
        while !detail.is_char_boundary(cut) {
            cut -= 1;
        }
        detail.truncate(cut);
        detail.push_str(TRUNCATION_MARKER);
    }
}

impl SecurityAuditLog {
    #[must_use]
    pub fn new(buffer_size: usize) -> (Self, mpsc::Receiver<AuditEntry>) {
        Self::new_with_policy(buffer_size, false)
    }

    /// [`new`] plus the fail-closed knob — see the `block_on_full` field.
    #[must_use]
    pub fn new_with_policy(
        buffer_size: usize,
        block_on_full: bool,
    ) -> (Self, mpsc::Receiver<AuditEntry>) {
        let (sender, receiver) = mpsc::channel(buffer_size);
        (
            Self {
                sender,
                dropped_count: Arc::new(AtomicU64::new(0)),
                block_on_full,
            },
            receiver,
        )
    }

    /// Total entries dropped because the channel was full (or gone). The
    /// drain task mirrors this counter into the table itself as
    /// [`AuditEventType::AuditLogDropped`] rows; the getter is for live
    /// observers (diagnostics, tests) that cannot wait for the drain.
    pub fn dropped_count(&self) -> u64 {
        self.dropped_count.load(Ordering::Acquire)
    }

    /// Shared handle to the drop counter, for the drain task that turns
    /// counter deltas into [`AuditEventType::AuditLogDropped`] rows.
    #[must_use]
    pub fn dropped_counter(&self) -> Arc<AtomicU64> {
        self.dropped_count.clone()
    }

    fn note_dropped(&self) {
        let count = self.dropped_count.fetch_add(1, Ordering::AcqRel) + 1;
        if count == 1 || count.is_multiple_of(100) {
            error!(
                "Security audit log channel full, dropping entry (total dropped: {count})"
            );
        }
    }

    /// `detail` is sanitised here, at the chokepoint every production entry
    /// flows through, rather than at each producer — see [`sanitize_detail`].
    ///
    /// Async because of `block_on_full`: with the knob off (default) this is
    /// a `try_send` that completes immediately; with it on, a full channel
    /// applies backpressure to the producer instead of dropping the entry.
    pub async fn log(&self, mut entry: AuditEntry) {
        sanitize_detail(&mut entry.detail);
        if self.block_on_full {
            // A send error means the receiver is gone — the entry is lost
            // exactly as if the channel had been full, so count it as one.
            if self.sender.send(entry).await.is_err() {
                self.note_dropped();
            }
        } else if self.sender.try_send(entry).is_err() {
            self.note_dropped();
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

    /// Every file that is expected to record an authority change, and the verb
    /// it records.
    ///
    /// # What this census can and cannot do
    ///
    /// The **backward** direction is mechanical and is the half that catches
    /// drift on its own: any file that calls `authority_change(` and is not
    /// listed here fails, so a new producer cannot appear un-reviewed.
    ///
    /// The **forward** direction is a name list, and name lists only describe
    /// the world on the day they are written — "a write that changes who can do
    /// what" is a judgement, not a grep. That limit is declared rather than
    /// papered over. What the list does buy is the failure this round found:
    /// [`AuditEventType::AuthorityChange`]'s own doc names *device revoked* in
    /// its list of covered writes, and no producer existed. Prose naming a
    /// behaviour is not the behaviour, and a doc comment has no test.
    const AUTHORITY_PRODUCERS: &[(&str, &str)] = &[
        (
            "src/gateway/handlers/users.rs",
            "principal created / role / status",
        ),
        (
            "src/gateway/handlers/projects.rs",
            "room roster add / remove (RPC)",
        ),
        (
            "src/builtin_tools/project_manage.rs",
            "room roster add / remove (tool)",
        ),
        (
            "src/gateway/handlers/projects_channel.rs",
            "room bound to / released from a channel conversation",
        ),
        (
            "src/gateway/handlers/pairing.rs",
            "channel sender approve / revoke",
        ),
        (
            "src/gateway/handlers/gateway_ticket.rs",
            "bootstrap ticket minted",
        ),
        (
            "src/gateway/handlers/gateway_devices.rs",
            "device credential revoked",
        ),
        (
            "src/gateway/handlers/agents.rs",
            "allowed_users rewritten (RPC)",
        ),
        (
            "src/builtin_tools/agent_manage/update.rs",
            "allowed_users rewritten (tool)",
        ),
        // Added by 9d3bdc3a5 ("audit high-privilege mutations"), which wrote
        // the calls but not the rows — the census went red at that commit and
        // stayed red, which is the census working. Verbs below are the ones
        // each `authority_change` actually records, not a summary of the file.
        (
            "src/gateway/handlers/cluster.rs",
            "cluster node device credential mint (enroll) / revoke (deregister)",
        ),
        (
            "src/gateway/handlers/config.rs",
            "config section rewritten (may touch auth / provider keys / channel wiring)",
        ),
        (
            "src/gateway/handlers/daemon_control.rs",
            "daemon shutdown requested (ends every connected session)",
        ),
        (
            "src/gateway/handlers/gateway_token.rs",
            "shared gateway token rotated / every paired device revoked",
        ),
        (
            "src/gateway/handlers/secrets.rs",
            "vault secret set / delete (key name only)",
        ),
    ];

    /// Strip `//` line comments before scanning.
    ///
    /// A scanner judges code; a comment is documentation. Without this, the
    /// sentence explaining why a producer was removed would satisfy the check
    /// that the producer still exists — the exact way a guard reports green on
    /// the bug it was written to catch.
    fn code_only(src: &str) -> String {
        src.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn repo_root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn every_declared_authority_producer_still_records_one() {
        let root = repo_root();
        for (path, verb) in AUTHORITY_PRODUCERS {
            let full = root.join(path);
            let src = std::fs::read_to_string(&full)
                .unwrap_or_else(|e| panic!("{path} (declared producer of {verb}) unreadable: {e}"));
            assert!(
                code_only(&src).contains("authority_change("),
                "{path} is the declared producer of the authority change {verb:?}, and it no \
                 longer records one. Either restore the call or remove the row — an authority \
                 write that leaves no record is invisible to `aleph audit`, which is the only \
                 surface that can answer what changed."
            );
        }
    }

    #[test]
    fn no_authority_producer_exists_outside_the_census() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }

        let root = repo_root();
        let src_root = root.join("src");
        let mut files = Vec::new();
        walk(&src_root, &mut files);
        assert!(
            files.len() > 100,
            "the scan found {} files — it is not looking at the source tree it thinks it is",
            files.len()
        );

        // This file defines the constructor and the census, so it names the
        // call unavoidably; it is not a producer.
        let this_file = root.join("src/security/audit.rs");
        let declared: std::collections::HashSet<_> = AUTHORITY_PRODUCERS
            .iter()
            .map(|(p, _)| root.join(p))
            .collect();

        let mut undeclared = Vec::new();
        for file in files {
            if file == this_file || declared.contains(&file) {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&file) else {
                continue;
            };
            if code_only(&src).contains("authority_change(") {
                undeclared.push(
                    file.strip_prefix(&root)
                        .unwrap_or(&file)
                        .display()
                        .to_string(),
                );
            }
        }
        assert!(
            undeclared.is_empty(),
            "these files record authority changes but are not in AUTHORITY_PRODUCERS: {undeclared:?}. \
             Add them with the verb they record — the census is how the next round learns which \
             writes are supposed to leave a trail."
        );
    }

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
        }).await;
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
        }).await;
        log.log(AuditEntry {
            event_type: AuditEventType::AuthFailure,
            severity: AuditSeverity::Critical,
            source_ip: None,
            session_id: None,
            actor_user: None,
            detail: "second".into(),
        }).await;
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

    #[test]
    fn sanitize_detail_collapses_control_runs() {
        let mut d = "line one\n\nline two\r\nline three\tend\u{7}".to_string();
        sanitize_detail(&mut d);
        assert_eq!(d, "line one line two line three end");
    }

    #[test]
    fn sanitize_detail_caps_length_with_marker_on_char_boundary() {
        // 5 KiB of ASCII: capped at 4 KiB + marker.
        let mut d = "a".repeat(5 * 1024);
        sanitize_detail(&mut d);
        assert!(d.ends_with(TRUNCATION_MARKER));
        assert_eq!(d.len(), MAX_DETAIL_LEN + TRUNCATION_MARKER.len());

        // Multi-byte content straddling the cap: the cut must land on a char
        // boundary (no panic, no invalid UTF-8).
        let mut d = "é".repeat(3 * 1024); // 6 KiB of two-byte chars
        sanitize_detail(&mut d);
        assert!(d.ends_with(TRUNCATION_MARKER));
        assert!(d.len() <= MAX_DETAIL_LEN + TRUNCATION_MARKER.len());
    }

    #[test]
    fn sanitize_detail_leaves_clean_short_detail_untouched() {
        let mut d = "blocked fetch: host=example.com reason=loopback".to_string();
        let before = d.clone();
        sanitize_detail(&mut d);
        assert_eq!(d, before);
    }

    #[tokio::test]
    async fn log_sanitises_detail_before_send() {
        let (log, mut rx) = SecurityAuditLog::new(4);
        log.log(AuditEntry {
            event_type: AuditEventType::ExecBlocked,
            severity: AuditSeverity::Warn,
            source_ip: None,
            session_id: None,
            actor_user: None,
            detail: format!("multi\nline\n{}", "x".repeat(8 * 1024)),
        }).await;
        let entry = rx.recv().await.unwrap();
        assert!(!entry.detail.contains('\n'));
        assert!(entry.detail.ends_with(TRUNCATION_MARKER));
    }
}
