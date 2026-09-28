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
    /// A write that moves the boundary of who can do what — as opposed to
    /// [`Self::ScopedContentRead`], a read across it.
    ///
    /// **Which verbs record one is deliberately not listed here.** The list is
    /// `tests::AUTHORITY_VERBS`, compared against a scan of every production
    /// `AuditEntry::authority_change(` call by
    /// `every_authority_change_verb_is_in_the_census_and_nothing_else_is`. This
    /// doc used to hand-copy eight verbs while the producers recorded
    /// twenty-two — `daemon.shutdown`, `config.patch` and `secrets.*` among
    /// them: a narrowing sentence beside a broader practice.
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
    /// The SSRF guard refused an outbound fetch — a URL the model (or an MCP
    /// server, or an operator config) asked for resolved to a blocked
    /// address: loopback, link-local, cloud metadata, a blocklisted host, a
    /// legacy IP literal.
    ///
    /// Emitted at the two validator chokepoints
    /// ([`crate::security::ssrf::validate_url_with_pinned`] and
    /// `safe_fetch`'s internal `validate_url_full`) — every `BlockedAddress`
    /// decision funnels through one of them, so the trail cannot be bypassed
    /// by taking a different entry point. Until this variant the refusal was
    /// a `tracing` line and a returned error: the fetcher saw the "no", but
    /// the post-incident question "did anything TRY to reach the metadata
    /// endpoint, and when" had no answer (audit I-3).
    ///
    /// `detail` names the host and the refusal reason, never the full URL —
    /// the query string is exactly where an exfiltrating URL would carry the
    /// loot. Severity is `Critical`: the request was an attack shape, stopped.
    SsrfBlocked,
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
            Self::SsrfBlocked => "ssrf_blocked",
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

    /// The SSRF guard refused an outbound fetch — see
    /// [`AuditEventType::SsrfBlocked`]. `host` only, never the full URL; the
    /// reason comes from the validator (`SsrfError::BlockedAddress`).
    /// `session_id` stays `None`: the validators run without one, and the
    /// host is the join column an investigator actually needs.
    #[must_use]
    pub fn ssrf_blocked(host: impl Into<String>, reason: impl Into<String>) -> Self {
        let host = host.into();
        let reason = reason.into();
        Self {
            event_type: AuditEventType::SsrfBlocked,
            severity: AuditSeverity::Critical,
            source_ip: None,
            session_id: None,
            // Same task-local the runtime guard reads: the fetch tool runs on
            // the spawned run task where CALLER_USER is dead, but the
            // run-start seeding nest makes the room author visible here.
            actor_user: crate::scope::current_room_author(),
            detail: format!("blocked fetch: host={host} reason={reason}"),
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

/// Record an SSRF refusal on the process-wide trail, if one is installed.
///
/// Called by the two SSRF validator chokepoints on every
/// [`crate::security::ssrf::SsrfError::BlockedAddress`]. `url_str` is the URL
/// the fetcher asked for; only its host reaches the log (see
/// [`AuditEventType::SsrfBlocked`] — the query string is where exfiltrated
/// data would ride). A host that does not parse is reported as
/// `"<unparseable>"`, which is itself the interesting fact.
pub async fn emit_ssrf_blocked(url_str: &str, reason: &str) {
    let Some(log) = global() else {
        return;
    };
    let host = url::Url::parse(url_str)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "<unparseable>".to_string());
    log.log(AuditEntry::ssrf_blocked(host, reason)).await;
}

/// Serialises the audit-asserting tests: each swaps in its own log, so two
/// running concurrently would steal each other's entries. Take this lock,
/// then [`replace_global_for_test`].
#[cfg(test)]
pub(crate) static AUDIT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Channel capacity for a test's own audit log.
///
/// Generous ON PURPOSE, and the size is the point. [`AUDIT_TEST_LOCK`]
/// serialises the tests that ASSERT on audit rows; it does not stop every
/// other test in the binary from PRODUCING them, and while an asserting test
/// has its log installed those foreign rows go into THIS channel.
/// [`SecurityAuditLog::log`] uses `try_send`, so a full channel silently
/// DROPS -- and what it drops may be the asserting test's own row.
///
/// Measured 2026-09-05: at capacity 16, `authority_changes_are_audited` saw
/// 2 of its own 3 rows inside a full `--lib` run and read that as the
/// handler having failed to write one.
#[cfg(test)]
pub(crate) const TEST_LOG_CAPACITY: usize = 1024;

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
            error!("Security audit log channel full, dropping entry (total dropped: {count})");
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

    /// Every production call that records an [`AuditEventType::AuthorityChange`],
    /// as `(file, verb)` — one row per call site, sorted.
    ///
    /// This is the list the variant's doc used to hand-copy (eight verbs,
    /// while the producers recorded twenty-two). It is now the only copy, and
    /// it is compared against a scan of the source rather than read by a
    /// person: a new call site, a removed one, or a renamed verb all go red in
    /// `every_authority_change_verb_is_in_the_census_and_nothing_else_is`.
    ///
    /// # What this census still cannot see
    ///
    /// A write that SHOULD record an authority change and calls nothing. "A
    /// write that changes who can do what" is a judgement, not a grep — the
    /// round-10 tool-face roster gap was exactly that shape. The census makes
    /// the existing trail exact; it does not find a missing one.
    const AUTHORITY_VERBS: &[(&str, &str)] = &[
        ("src/builtin_tools/agent_manage/update.rs", "agent_update"),
        ("src/builtin_tools/project_manage.rs", "project_manage.member_add"),
        ("src/builtin_tools/project_manage.rs", "project_manage.member_remove"),
        ("src/gateway/handlers/agents.rs", "agents.update"),
        ("src/gateway/handlers/cluster.rs", "cluster.deregister"),
        ("src/gateway/handlers/cluster.rs", "cluster.enroll"),
        ("src/gateway/handlers/config.rs", "config.patch"),
        ("src/gateway/handlers/daemon_control.rs", "daemon.shutdown"),
        ("src/gateway/handlers/gateway_devices.rs", "devices.revoke"),
        ("src/gateway/handlers/gateway_ticket.rs", "gateway.ticket.create"),
        ("src/gateway/handlers/gateway_ticket.rs", "gateway.ticket.revoke"),
        ("src/gateway/handlers/gateway_token.rs", "gateway.token.rotate"),
        ("src/gateway/handlers/pairing.rs", "channel.pairing.approve"),
        ("src/gateway/handlers/pairing.rs", "channel.pairing.revoke"),
        ("src/gateway/handlers/projects.rs", "projects.member.add"),
        ("src/gateway/handlers/projects.rs", "projects.member.remove"),
        ("src/gateway/handlers/projects_channel.rs", "projects.channel.bind"),
        ("src/gateway/handlers/projects_channel.rs", "projects.channel.unbind"),
        ("src/gateway/handlers/secrets.rs", "secrets.delete"),
        ("src/gateway/handlers/secrets.rs", "secrets.set"),
        ("src/gateway/handlers/users.rs", "users.create"),
        ("src/gateway/handlers/users.rs", "users.update"),
        ("src/gateway/handlers/users.rs", "users.update"),
        ("src/gateway/handlers/users.rs", "users.update"),
        ("src/gateway/handlers/users.rs", "users.update"),
    ];

    /// The verb each `AuditEntry::authority_change(` call in `code` records:
    /// the first string literal of the call, up to its first `": "`.
    ///
    /// Refuses — rather than guessing — every shape it does not recognise, so
    /// an unreadable call site is a red test and not a silently skipped one:
    /// a call spelled through an alias, a detail that is not a literal (a `;`
    /// before any `"` means the call ended without one), and a literal whose
    /// prefix is not a dotted lowercase verb followed by `": "`.
    fn verbs_recorded_in(code: &str) -> Result<Vec<String>, String> {
        const CALL: &str = "::authority_change(";
        const SPELLED: &str = "AuditEntry::authority_change(";
        let mut verbs = Vec::new();
        let mut rest = code;
        while let Some(at) = rest.find(CALL) {
            if !rest[..at + CALL.len()].ends_with(SPELLED) {
                return Err(format!("a call not spelled `{SPELLED}` — this scanner cannot read it"));
            }
            let after = &rest[at + CALL.len()..];
            let quote = after.find('"').ok_or("no string literal after the call")?;
            if after[..quote].contains(';') {
                return Err("the detail is not a string literal (`;` before any `\"`)".into());
            }
            let body = &after[quote + 1..];
            let end = body.find('"').ok_or("unterminated detail literal")?;
            let literal = &body[..end];
            let (verb, _) = literal
                .split_once(": ")
                .ok_or_else(|| format!("detail {literal:?} has no `<verb>: ` prefix"))?;
            let well_formed = !verb.is_empty()
                && verb
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_');
            if !well_formed {
                return Err(format!("detail prefix {verb:?} is not a dotted lowercase verb"));
            }
            verbs.push(verb.to_string());
            rest = &body[end + 1..];
        }
        Ok(verbs)
    }

    #[test]
    fn every_authority_change_verb_is_in_the_census_and_nothing_else_is() {
        let root = repo_root();
        let sources = crate::utils::source_scan::rust_sources_under(&root.join("src"));
        assert!(
            sources.len() > 100,
            "the scan found {} files — it is not looking at the source tree it thinks it is",
            sources.len()
        );
        let mut found: Vec<(String, String)> = Vec::new();
        for (rel, text) in sources {
            // This file defines the constructor and the census; not a producer.
            if rel == "src/security/audit.rs" {
                continue;
            }
            let production = crate::utils::source_scan::production_text(&root.join(&rel), &text);
            let code = crate::utils::source_scan::strip_comment_lines(&production);
            let verbs = verbs_recorded_in(&code).unwrap_or_else(|e| panic!("{rel}: {e}"));
            found.extend(verbs.into_iter().map(|v| (rel.clone(), v)));
        }
        found.sort();
        let mut expected: Vec<(String, String)> = AUTHORITY_VERBS
            .iter()
            .map(|(f, v)| ((*f).to_string(), (*v).to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            found, expected,
            "the authority-change call sites and AUTHORITY_VERBS disagree. A new \
             producer gets a row (with the verb it records); a removed one loses its \
             row. This table is the only list of what `aleph audit` can show under \
             `authority_change` — the variant's doc points here instead of copying it."
        );
    }

    /// The extractor is a guard, so it has to be shown going red on each
    /// shape it claims to refuse (criterion §3: a guard never falsified is
    /// not a guard).
    #[test]
    fn the_verb_extractor_reads_the_shipped_shapes_and_refuses_the_rest() {
        let shipped = "log.log(crate::security::audit::AuditEntry::authority_change(\n\
                       actor,\n format!(\n \"users.update: role {} {}→{}\",\n a, b, c\n ),\n));\n\
                       log.log(AuditEntry::authority_change(None, \"daemon.shutdown: x\".to_string()));";
        assert_eq!(
            verbs_recorded_in(shipped).unwrap(),
            vec!["users.update".to_string(), "daemon.shutdown".to_string()]
        );
        assert!(verbs_recorded_in("Entry::authority_change(None, \"a.b: c\")").is_err(), "alias");
        assert!(
            verbs_recorded_in("AuditEntry::authority_change(None, detail);\nlet s = \"x.y: z\";").is_err(),
            "a non-literal detail must not borrow the next literal in the file"
        );
        assert!(verbs_recorded_in("AuditEntry::authority_change(None, \"no verb here\")").is_err());
        assert!(verbs_recorded_in("AuditEntry::authority_change(None, \"Users.Update: x\")").is_err());
    }

    /// Every file expected to emit an SSRF-block audit entry. Much smaller
    /// than [`AUTHORITY_VERBS`] because the design is a chokepoint, not a
    /// census of call sites: every `SsrfError::BlockedAddress` funnels through
    /// one of these two validators, so two files cover every refusal path
    /// (direct fetch, redirect hop, MCP SSE transport).
    const SSRF_PRODUCERS: &[&str] = &["src/security/ssrf/mod.rs", "src/security/ssrf/fetch.rs"];

    #[test]
    fn every_declared_ssrf_producer_still_emits() {
        let root = repo_root();
        for path in SSRF_PRODUCERS {
            let full = root.join(path);
            let src = std::fs::read_to_string(&full).unwrap_or_else(|e| {
                panic!("{path} (declared SSRF audit producer) unreadable: {e}")
            });
            assert!(
                code_only(&src).contains("emit_ssrf_blocked("),
                "{path} is a declared SSRF audit chokepoint and no longer emits. The refusal \n                 still happens — what vanished is the trail of it (audit I-3)."
            );
        }
    }

    #[test]
    fn no_ssrf_producer_exists_outside_the_chokepoints() {
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
        let mut files = Vec::new();
        walk(&root.join("src"), &mut files);

        // This file defines the emitter; it is not a producer.
        let this_file = root.join("src/security/audit.rs");
        let declared: std::collections::HashSet<_> =
            SSRF_PRODUCERS.iter().map(|p| root.join(p)).collect();

        let mut undeclared = Vec::new();
        for file in files {
            if file == this_file || declared.contains(&file) {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&file) else {
                continue;
            };
            if code_only(&src).contains("emit_ssrf_blocked(") {
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
            "these files emit SSRF audit entries outside the validator chokepoints: {undeclared:?}. \
             Route the refusal through validate_url_with_pinned / validate_url_full instead — \
             a second emission path is how the trail forks."
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
        })
        .await;
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
        })
        .await;
        log.log(AuditEntry {
            event_type: AuditEventType::AuthFailure,
            severity: AuditSeverity::Critical,
            source_ip: None,
            session_id: None,
            actor_user: None,
            detail: "second".into(),
        })
        .await;
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
    fn ssrf_blocked_entry_names_host_and_reason_never_the_url() {
        let e = AuditEntry::ssrf_blocked("169.254.169.254", "blocked by policy");
        assert_eq!(e.event_type, AuditEventType::SsrfBlocked);
        assert_eq!(e.severity, AuditSeverity::Critical);
        assert_eq!(e.event_type.to_string(), "ssrf_blocked");
        assert!(e.detail.contains("169.254.169.254"));
        assert!(e.detail.contains("blocked by policy"));
    }

    /// The fold's contract is three properties, not one exact output string.
    /// This guard originally asserted the string `"... three end"` and so also
    /// demanded a trailing trim that [`sanitize_detail`] never promised — its
    /// doc says each control *run* becomes one space, and a trailing `\u{7}`
    /// is such a run. It was therefore red from the commit that introduced it
    /// (`a2d83f830`): a guard coupled to incidental whitespace instead of to
    /// the thing it protects. Leading/trailing whitespace is deliberately not
    /// asserted here; `sanitize_detail_leaves_clean_short_detail_untouched`
    /// pins the matching promise that clean text is not whitespace-normalised.
    #[test]
    fn sanitize_detail_collapses_control_runs() {
        // The space before `\r\n` is the case the original input never had:
        // the fold must carry "already sitting on whitespace" across a
        // *non-control* space, or the run that follows emits a second one.
        let mut d = "line one\n\nline two \r\nline three\tend\u{7}".to_string();
        sanitize_detail(&mut d);

        // 1. Nothing that can break the one-row-one-event shape, or drive the
        //    terminal that later prints this row, survives.
        assert!(
            !d.chars().any(char::is_control),
            "control char survived the fold: {d:?}"
        );
        // 2. A run collapses to exactly one space — including a run that
        //    starts where the input already had a space.
        assert!(!d.contains("  "), "control run did not collapse: {d:?}");
        // 3. Non-control content survives verbatim and in order.
        assert_eq!(
            d.split_whitespace().collect::<Vec<_>>(),
            ["line", "one", "line", "two", "line", "three", "end"],
            "fold altered non-control content: {d:?}"
        );
    }

    #[test]
    fn sanitize_detail_caps_length_with_marker_on_char_boundary() {
        // 5 KiB of ASCII: capped at 4 KiB + marker.
        let mut d = "a".repeat(5 * 1024);
        sanitize_detail(&mut d);
        assert!(d.ends_with(TRUNCATION_MARKER));
        assert_eq!(d.len(), MAX_DETAIL_LEN + TRUNCATION_MARKER.len());

        // Multi-byte content *straddling* the cap. The original form used
        // `é` (2 bytes), which never straddles: MAX_DETAIL_LEN is even, so
        // the cut is already on a boundary and the boundary walk never runs —
        // the assertion could not tell the walk apart from a bare `truncate`.
        // `€` is 3 bytes and MAX_DETAIL_LEN % 3 == 1, so the cut must walk back.
        let mut d = "€".repeat(2 * 1024); // 6 KiB of three-byte chars
        sanitize_detail(&mut d);
        let body_len = MAX_DETAIL_LEN - MAX_DETAIL_LEN % "€".len();
        assert!(d.ends_with(TRUNCATION_MARKER));
        assert_eq!(
            d.len(),
            body_len + TRUNCATION_MARKER.len(),
            "cut did not land on the largest char boundary at or below the cap"
        );
        assert!(
            d.trim_end_matches(TRUNCATION_MARKER)
                .chars()
                .all(|c| c == '€'),
            "cut mangled a char instead of walking back to its boundary"
        );
    }

    /// The other half of the fold's contract: control chars are folded, plain
    /// whitespace is *not* normalised. The doubled and trailing spaces below
    /// are the falsifiable part — they go red if a future edit adds a global
    /// `trim`/whitespace-squash, which is the change that would silently make
    /// [`sanitize_detail`] lossy for details that never contained a control
    /// char in the first place.
    #[test]
    fn sanitize_detail_leaves_clean_short_detail_untouched() {
        let mut d = "blocked fetch:  host=example.com  reason=loopback ".to_string();
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
        })
        .await;
        let entry = rx.recv().await.unwrap();
        assert!(!entry.detail.contains('\n'));
        assert!(entry.detail.ends_with(TRUNCATION_MARKER));
    }
}
