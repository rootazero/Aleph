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

use crate::diagnostics::check::{settle_probe, HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, RepairOutcome, Severity};
use crate::utils::instance_lock::diagnose_holder;

const ID: &str = "core/instance-lock";
/// Noun phrase the "unknown" finding is titled with — `"Instance lock
/// unknown"`. See [`crate::diagnostics::check::unknown_finding`].
const SUBJECT: &str = "Instance lock";
const LOCK_FILENAME: &str = "aleph.lock";
const HOLDER_FILENAME: &str = "aleph.lock.pid";

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
        // `diagnose_holder` does a synchronous sysinfo process scan — keep it
        // off the async executor (same discipline as `core/duplicate-instance`).
        let data_dir = self.data_dir.clone();
        // A panicked or cancelled probe knows nothing about the lock. Folding
        // it into `None` reached the `None` arm below, which says "No lock
        // held … the singleton is free" at `Info` — byte-identical to a real
        // pass. `check::settle_probe` is the one place that decides what a
        // probe that did not run means.
        let holder = match settle_probe(
            ID,
            SUBJECT,
            tokio::task::spawn_blocking(move || diagnose_holder(&data_dir)).await,
        ) {
            Ok(h) => h,
            Err(finding) => return vec![finding],
        };
        let holder = match holder {
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

        // PID is dead but the holder record remains → stale. The PID now lives
        // in the unlocked `aleph.lock.pid` sidecar; remove it (and tidy the
        // now-empty lock target) so the next diagnosis reports the lock free.
        let lock_path = self.data_dir.join(LOCK_FILENAME);
        let holder_path = self.data_dir.join(HOLDER_FILENAME);
        let display = lock_path.display().to_string();
        let holder_display = holder_path.display().to_string();
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
            "Run `aleph doctor --fix`, or remove manually: rm \"{display}\" \"{holder_display}\""
        ))
        .repairable();

        if posture.allows_repair() {
            let outcome = match tokio::fs::remove_file(&holder_path).await {
                Ok(()) => {
                    // Best-effort tidy of the empty lock target; the sidecar was
                    // the file carrying the stale PID.
                    let _ = tokio::fs::remove_file(&lock_path).await;
                    RepairOutcome::Repaired {
                        detail: format!("Removed stale lock ({display})"),
                    }
                }
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
    // Liveness is now cross-platform (`utils::process_alive`, sysinfo-backed),
    // so a dead PID is detectable on Windows too — no platform gate needed.
    async fn detects_and_repairs_stale_lock() {
        let tmp = tempdir().unwrap();
        // PID 1 on a typical system is alive (init); use an absurd PID that
        // is virtually guaranteed to be dead to simulate a stale holder. The
        // PID lives in the unlocked `aleph.lock.pid` sidecar.
        let holder = tmp.path().join(HOLDER_FILENAME);
        fs::write(&holder, "2147480000\n").unwrap();

        let check = StaleLockCheck::new(tmp.path().to_path_buf());
        let inspect = check.run(Posture::Inspect).await;
        assert_eq!(inspect[0].severity, Severity::Warning);
        assert!(inspect[0].repairable);
        assert!(holder.exists(), "inspect must not mutate");

        let fixed = check.run(Posture::Fix).await;
        assert!(matches!(
            fixed[0].repair_outcome,
            Some(RepairOutcome::Repaired { .. })
        ));
        assert!(!holder.exists(), "fix must remove the stale holder record");
    }

    /// The `[ok] No lock held` line must be reachable only from a probe that
    /// actually looked.
    ///
    /// Pins this check's own `(ID, SUBJECT)` wiring, which the shared test on
    /// `check::settle_probe` cannot: a check that passed the wrong subject
    /// would still produce a `Warning` there and would title it about the
    /// wrong thing here.
    #[tokio::test]
    async fn a_holder_probe_that_did_not_run_is_not_a_free_singleton() {
        let joined: Result<Option<()>, tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("holder probe blew up")).await;
        let finding = settle_probe(ID, SUBJECT, joined)
            .err()
            .expect("a task that did not complete must not settle into `no holder`");
        assert_eq!(finding.check_id, ID);
        assert_eq!(finding.title, "Instance lock unknown");
        assert!(finding.is_problem(), "an unknown must never render as [ok]");
    }
}
