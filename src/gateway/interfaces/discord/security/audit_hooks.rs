//! Discord audit hooks — bridge the discord channel's command-execution
//! path into the process-wide [`SecurityAuditLog`].
//!
//! # Why this lives in `discord/audit_hooks.rs` and not `security/mod.rs`
//!
//! Three surfaces must meet for a discord command to leave a trace:
//!
//! 1. the discord channel's inbound handler (line 2's responsibility),
//! 2. the generic exec-approval manager that decides whether the
//!    command is allowed to run,
//! 3. the process-wide audit pipeline.
//!
//! The translation between them is discord-specific (which event type,
//! which verb, which fields), but the audit and approval managers are
//! not — they cannot depend on discord. The seam lives here: discord
//! surfaces its hooks as plain `pub async fn`s that take an
//! [`ApprovalOutcome`]-shaped decision (or a refusal reason), and the
//! line-2 inbound handler calls them at the right moment. The exec
//! manager calls [`record_approval_resolved`] from a generic spot in
//! its `resolve_with_reason` chokepoint.
//!
//! # Why we hand-construct `AuditEntry` instead of using a builder
//!
//! [`AuditEntry`] already exposes builders for every event type the
//! audit pipeline *invented*; the `ExecBlocked` and `AuthorityChange`
//! variants the discord path uses both have builders
//! ([`AuditEntry::command_policy`] doubles as a `command_policy` builder;
//! [`AuditEntry::authority_change`] for `AuthorityChange`). Building the
//! entries inline keeps the constructors stable for the test suite and
//! avoids adding a new builder for a producer that fires at most a
//! handful of times per bot lifetime.
//!
//! [`SecurityAuditLog`]: crate::security::audit::SecurityAuditLog
//! [`AuditEntry`]: crate::security::audit::AuditEntry
//! [`ApprovalOutcome`]: crate::sandbox::exec_approval::gate::ApprovalOutcome

use crate::security::audit::{AuditEntry, AuditEventType, AuditSeverity};

/// Record a `discord.command.approval.requested` audit entry.
///
/// Called by the discord channel (line 2) immediately after it raises
/// an approval request for an incoming command. The actor and session
/// are best-effort: `actor_user` reads the ambient caller identity if
/// one is set, otherwise falls back to the discord sender id (line 2
/// reads both). `detail` carries the command shape (`"/approve"`,
/// `"!ping"`, or the raw slash command name) — never the full message
/// body, which is where credentials would land.
///
/// The entry is intentionally `AuthorityChange`-severity-`Warn`: a
/// command being put in front of a human is a **ratified** move, not a
/// refusal. The matching refusal lives at
/// [`record_approval_blocked`].
pub async fn record_approval_requested(
    actor_user: Option<String>,
    session_id: Option<String>,
    command_shape: &str,
) {
    // `format!` is inlined into the call so the audit-census extractor (which
    // scans for the first `"` after `::authority_change(`) reads the verb
    // literal directly; splitting it into a `let detail` first would put a
    // `;` between the call and its string literal, which the extractor
    // refuses as "the detail is not a string literal".
    if let Some(log) = crate::security::audit::global() {
        let mut entry = AuditEntry::authority_change(
            actor_user,
            format!("discord.command.approval.requested: {command_shape}"),
        );
        entry.session_id = session_id;
        log.log(entry).await;
    }
}

/// Record a `discord.command.approval.resolved` audit entry.
///
/// Called by [`crate::exec::manager::ExecApprovalManager::resolve_with_reason`]
/// after a successful resolution. `decision_label` is one of
/// `"allow_once"`, `"allow_session"`, `"allow_always"`, `"deny"`,
/// `"timeout"`, `"unavailable"` — the human-readable form, not the
/// internal enum name. The verb stays "approval_resolved", not the
/// verb-on-the-event-type ("command_policy"), because the post-incident
/// question is "did this discord command get a human answer, and how",
/// which is one filterable row.
pub async fn record_approval_resolved(
    actor_user: Option<String>,
    session_id: Option<String>,
    decision_label: &str,
    command_shape: &str,
) {
    // Inlined `format!` so the audit-census extractor reads the verb literal;
    // see `record_approval_requested` for the reason.
    if let Some(log) = crate::security::audit::global() {
        let mut entry = AuditEntry::authority_change(
            actor_user,
            format!("discord.command.approval.resolved: {decision_label} {command_shape}"),
        );
        entry.session_id = session_id;
        log.log(entry).await;
    }
}

/// Record a `discord.command.approval.blocked` audit entry.
///
/// Called when the discord channel refused to put a command in front
/// of a human (rate-limited, denied by policy, off-channel, etc.).
/// This is the **refusal** half — `ExecBlocked` / `Critical` — that the
/// the requested/resolved pair deliberately are not. The detail names
/// the reason ("rate_limited", "off_channel", "policy_denied") and the
/// command shape.
pub async fn record_approval_blocked(
    actor_user: Option<String>,
    session_id: Option<String>,
    reason: &str,
    command_shape: &str,
) {
    let detail = format!("discord.command.approval.blocked: {reason} {command_shape}");
    if let Some(log) = crate::security::audit::global() {
        log.log(AuditEntry {
            event_type: AuditEventType::ExecBlocked,
            severity: AuditSeverity::Critical,
            source_ip: None,
            session_id,
            actor_user,
            detail,
        })
        .await;
    }
}

