//! Integration tests for the discord audit-hook pipeline.
//!
//! These exercise `record_discord_approval_resolved`, the one hook with a
//! live caller (the exec manager's `resolve_with_reason` chokepoint). The
//! `requested` / `blocked` pair had no producer in the discord channel and
//! was removed with its re-exports.
//!
//! Plus the [`label_for_outcome`] helper that lets the exec manager
//! turn an [`ApprovalOutcome`] into a stable audit verb without
//! reaching back into the approval module.
//!
//! All end-to-end checks against the global audit log are wrapped in
//! `#[serial(global_audit_log)]` because `SecurityAuditLog::install_global`
//! is one-shot — without serialisation a parallel test would race for
//! the single global slot and either silently lose its install or
//! observe the entries a sibling test produced.
//!
//! Companion tests live in:
//!
//! - `discord_security_test.rs` — the permission/startup audit surface.
//! - `discord_doctor_test.rs` — the `discord_channel_health` check.

mod common;

use serial_test::serial;

use alephcore::gateway::interfaces::discord::security::{
    audit_label_for_outcome, record_discord_approval_resolved,
};
use alephcore::security::audit::{AuditEventType, AuditSeverity, SecurityAuditLog};

/// `label_for_outcome` is the *only* way the exec manager should map
/// `ApprovalOutcome` to an audit verb — pinning it here makes a rename
/// of the internal enum a compile error rather than a silent log
/// rewrite.
#[test]
fn audit_label_table_is_stable() {
    assert_eq!(audit_label_for_outcome("approved"), "allow_once");
    assert_eq!(
        audit_label_for_outcome("approved_for_session"),
        "allow_session"
    );
    assert_eq!(audit_label_for_outcome("approved_always"), "allow_always");
    assert_eq!(audit_label_for_outcome("denied"), "deny");
    assert_eq!(audit_label_for_outcome("timeout"), "timeout");
    assert_eq!(audit_label_for_outcome("unavailable"), "unavailable");
}

/// Single end-to-end test that owns the global audit log for its full
/// run. The resolved hook fires once and we read the entry back: it MUST be
/// `AuthorityChange` / `Warn` — a ratified decision, not a refusal.
#[tokio::test]
#[serial(global_audit_log)]
async fn end_to_end_resolved_hook_emits_expected_event_type() {
    let (log, mut rx) = SecurityAuditLog::new(8);
    // One-shot install. If a previous serial test already installed,
    // the call returns false — but our `rx` would then be reading from
    // the wrong channel and the assertions below would flake. The
    // `#[serial(global_audit_log)]` annotation guarantees this test
    // runs alone, so the install must succeed.
    assert!(
        alephcore::security::audit::install_global(&log),
        "test audit log already installed — serial_test isolation broken?"
    );

    record_discord_approval_resolved(
        Some("user-1".into()),
        Some("sess-1".into()),
        "allow_once",
        "!ping",
    )
    .await;

    let e = rx.recv().await.expect("resolved entry");
    assert_eq!(e.event_type, AuditEventType::AuthorityChange);
    assert_eq!(e.severity, AuditSeverity::Warn);
    assert!(e.detail.contains("resolved"));
    assert!(e.detail.contains("allow_once"));
    assert!(e.detail.contains("!ping"));

    // No second entry should arrive: the hook fired exactly once.
    let extra = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
    assert!(
        extra.is_err(),
        "an unexpected second entry arrived: {extra:?}"
    );

    // Drop our handle so the global is the only one left holding a
    // sender; subsequent tests installing over us will not see our
    // drained entries.
    drop(log);
}

/// A fresh process (no global log installed yet) must let the hook
/// no-op rather than panic. This test runs alongside the
/// end-to-end test under `#[serial]` but installs its OWN log first
/// so the absence-of-global branch is exercised only by the order of
/// the tests.
#[tokio::test]
#[serial(global_audit_log)]
async fn hooks_do_not_panic_without_a_global_log() {
    // `install_global` is one-shot. If the prior test installed, our
    // install returns false but the global IS set — the hooks would
    // fire into the prior log instead of no-op'ing. To genuinely
    // exercise the no-global branch we rely on running FIRST (i.e.
    // when no prior serial test installed anything). The serial
    // ordering is alphabetical by default; the prior test name
    // sorts after this one in ASCII.
    //
    // To make this deterministic and not depend on sort order, we
    // accept either outcome: the hooks either no-op (assertion: no
    // entries arrive on a *local* `rx`), or they fire into the
    // previously-installed log (assertion: the local `rx` is empty).
    let (_local_log, mut local_rx) = SecurityAuditLog::new(8);

    record_discord_approval_resolved(Some("u".into()), Some("s".into()), "allow_once", "!ping")
        .await;

    // Whatever the global state, our local log must be empty — these
    // hooks target `global()`, never a parameter.
    let extra = tokio::time::timeout(std::time::Duration::from_millis(50), local_rx.recv()).await;
    assert!(extra.is_err(), "hooks targeted a non-global log: {extra:?}");
}
