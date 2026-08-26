//! `core/duplicate-instance` — at most one `aleph-server` may run at a time.
//!
//! Multiple daemon processes racing the same vault cause HMAC failure and
//! **vault data loss** (the AGENTS.md process-management redline). This check
//! counts live processes whose exe name contains `aleph-server`, excluding
//! itself, and warns when others exist. **Not repairable**: killing processes
//! is a human call (which one survives is not a mechanical decision), so the
//! finding quotes the documented procedure instead of applying it.

use async_trait::async_trait;

use crate::diagnostics::check::{settle_probe, HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/duplicate-instance";
/// Noun phrase the "unknown" finding is titled with — `"Duplicate instance
/// unknown"`. See [`crate::diagnostics::check::unknown_finding`].
const SUBJECT: &str = "Duplicate instance";

/// Classification, pure so it can be unit tested without a process table:
/// `other_instances` is the count of OTHER live `aleph-server` processes
/// (self already excluded). `None` means exactly one instance (ok finding).
fn classify_other_instances(other_instances: usize) -> Option<Severity> {
    if other_instances > 0 {
        Some(Severity::Warning)
    } else {
        None
    }
}

/// Count live `aleph-server` processes other than this one.
fn count_other_instances() -> usize {
    use sysinfo::{ProcessesToUpdate, System};

    let mut sys = System::new();
    // Processes only — no CPU/memory/disk refresh (kept minimal on purpose).
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let own = std::process::id();
    sys.processes()
        .values()
        .filter(|p| {
            p.pid().as_u32() != own
                // Zombies/the dead still hold a table entry but are not
                // "running" — and typically have no exe path anyway.
                && !matches!(
                    p.status(),
                    sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
                )
                && p.exe()
                    .and_then(|exe| exe.file_name())
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains("aleph-server"))
        })
        .count()
}

#[derive(Default)]
pub struct DuplicateInstanceCheck;

impl DuplicateInstanceCheck {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl HealthCheck for DuplicateInstanceCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Duplicate instance"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        // sysinfo does synchronous /proc I/O — keep it off the async executor.
        // A panicked or cancelled probe counted nothing. Folding it into `0`
        // fed `classify_other_instances(0)` — i.e. `[ok] Single instance`, the
        // reassuring line in front of the one condition this check exists for.
        // Sibling of the same fix in `core/instance-lock`; both go through
        // `check::settle_probe`.
        let others = match settle_probe(
            ID,
            SUBJECT,
            tokio::task::spawn_blocking(count_other_instances).await,
        ) {
            Ok(n) => n,
            Err(finding) => {
                return vec![finding.with_fix_hint(
                    "Count them by hand before trusting this host with a vault: \
                     pgrep -fa aleph-server",
                )]
            }
        };

        match classify_other_instances(others) {
            Some(severity) => vec![Finding::problem(
                ID,
                severity,
                "Multiple aleph-server processes running",
                format!(
                    "{others} other aleph-server process(es) detected. Multiple daemons racing \
                     the same vault cause HMAC failure and vault data loss."
                ),
            )
            .with_fix_hint(
                "Stop the duplicates before continuing (AGENTS.md procedure): \
                 pkill -f \"target/release/aleph-server\"; pkill -f \"target/debug/aleph-server\"; \
                 sleep 2 — then restart a single instance.",
            )],
            None => vec![Finding::ok(
                ID,
                "Single instance",
                "No other aleph-server processes detected.",
            )],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_counts() {
        assert_eq!(classify_other_instances(0), None);
        assert_eq!(classify_other_instances(1), Some(Severity::Warning));
        assert_eq!(classify_other_instances(3), Some(Severity::Warning));
    }

    #[tokio::test]
    async fn reports_one_non_repairable_finding() {
        let check = DuplicateInstanceCheck::new();
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].check_id, ID);
        // Killing processes is a human call — never mechanically repairable.
        assert!(!findings[0].repairable);
        assert!(findings[0].repair_outcome.is_none());
    }

    /// `classify_other_instances(0)` is `[ok] Single instance`, so a probe that
    /// never ran must not be allowed to produce `0`.
    ///
    /// Pins this check's own `(ID, SUBJECT)` wiring — see the sibling test in
    /// `stale_lock.rs` for why the shared `settle_probe` test is not enough.
    #[tokio::test]
    async fn a_process_scan_that_did_not_run_is_not_a_single_instance() {
        let joined: Result<usize, tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("process scan blew up")).await;
        let finding = settle_probe(ID, SUBJECT, joined)
            .expect_err("a task that did not complete must not settle into a count of 0");
        assert_eq!(finding.check_id, ID);
        assert_eq!(finding.title, "Duplicate instance unknown");
        assert!(finding.is_problem(), "an unknown must never render as [ok]");
    }
}