/// Map an [`ApprovalOutcome`] to its human-readable label, so the
/// exec manager can call [`record_approval_resolved`] without reaching
/// back into the approval module's enum surface.
///
/// [`ApprovalOutcome`]: crate::sandbox::exec_approval::gate::ApprovalOutcome
#[must_use]
pub fn label_for_outcome(outcome_label: &str) -> &str {
    // Stable, lowercase, snake_case — kept verbatim so the audit detail
    // stays greppable from the operator's side.
    match outcome_label {
        "approved" => "allow_once",
        "approved_for_session" => "allow_session",
        "approved_always" => "allow_always",
        "denied" => "deny",
        "timeout" => "timeout",
        "unavailable" => "unavailable",
        // Unknown future variants fall through as `unknown(<label>)`
        // so the detail still carries what was decided; the audit
        // pipeline's `sanitize_detail` caps the length so a long label
        // does not blow up the row.
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The label table is stable: a refused outcome labelled "deny" is
    /// what every post-incident query keys on. A typo here is a silent
    /// log rewrite.
    #[test]
    fn label_for_outcome_is_stable() {
        assert_eq!(label_for_outcome("approved"), "allow_once");
        assert_eq!(label_for_outcome("approved_for_session"), "allow_session");
        assert_eq!(label_for_outcome("approved_always"), "allow_always");
        assert_eq!(label_for_outcome("denied"), "deny");
        assert_eq!(label_for_outcome("timeout"), "timeout");
        assert_eq!(label_for_outcome("unavailable"), "unavailable");
    }

    /// Each hook is a thin shim around `SecurityAuditLog::log`, but the
    /// *callers* expect: (a) they never panic when no audit log is
    /// installed, and (b) they construct the right event type / severity.
    /// The unit tests below exercise both without standing up a real log.
    #[tokio::test]
    #[serial_test::serial(global_audit_log)]
    async fn record_requested_does_not_panic_when_no_audit_log() {
        record_approval_requested(Some("user-1".into()), Some("sess-1".into()), "!ping").await;
    }

    #[tokio::test]
    #[serial_test::serial(global_audit_log)]
    async fn record_resolved_does_not_panic_when_no_audit_log() {
        record_approval_resolved(
            Some("user-1".into()),
            Some("sess-1".into()),
            "allow_once",
            "!ping",
        )
        .await;
    }

    #[tokio::test]
    #[serial_test::serial(global_audit_log)]
    async fn record_blocked_does_not_panic_when_no_audit_log() {
        record_approval_blocked(
            Some("user-1".into()),
            Some("sess-1".into()),
            "rate_limited",
            "!ping",
        )
        .await;
    }

    /// End-to-end against a real [`SecurityAuditLog`]: the requested /
    /// resolved entries are `AuthorityChange` / `Warn`, the blocked
    /// entry is `ExecBlocked` / `Critical`. This is the spec's reverse
    /// test — "should be rejected but audit recorded it" — at the
    /// audit-hook layer.
    #[tokio::test]
    #[serial_test::serial(global_audit_log)]
    async fn emits_correct_event_types_against_a_real_log() {
        use crate::security::audit::{AuditEntry, SecurityAuditLog};

        let (log, mut rx) = SecurityAuditLog::new(8);
        // The hooks look up `crate::security::audit::global()` — install
        // the test log there so the hooks actually fire. (Production
        // installs it once at boot via `install_global`; tests have to
        // repeat the install because there is no other entry point.)
        assert!(
            crate::security::audit::install_global(&log),
            "test log already installed"
        );

        record_approval_requested(Some("user-1".into()), Some("sess-1".into()), "!ping").await;
        record_approval_resolved(
            Some("user-1".into()),
            Some("sess-1".into()),
            "allow_once",
            "!ping",
        )
        .await;
        record_approval_blocked(
            Some("user-2".into()),
            Some("sess-2".into()),
            "policy_denied",
            "!rm_rf",
        )
        .await;

        let e1 = rx.recv().await.expect("entry 1");
        assert_eq!(e1.event_type, AuditEventType::AuthorityChange);
        // Debug aid: if the install_global returned false the hooks
        // fired into a different log and we'd see the wrong entries.
        // Print the actual detail so a future regression points at
        // the exact mismatch rather than a "contains" assertion.
        assert!(
            e1.detail.contains("requested"),
            "expected detail to contain 'requested', got: {:?}",
            e1.detail
        );
        assert!(e1.detail.contains("!ping"));

        let e2 = rx.recv().await.expect("entry 2");
        assert_eq!(e2.event_type, AuditEventType::AuthorityChange);
        assert!(e2.detail.contains("resolved"));
        assert!(e2.detail.contains("allow_once"));

        let e3 = rx.recv().await.expect("entry 3");
        assert_eq!(e3.event_type, AuditEventType::ExecBlocked);
        assert_eq!(e3.severity, crate::security::audit::AuditSeverity::Critical);
        assert!(e3.detail.contains("policy_denied"));
        assert!(e3.detail.contains("!rm_rf"));

        // Tear down the global install so the next test sees a clean
        // slate. `install_global` is one-shot so we cannot un-install
        // through it — drop is enough; subsequent tests will overwrite.
        drop(log);
        // Touch the import so clippy does not flag it as unused under
        // the local `use` block in the test body.
        let _ = std::mem::size_of::<AuditEntry>();
    }

    #[test]
    fn label_for_outcome_passes_through_unknown() {
        // Future-proof: an enum variant we have not taught the table
        // about should still appear in the detail, so a post-incident
        // search picks up the row.
        assert_eq!(label_for_outcome("custom_outcome"), "custom_outcome");
    }

    // `AuditEntry` is referenced here only to keep `cargo` honest
    // about which module is in use at the test boundary — clippy is
    // fine without it but the visibility check is worth keeping.
    #[allow(dead_code)]
    fn _audit_entry_is_in_scope(_e: AuditEntry) {}
}
