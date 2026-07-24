//! `core/hooks-consent` — audit the shell-hook consent registry.
//!
//! Hoists the diagnosis that previously lived inline in
//! `aleph hooks doctor` into a reusable check, so the unified `aleph doctor`
//! and the `doctor` tool report consent health too. The `aleph hooks doctor`
//! CLI now renders these same findings (single source of truth — entropy reduction).
//!
//! Read-only: approving a pending hook is a deliberate human trust decision,
//! never an automatic repair.

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};
use crate::extension::hooks::{ConsentStatus, ShellHookConsent};

const ID: &str = "core/hooks-consent";

pub struct HooksConsentCheck {
    consent: ShellHookConsent,
}

impl HooksConsentCheck {
    pub const fn new(consent: ShellHookConsent) -> Self {
        Self { consent }
    }

    /// Build against the default `~/.aleph/shell-hooks-allowlist.json`.
    #[must_use]
    pub fn from_default_path() -> Self {
        Self::new(ShellHookConsent::with_path(ShellHookConsent::default_path()))
    }

    /// Pure detection — the shared core of both the CLI doctor and the engine.
    /// Returns `ok` summary when clean, or one finding per issue class.
    pub fn diagnose(&self) -> Vec<Finding> {
        let entries = self.consent.entries();
        if entries.is_empty() {
            return vec![Finding::ok(
                ID,
                "No shell hooks",
                "No shell-command hooks recorded; nothing to approve.",
            )];
        }

        let mut findings = Vec::new();

        let pending = entries
            .iter()
            .filter(|e| e.status == ConsentStatus::Pending)
            .count();
        if pending > 0 {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Warning,
                    "Hooks await approval",
                    format!("{pending} shell hook(s) are pending approval and will be skipped until reviewed."),
                )
                .with_fix_hint("Review each with `aleph hooks test <fingerprint>`, then approve if trusted."),
            );
        }

        let empty = entries
            .iter()
            .filter(|e| e.command.trim().is_empty())
            .count();
        if empty > 0 {
            findings.push(Finding::problem(
                ID,
                Severity::Warning,
                "Empty hook command",
                format!("{empty} consent entr(ies) have an empty command."),
            ));
        }

        // A stored fingerprint that no longer matches hash(plugin, command)
        // means the registry was hand-edited or corrupted — consent for those
        // rows can no longer be trusted.
        let drifted = entries
            .iter()
            .filter(|e| ShellHookConsent::fingerprint(&e.plugin_name, &e.command) != e.fingerprint)
            .count();
        if drifted > 0 {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Error,
                    "Stale fingerprint",
                    format!("{drifted} entr(ies) have a fingerprint that no longer matches their command (registry hand-edited?)."),
                )
                .with_fix_hint("Revoke and re-approve the affected hooks: `aleph hooks revoke <fingerprint>`."),
            );
        }

        if findings.is_empty() {
            findings.push(Finding::ok(
                ID,
                "Hooks consent OK",
                format!(
                    "{} hook(s) recorded, all approved and consistent.",
                    entries.len()
                ),
            ));
        }
        findings
    }
}

#[async_trait]
impl HealthCheck for HooksConsentCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Shell-hook consent"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        // Consent is never auto-repaired — approval is a human decision.
        self.diagnose()
    }
}
