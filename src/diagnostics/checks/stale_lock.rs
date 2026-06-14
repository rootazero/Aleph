//! `core/instance-lock` — detect and clear a stale singleton lock file.
//!
//! The OS releases the `flock` on process exit, but the `aleph.lock` file
//! (carrying the holder PID) can linger. A lingering file whose PID is no
//! longer alive is harmless to `flock`-based acquisition, yet it produces
//! the scary `Stale lock file detected` diagnostic at startup. Clearing it
//! is a deterministic, safe repair — but ONLY when the holder is dead.
//!
//! Reuses [`crate::utils::instance_lock::diagnose_holder`] so the PID-read
//! and liveness logic is not duplicated.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, RepairOutcome, Severity};
use crate::utils::instance_lock::diagnose_holder;

const ID: &str = "core/instance-lock";
const LOCK_FILENAME: &str = "aleph.lock";

pub struct StaleLockCheck {
    data_dir: PathBuf,
}

impl StaleLockCheck {
    #[must_use]
    pub const fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

#[async_trait]
impl HealthCheck for StaleLockCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Instance lock"
    }

    async fn run(&self, posture: Posture) -> Vec<Finding> {
        let holder = match diagnose_holder(&self.data_dir) {
            // No lock file at all — nothing is holding the singleton.
            None => {
                return vec![Finding::ok(
                    ID,
                    "No lock held",
                    "No aleph.lock present; the singleton is free.",
                )];
            }
            Some(h) => h,
        };

        if holder.process_alive {
            return vec![Finding::ok(
                ID,
                "Server running",
                format!("aleph.lock held by live PID {}.", holder.pid),
            )];
        }

        // PID is dead but the file remains → stale.
        let lock_path = self.data_dir.join(LOCK_FILENAME);
        let display = lock_path.display().to_string();
        let mut finding = Finding::problem(
            ID,
            Severity::Warning,
            "Stale lock file",
            format!(
                "aleph.lock names PID {} which is not running; a crashed daemon left it behind.",
                holder.pid
            ),
        )
        .with_fix_hint(format!(
            "Run `aleph doctor --fix`, or remove it manually: rm {display}"
        ))
        .repairable();

        if posture.allows_repair() {
            let outcome = match std::fs::remove_file(&lock_path) {
                Ok(()) => RepairOutcome::Repaired {
                    detail: format!("Removed stale {display}"),
                },
                Err(e) => RepairOutcome::Failed {
                    error: e.to_string(),
                },
            };
            finding = finding.with_repair(outcome);
        }

        vec![finding]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn ok_when_no_lock_file() {
        let tmp = tempdir().unwrap();
        let check = StaleLockCheck::new(tmp.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert!(!findings[0].is_problem());
    }

    #[tokio::test]
    #[cfg(not(windows))] // is_process_alive is a non-unix stub (always true) → a dead PID can't be detected
    async fn detects_and_repairs_stale_lock() {
        let tmp = tempdir().unwrap();
        // PID 1 on a typical system is alive (init); use an absurd PID that
        // is virtually guaranteed to be dead to simulate a stale holder.
        let lock = tmp.path().join(LOCK_FILENAME);
        fs::write(&lock, "2147480000\n").unwrap();

        let check = StaleLockCheck::new(tmp.path().to_path_buf());
        let inspect = check.run(Posture::Inspect).await;
        assert_eq!(inspect[0].severity, Severity::Warning);
        assert!(inspect[0].repairable);
        assert!(lock.exists(), "inspect must not mutate");

        let fixed = check.run(Posture::Fix).await;
        assert!(matches!(
            fixed[0].repair_outcome,
            Some(RepairOutcome::Repaired { .. })
        ));
        assert!(!lock.exists(), "fix must remove the stale lock");
    }
}
