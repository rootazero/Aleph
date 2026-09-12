//! `core/instance-lock` — detect and clear a stale singleton lock file.
//!
//! The OS releases the `flock` on process exit, and a clean release also
//! removes the holder sidecar (`aleph.lock.pid`, see `InstanceLock::drop`;
//! the server's forced-exit failsafe does the same before `process::exit`).
//! A crash or SIGKILL skips both, leaving a sidecar that names a dead PID.
//! It is harmless to `flock`-based acquisition — the next `try_acquire`
//! simply wins the lock and overwrites it — so this check is the only place
//! the leftover is ever reported.
//! Clearing it is a deterministic, safe repair — but ONLY when the holder
//! is dead.
//!
//! Reuses [`crate::utils::instance_lock::diagnose_holder`] so the PID-read
//! and liveness logic is not duplicated.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::diagnostics::check::{settle_probe, unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, RepairOutcome, Severity};
use crate::utils::instance_lock::{
    diagnose_holder, is_lock_held, remove_holder_record_if_lock_free, StaleRecordRepair,
};

const ID: &str = "core/instance-lock";
/// Noun phrase the "unknown" finding is titled with — `"Instance lock
/// unknown"`. See [`crate::diagnostics::check::unknown_finding`].
const SUBJECT: &str = "Instance lock";
/// Only for the arm where the sidecar could not be read at all (so there is
/// no `HolderDiagnostic` to take the path from) and for the tests' fixtures.
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
        //
        // A probe that DID run but whose filesystem read errored (EACCES, AV
        // lock, ACL revoke) returns `Ok(Err(e))` — also an unknown answer,
        // also not a free singleton. Folding that into the `Ok(None)` arm
        // below is the bug: the operator would see `[ok] No lock held` for a
        // holder file the doctor could not read, the exact reassuring line in
        // front of the vault-data-loss condition.
        let probe = settle_probe(
            ID,
            SUBJECT,
            tokio::task::spawn_blocking(move || diagnose_holder(&data_dir)).await,
        );
        let holder = match probe {
            Err(finding) => return vec![finding],
            Ok(Err(e)) => {
                return vec![unknown_finding(
                    ID,
                    SUBJECT,
                    format!(
                        "the holder sidecar exists but could not be read: {e}. \
                         Treating the singleton state as unknown rather than free; \
                         check ownership and permissions on {} and try again.",
                        self.data_dir.join(HOLDER_FILENAME).display()
                    ),
                )];
            }
            Ok(Ok(None)) => {
                return vec![Finding::ok(
                    ID,
                    "No lock held",
                    "No aleph.lock present; the singleton is free.",
                )];
            }
            Ok(Ok(Some(h))) => h,
        };

        if holder.process_alive {
            return vec![Finding::ok(
                ID,
                "Server running",
                format!("aleph.lock held by live PID {}.", holder.pid),
            )];
        }

        // The record names a dead PID. That is NOT yet "stale lock": the
        // record and the lock are two different facts. A daemon whose PID was
        // never rewritten after forking is alive and holding the lock while
        // its record names its exited parent — and removing its files on the
        // strength of the record alone is how a second instance gets to run
        // beside it. Ask the lock itself before saying anything is removable.
        let data_dir = self.data_dir.clone();
        let held = settle_probe(
            ID,
            SUBJECT,
            tokio::task::spawn_blocking(move || is_lock_held(&data_dir)).await,
        );
        let held = match held {
            Err(finding) => return vec![finding],
            Ok(Err(e)) => {
                return vec![unknown_finding(
                    ID,
                    SUBJECT,
                    format!(
                        "the holder record names PID {}, which is not running, but the \
                         lock itself could not be probed: {e}. Treating the singleton as \
                         unknown rather than free; nothing was removed.",
                        holder.pid
                    ),
                )];
            }
            Ok(Ok(held)) => held,
        };
        let holder_display = holder.holder_path.display().to_string();
        if held {
            return vec![Finding::problem(
                ID,
                Severity::Warning,
                "Holder record stale",
                format!(
                    "{} is held by a running process, but the holder record names PID {}, \
                     which is not running — the record was not rewritten (a daemon that \
                     forked, typically). Nothing is safe to remove while the lock is held.",
                    holder.lock_path.display(),
                    holder.pid
                ),
            )
            .with_fix_hint(
                "Find the holder in your process list and stop it cleanly (`aleph stop`); \
                 a clean stop removes the record and a clean start writes a correct one. \
                 Do not remove the lock file while it is held.",
            )];
        }

        // Lock free + record naming a dead PID: a crash or SIGKILL left the
        // record behind. Removing it is safe, and the repair re-checks the
        // lock while holding it, so a starter racing this run cannot lose its
        // record.
        let mut finding = Finding::problem(
            ID,
            Severity::Warning,
            "Stale holder record",
            format!(
                "the holder record names PID {}, which is not running, and the lock is \
                 free; a crashed daemon left it behind.",
                holder.pid
            ),
        )
        .with_fix_hint(format!(
            "Run `aleph doctor --fix`, or remove manually: rm \"{holder_display}\""
        ))
        .repairable();

        if posture.allows_repair() {
            let data_dir = self.data_dir.clone();
            let outcome = match tokio::task::spawn_blocking(move || {
                remove_holder_record_if_lock_free(&data_dir)
            })
            .await
            {
                Ok(Ok(StaleRecordRepair::Removed)) => RepairOutcome::Repaired {
                    detail: format!("Removed stale holder record ({holder_display})"),
                },
                Ok(Ok(StaleRecordRepair::LockHeld)) => RepairOutcome::Failed {
                    error: "the lock became held between the probe and the repair; \
                            nothing was removed"
                        .to_string(),
                },
                Ok(Err(e)) => RepairOutcome::Failed {
                    error: e.to_string(),
                },
                Err(e) => RepairOutcome::Failed {
                    error: format!("the repair task did not complete: {e}"),
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

    /// A record naming a dead PID next to a HELD lock is a live holder whose
    /// record was never rewritten — `--fix` must not touch it. Before this
    /// guard the check deleted both the record and `aleph.lock` on the
    /// strength of the record alone, which on Unix hands the next starter a
    /// fresh inode and a second running instance.
    #[tokio::test]
    async fn a_held_lock_with_a_stale_record_is_reported_not_removed() {
        use crate::utils::instance_lock::{try_acquire, AcquireOutcome};
        let tmp = tempdir().unwrap();
        let _hold = match try_acquire(tmp.path()).unwrap() {
            AcquireOutcome::Acquired(g) => g,
            other => panic!("first acquire should succeed, got {other:?}"),
        };
        // Overwrite the live record with one that reads as dead: our own PID
        // with an impossible start time, exactly the recycled-PID shape.
        let holder = tmp.path().join(HOLDER_FILENAME);
        fs::write(&holder, format!("{}\n1\n", std::process::id())).unwrap();
        let lock_file = tmp.path().join("aleph.lock");

        let check = StaleLockCheck::new(tmp.path().to_path_buf());
        let fixed = check.run(Posture::Fix).await;

        assert_eq!(fixed[0].severity, Severity::Warning, "{:?}", fixed[0]);
        assert!(
            fixed[0].is_problem(),
            "a stale record is still a problem to report"
        );
        assert!(
            !fixed[0].repairable,
            "nothing is repairable while the lock is held: {:?}",
            fixed[0]
        );
        assert!(
            fixed[0].repair_outcome.is_none(),
            "{:?}",
            fixed[0].repair_outcome
        );
        assert!(holder.exists(), "the held lock's record must survive --fix");
        assert!(lock_file.exists(), "the held lock file must survive --fix");
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
            .expect_err("a task that did not complete must not settle into `no holder`");
        assert_eq!(finding.check_id, ID);
        assert_eq!(finding.title, "Instance lock unknown");
        assert!(finding.is_problem(), "an unknown must never render as [ok]");
    }
}
