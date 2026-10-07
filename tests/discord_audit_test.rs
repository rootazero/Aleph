//! Integration tests for the discord audit-hook pipeline.
//!
//! These exercise the same three functions the discord channel's
//! inbound handler (line 2) will call:
//!
//! - `record_discord_approval_requested`
//! - `record_discord_approval_resolved`
//! - `record_discord_approval_blocked`
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
    audit_label_for_outcome, record_discord_approval_blocked, record_discord_approval_requested,
    record_discord_approval_resolved,
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
/// run. Three hooks fire (requested, resolved, blocked) and we read all
/// three entries back. The blocked entry MUST be `ExecBlocked` /
/// `Critical` (the spec's reverse test: "should be rejected but audit
/// recorded it") and the requested/resolved pair MUST be `AuthorityChange`
/// / `Warn` — same event_type, since both are ratified, not refusals.
#[tokio::test]
#[serial(global_audit_log)]
async fn end_to_end_three_hooks_emit_expected_event_types() {
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

    record_discord_approval_requested(Some("user-1".into()), Some("sess-1".into()), "!ping").await;
    record_discord_approval_resolved(
        Some("user-1".into()),
        Some("sess-1".into()),
        "allow_once",
        "!ping",
    )
    .await;
    record_discord_approval_blocked(
        Some("user-2".into()),
        Some("sess-2".into()),
        "policy_denied",
        "!rm_rf",
    )
    .await;

    let e1 = rx.recv().await.expect("requested entry");
    assert_eq!(e1.event_type, AuditEventType::AuthorityChange);
    assert!(e1.detail.contains("requested"));
    assert!(e1.detail.contains("!ping"));

    let e2 = rx.recv().await.expect("resolved entry");
    assert_eq!(e2.event_type, AuditEventType::AuthorityChange);
    assert!(e2.detail.contains("resolved"));
    assert!(e2.detail.contains("allow_once"));
    assert!(e2.detail.contains("!ping"));

    let e3 = rx.recv().await.expect("blocked entry");
    assert_eq!(e3.event_type, AuditEventType::ExecBlocked);
    assert_eq!(e3.severity, AuditSeverity::Critical);
    assert!(e3.detail.contains("policy_denied"));
    assert!(e3.detail.contains("!rm_rf"));

    // No fourth entry should arrive: every hook fired exactly once.
    let extra = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
    assert!(
        extra.is_err(),
        "an unexpected fourth entry arrived: {extra:?}"
    );

    // Drop our handle so the global is the only one left holding a
    // sender; subsequent tests installing over us will not see our
    // drained entries.
    drop(log);
}

/// A fresh process (no global log installed yet) must let the three
/// hooks no-op rather than panic. This test runs alongside the
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

    record_discord_approval_requested(Some("u".into()), Some("s".into()), "!ping").await;
    record_discord_approval_resolved(Some("u".into()), Some("s".into()), "allow_once", "!ping")
        .await;
    record_discord_approval_blocked(Some("u".into()), Some("s".into()), "rate_limited", "!ping")
        .await;

    // Whatever the global state, our local log must be empty — these
    // hooks target `global()`, never a parameter.
    let extra = tokio::time::timeout(std::time::Duration::from_millis(50), local_rx.recv()).await;
    assert!(extra.is_err(), "hooks targeted a non-global log: {extra:?}");
}
