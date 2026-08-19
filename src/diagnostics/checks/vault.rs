//! `core/vault` — the secret vault file must be loadable if present.
//!
//! Aleph stores provider API keys and other secrets in an encrypted
//! `~/.aleph/secrets.vault`. When that file is present but corrupt (a partial
//! write, a downgrade to an older build, or hand-editing), every credential
//! lookup fails — and today the only signal is a `tracing::error!` emitted deep
//! inside daemon boot when [`SecretVault::open_or_backup`] moves the file aside.
//! This check surfaces the same condition *proactively* and offline, the way
//! `core/data-dir` / `core/config-parse` already do for their domains. It maps
//! the "secret / credential readiness" doctor domain that the reference doctors
//! (opensquilla `evaluate_provider`, openclaw) cover, onto Aleph's own vault
//! infrastructure — reusing [`SecretVault::open`] so the format/version logic is
//! not duplicated.
//!
//! Read-only and **non-repairable**: a corrupt vault holds the user's only copy
//! of their secrets, so recovery is a deliberate human/daemon action
//! (`open_or_backup` preserves the original bytes), never a mechanical
//! doctor repair that could clobber them. The finding points at that recovery
//! path via a `fix_hint`.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};
use crate::secrets::SecretVault;

const ID: &str = "core/vault";

pub struct VaultCheck {
    vault_path: PathBuf,
}

impl VaultCheck {
    #[must_use]
    pub const fn new(vault_path: PathBuf) -> Self {
        Self { vault_path }
    }

    /// Build against the default `~/.aleph/secrets.vault`.
    #[must_use]
    pub fn from_default_path() -> Self {
        Self::new(SecretVault::default_path())
    }
}

#[async_trait]
impl HealthCheck for VaultCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Secret vault"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        let display = self.vault_path.display().to_string();

        // A missing vault is normal on a fresh install — no secrets stored yet.
        if !self.vault_path.exists() {
            return vec![Finding::ok(
                ID,
                "No vault yet",
                format!("{display} not present; no secrets stored yet."),
            )];
        }

        // `open()` reads and decrypts the file synchronously — keep it off
        // the async executor. It errors ONLY when a present file cannot be
        // loaded (corruption, an unsupported future version, or an I/O
        // error) — exactly the problem worth surfacing. It never mutates the
        // file.
        let path = self.vault_path.clone();
        match tokio::task::spawn_blocking(move || SecretVault::open(&path)).await {
            Ok(Ok(vault)) => vec![Finding::ok(
                ID,
                "Vault OK",
                format!("{display} loads; {} secret(s) stored.", vault.len()),
            )],
            Ok(Err(e)) => vec![Finding::problem(
                ID,
                Severity::Error,
                "Vault is not loadable",
                format!("{display} exists but could not be loaded: {e}"),
            )
            .with_fix_hint(
                "Do NOT delete the file — it holds your only copy of every secret. \
                 The daemon moves a corrupt vault aside to `<path>.corrupt-<timestamp>` \
                 on next start; restore from a backup, or re-enter secrets with \
                 `aleph secret set <name>` once the daemon has rebuilt an empty vault.",
            )],
            Err(e) => vec![Finding::problem(
                ID,
                Severity::Warning,
                "Vault probe failed",
                format!("the vault load task failed to run: {e}"),
            )],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn ok_when_vault_absent() {
        let tmp = tempdir().unwrap();
        let check = VaultCheck::new(tmp.path().join("secrets.vault"));
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].is_problem());
    }

    #[tokio::test]
    async fn detects_corrupt_vault_as_unrepairable_error() {
        // A present-but-undeserializable file is exactly the condition worth
        // surfacing. It must be a hard Error, carry a recovery hint, and never
        // be auto-repaired — the file holds the user's only copy of every secret.
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("secrets.vault");
        std::fs::write(&path, b"not a valid vault").unwrap();

        let check = VaultCheck::new(path);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].fix_hint.is_some());
        assert!(
            !findings[0].repairable,
            "a corrupt vault must never auto-repair"
        );
    }

    #[tokio::test]
    async fn never_repairs_in_fix_posture() {
        // Recovery is a deliberate action, never a mechanical doctor repair —
        // even in Fix posture the check must leave the file untouched.
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("secrets.vault");
        std::fs::write(&path, b"corrupt").unwrap();

        let check = VaultCheck::new(path.clone());
        let findings = check.run(Posture::Fix).await;
        assert!(findings.iter().all(|f| f.repair_outcome.is_none()));
        assert!(path.exists(), "fix posture must not delete the vault file");
    }
}
