//! `core/duplicate-instance` — at most one `aleph-server` may run at a time.
//!
//! Multiple daemon processes racing the same vault cause HMAC failure and
//! **vault data loss** (the AGENTS.md process-management redline). This check
//! counts live processes whose exe name contains `aleph-server`, excluding
//! itself, and warns when others exist. **Not repairable**: killing processes
//! is a human call (which one survives is not a mechanical decision), so the
//! finding quotes the documented procedure instead of applying it.

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/duplicate-instance";

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
        let others = match tokio::task::spawn_blocking(count_other_instances).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("duplicate-instance probe task failed: {e}");
                0
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
}
