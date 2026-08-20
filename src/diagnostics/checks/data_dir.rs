//! `core/data-dir` — the `~/.aleph/data` directory must exist and be writable.
//!
//! A missing data dir is the single most common first-run / corrupted-state
//! failure: the `SQLite` stores, vault, and instance lock all live under it.
//! This is the canonical *repairable* check — recreating the directory is
//! deterministic and safe.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, RepairOutcome, Severity};

const ID: &str = "core/data-dir";

pub struct DataDirCheck {
    data_dir: PathBuf,
}

impl DataDirCheck {
    #[must_use]
    pub const fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// Probe writability by creating and removing a throwaway file. Returns
    /// the io error if the directory is not writable.
    async fn write_probe(dir: &std::path::Path) -> std::io::Result<()> {
        let probe = dir.join(".aleph-doctor-write-probe");
        tokio::fs::write(&probe, b"ok").await?;
        let _ = tokio::fs::remove_file(&probe).await;
        Ok(())
    }
}

#[async_trait]
impl HealthCheck for DataDirCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Data directory"
    }

    async fn run(&self, posture: Posture) -> Vec<Finding> {
        let display = self.data_dir.display().to_string();

        if !self.data_dir.exists() {
            let mut finding = Finding::problem(
                ID,
                Severity::Error,
                "Data directory is missing",
                format!("{display} does not exist; SQLite stores, vault, and the instance lock cannot be created."),
            )
            .with_fix_hint(format!("Run `aleph doctor --fix`, or create it manually: mkdir -p \"{display}\""))
            .repairable();

            if posture.allows_repair() {
                let outcome = match tokio::fs::create_dir_all(&self.data_dir).await {
                    Ok(()) => RepairOutcome::Repaired {
                        detail: format!("Created {display}"),
                    },
                    Err(e) => RepairOutcome::Failed {
                        error: e.to_string(),
                    },
                };
                finding = finding.with_repair(outcome);
            }
            return vec![finding];
        }

        match Self::write_probe(&self.data_dir).await {
            Ok(()) => vec![Finding::ok(
                ID,
                "Data directory OK",
                format!("{display} exists and is writable."),
            )],
            Err(e) => vec![Finding::problem(
                ID,
                Severity::Error,
                "Data directory is not writable",
                format!("{display} exists but a write probe failed: {e}"),
            )
            .with_fix_hint(format!(
                "Check ownership/permissions: ls -ld \"{display}\" (the directory must be writable by the current user)."
            ))],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn reports_ok_for_existing_writable_dir() {
        let tmp = tempdir().unwrap();
        let check = DataDirCheck::new(tmp.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].is_problem());
    }

    #[tokio::test]
    async fn detects_missing_dir_and_does_not_repair_in_inspect() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("nope/data");
        let check = DataDirCheck::new(missing.clone());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].repairable);
        assert!(findings[0].repair_outcome.is_none());
        assert!(!missing.exists());
    }

    #[tokio::test]
    async fn repairs_missing_dir_in_fix_posture() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("nope/data");
        let check = DataDirCheck::new(missing.clone());
        let findings = check.run(Posture::Fix).await;
        assert!(matches!(
            findings[0].repair_outcome,
            Some(RepairOutcome::Repaired { .. })
        ));
        assert!(missing.exists());
    }
}
