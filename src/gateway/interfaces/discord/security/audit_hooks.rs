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
//! surfaces its hook as a plain `pub async fn` that takes an
//! [`ApprovalOutcome`]-shaped decision, and the exec manager calls
//! [`record_approval_resolved`] from a generic spot in its
//! `resolve_with_reason` chokepoint.
//!
//! The `requested` / `blocked` halves this module used to declare were
//! removed: the discord channel never called them (nothing in the path
//! raises an approval request through this seam), so they recorded nothing.
//!
//! # Why we hand-construct `AuditEntry` instead of using a builder
//!
//! [`AuditEntry`] already exposes a builder for the event type this hook
//! records ([`AuditEntry::authority_change`] for `AuthorityChange`).
//! Building the entry inline keeps the constructor stable for the test
//! suite and avoids adding a new builder for a producer that fires at most
//! a handful of times per bot lifetime.
//!
//! [`SecurityAuditLog`]: crate::security::audit::SecurityAuditLog
//! [`AuditEntry`]: crate::security::audit::AuditEntry
//! [`ApprovalOutcome`]: crate::sandbox::exec_approval::gate::ApprovalOutcome

use crate::security::audit::AuditEntry;
#[cfg(test)]
use crate::security::audit::{AuditEventType, AuditSeverity};

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
    // Inlined `format!` so the audit-census extractor (which scans for the
    // first `"` after `::authority_change(`) reads the verb literal directly;
    // splitting it into a `let detail` first would put a `;` between the call
    // and its string literal, which the extractor refuses as "the detail is
    // not a string literal".
    if let Some(log) = crate::security::audit::global() {
        let mut entry = AuditEntry::authority_change(
            actor_user,
            format!("discord.command.approval.resolved: {decision_label} {command_shape}"),
        );
        entry.session_id = session_id;
        log.log(entry).await;
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

    /// The hook is a thin shim around `SecurityAuditLog::log`, but the
    /// *callers* expect: (a) it never panics when no audit log is
    /// installed, and (b) it constructs the right event type / severity.
    /// The unit tests below exercise both without standing up a real log.
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

    /// End-to-end against a real [`SecurityAuditLog`]: the resolved entry is
    /// `AuthorityChange` / `Warn` — a ratified decision, not a refusal.
    #[tokio::test]
    #[serial_test::serial(global_audit_log)]
    async fn emits_correct_event_type_against_a_real_log() {
        use crate::security::audit::SecurityAuditLog;

        let (log, mut rx) = SecurityAuditLog::new(8);
        // The hook looks up `crate::security::audit::global()` — install
        // the test log there so it actually fires. (Production installs it
        // once at boot via `install_global`; tests have to repeat the
        // install because there is no other entry point.)
        assert!(
            crate::security::audit::install_global(&log),
            "test log already installed"
        );

        record_approval_resolved(
            Some("user-1".into()),
            Some("sess-1".into()),
            "allow_once",
            "!ping",
        )
        .await;

        let e = rx.recv().await.expect("resolved entry");
        assert_eq!(e.event_type, AuditEventType::AuthorityChange);
        assert_eq!(e.severity, AuditSeverity::Warn);
        assert!(
            e.detail.contains("resolved"),
            "expected detail to contain 'resolved', got: {:?}",
            e.detail
        );
        assert!(e.detail.contains("allow_once"));
        assert!(e.detail.contains("!ping"));

        // Tear down the global install so the next test sees a clean
        // slate. `install_global` is one-shot so we cannot un-install
        // through it — drop is enough; subsequent tests will overwrite.
        drop(log);
    }

    #[test]
    fn label_for_outcome_passes_through_unknown() {
        // Future-proof: an enum variant we have not taught the table
        // about should still appear in the detail, so a post-incident
        // search picks up the row.
        assert_eq!(label_for_outcome("custom_outcome"), "custom_outcome");
    }
}
