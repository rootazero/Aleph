//! `core/hooks-consent` — audit the shell-hook consent registry.
//!
//! Hoists the diagnosis that previously lived inline in
//! `aleph-server hooks doctor` into a reusable check, so the unified `aleph doctor`
//! and the `doctor` tool report consent health too. The `aleph-server hooks doctor`
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
    #[must_use]
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

        // Approvals given under the shared `user:project` label before
        // approvals were bound to a project. No project hook is looked up
        // under one, so each authorises nothing — and the hook it was given
        // for does not run, a blocking guard included (it stops blocking),
        // until its project's own pending entry is approved. The registry
        // never deletes them, so they are counted here rather than as
        // approvals; revoking one acknowledges it and clears this finding.
        let superseded = entries
            .iter()
            .filter(|e| e.status == ConsentStatus::Approved && e.predates_project_binding())
            .count();
        if superseded > 0 {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Warning,
                    "Project-hook approvals no longer apply",
                    format!(
                        "{superseded} project-hook approval(s) predate project binding and \
                         authorise nothing — those hooks, blocking ones included, are not \
                         running; approve each project's pending entry."
                    ),
                )
                .with_fix_hint(
                    "`aleph-server hooks list` marks the old entries. Review each project's pending \
                     entry with `aleph-server hooks test <fingerprint>`, then \
                     `aleph-server hooks revoke <old fingerprint>` to clear this warning.",
                ),
            );
        }

        // Approvals of a plugin's hook or inline command recorded while the
        // install path was still spliced into its text (before 2026-09-27).
        // The text as written is the key now, so each authorises nothing —
        // and that hook or command, a blocking guard included, does not run
        // until the new pending entry (the text as the plugin wrote it) is
        // approved.
        let spliced = entries
            .iter()
            .filter(|e| e.status == ConsentStatus::Approved && e.predates_literal_plugin_text())
            .count();
        if spliced > 0 {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Warning,
                    "Plugin-hook approvals of the expanded path no longer apply",
                    format!(
                        "{spliced} plugin hook / inline-command approval(s) were given to the \
                         text with the install path expanded into it and authorise nothing — \
                         those commands, blocking hooks included, are not running; approve \
                         the new pending entry, recorded as the plugin wrote it."
                    ),
                )
                .with_fix_hint(
                    "`aleph-server hooks list` marks the old entries. Review each new pending \
                     entry with `aleph-server hooks test <fingerprint>`, then \
                     `aleph-server hooks revoke <old fingerprint>` to clear this warning.",
                ),
            );
        }

        // A superseded entry never fires again, so one left pending is not
        // awaiting anything.
        let pending = entries
            .iter()
            .filter(|e| e.status == ConsentStatus::Pending && !e.is_superseded())
            .count();
        if pending > 0 {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Warning,
                    "Hooks await approval",
                    format!("{pending} shell hook(s) are pending approval and will be skipped until reviewed."),
                )
                .with_fix_hint("Review each with `aleph-server hooks test <fingerprint>`, then approve if trusted."),
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

        // A stored fingerprint that no longer matches the hash of the entry's
        // own key (plugin, project, command) means the registry was
        // hand-edited or corrupted — consent for those rows can no longer be
        // trusted.
        let drifted = entries
            .iter()
            .filter(|e| e.expected_fingerprint() != e.fingerprint)
            .count();
        if drifted > 0 {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Error,
                    "Stale fingerprint",
                    format!("{drifted} entr(ies) have a fingerprint that no longer matches their command (registry hand-edited?)."),
                )
                .with_fix_hint("Revoke and re-approve the affected hooks: `aleph-server hooks revoke <fingerprint>`."),
            );
        }

        if findings.is_empty() {
            // Old `user:project*` entries left pending are not hooks anyone
            // can approve into running; they are not "all approved".
            let (kept, live): (Vec<_>, Vec<_>) = entries.iter().partition(|e| e.is_superseded());
            let detail = match kept.len() {
                0 => format!(
                    "{} hook(s) recorded, all approved and consistent.",
                    live.len()
                ),
                n => format!(
                    "{} hook(s) recorded, all approved and consistent; {n} older \
                     entr(ies) are kept on disk and authorise nothing.",
                    live.len()
                ),
            };
            findings.push(Finding::ok(ID, "Hooks consent OK", detail));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::visibility::ScopeKey;

    /// A project entry's key includes its project, so the drift check must
    /// recompute it with the project — a check that hashed `(plugin,
    /// command)` alone would call every project approval hand-edited.
    #[test]
    fn a_project_bound_entry_is_not_reported_as_drifted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let consent = ShellHookConsent::with_path(dir.path().join("allowlist.json"));
        consent.record_pending(
            "user:project",
            &ScopeKey::project(dir.path()),
            "lint",
            "PreToolUse",
            dir.path(),
        );

        let findings = HooksConsentCheck::new(consent).diagnose();
        assert!(
            findings.iter().all(|f| f.title != "Stale fingerprint"),
            "{findings:?}"
        );
    }

    /// After the project binding, an approval given under the shared
    /// `user:project` label authorises nothing, and the hook it was for — a
    /// blocking guard included — is not running. The doctor says so, counts
    /// it as neither approved nor awaiting approval, and stops once it is
    /// revoked.
    #[test]
    fn a_project_approval_from_before_project_binding_is_reported_as_not_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("allowlist.json");
        let entry = |owner: &str, command: &str, status: &str| {
            serde_json::json!({
                "fingerprint": ShellHookConsent::fingerprint(owner, None, command),
                "plugin_name": owner, "command": command, "plugin_root": "/r",
                "status": status, "first_seen": 1,
            })
        };
        let registry = serde_json::json!({ "version": 1, "entries": [
            entry("user:project", "guard", "approved"),
            entry("user:project", "old", "pending"),
            entry("user:global", "live", "approved"),
        ] });
        std::fs::write(&path, registry.to_string()).expect("seed registry");
        let diagnose = || HooksConsentCheck::new(ShellHookConsent::with_path(&path)).diagnose();

        let findings = diagnose();
        let lapsed = findings
            .iter()
            .find(|f| f.title == "Project-hook approvals no longer apply")
            .unwrap_or_else(|| panic!("no finding for the lapsed approval: {findings:?}"));
        assert!(lapsed.is_problem());
        assert!(
            lapsed.detail.starts_with(
                "1 project-hook approval(s) predate project binding and authorise nothing"
            ),
            "{}",
            lapsed.detail
        );
        assert!(
            findings.iter().all(|f| f.title != "Hooks await approval"),
            "an old project entry is not awaiting approval: {findings:?}"
        );

        let guard = ShellHookConsent::fingerprint("user:project", None, "guard");
        ShellHookConsent::with_path(&path)
            .revoke(&guard)
            .expect("revoke")
            .expect("entry");
        assert!(
            diagnose().iter().all(|f| !f.is_problem()),
            "revoking the old approval clears the warning"
        );
    }

    /// P4.16 review M-4: a plugin approval given to the text with the install
    /// path spliced in (before 2026-09-27) authorises nothing. The doctor
    /// says so, does not count its pending sibling as awaiting approval, and
    /// stops once it is revoked; the text as written is a live entry.
    #[test]
    fn a_plugin_approval_of_the_spliced_text_is_reported_as_not_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("allowlist.json");
        let entry = |command: &str, status: &str| {
            serde_json::json!({
                "fingerprint": ShellHookConsent::fingerprint("fmt", None, command),
                "plugin_name": "fmt", "command": command, "plugin_root": "/inst/fmt",
                "status": status, "first_seen": 1,
            })
        };
        let registry = serde_json::json!({ "version": 1, "entries": [
            entry("sh /inst/fmt/guard.sh", "approved"),
            entry("sh /inst/fmt/old.sh", "pending"),
            entry("sh ${CLAUDE_PLUGIN_ROOT}/live.sh", "approved"),
        ] });
        std::fs::write(&path, registry.to_string()).expect("seed registry");
        let diagnose = || HooksConsentCheck::new(ShellHookConsent::with_path(&path)).diagnose();

        let findings = diagnose();
        let lapsed = findings
            .iter()
            .find(|f| f.title == "Plugin-hook approvals of the expanded path no longer apply")
            .unwrap_or_else(|| panic!("no finding for the spliced approval: {findings:?}"));
        assert!(
            lapsed.detail.starts_with("1 plugin hook"),
            "{}",
            lapsed.detail
        );
        assert!(
            findings.iter().all(|f| f.title != "Hooks await approval"),
            "a superseded plugin entry is not awaiting approval: {findings:?}"
        );

        let guard = ShellHookConsent::fingerprint("fmt", None, "sh /inst/fmt/guard.sh");
        ShellHookConsent::with_path(&path)
            .revoke(&guard)
            .expect("revoke")
            .expect("entry");
        assert!(
            diagnose().iter().all(|f| !f.is_problem()),
            "revoking the old approval clears the warning"
        );
    }
}
