//! Unified self-diagnosis engine ("doctor").
//!
//! Aggregates per-domain [`HealthCheck`]s into one surface with three
//! postures — Inspect (human), Lint (JSON for CI), Fix (apply mechanical
//! repairs) — mirroring `openclaw doctor` but mapped onto Rust traits and
//! run **concurrently** via Tokio (`join_all`), unlike the reference's
//! sequential execution.
//!
//! Relationship to self-management: this engine is the read/repair *sensor*
//! (deterministic, mechanical — an R7 enabling layer, never reasoning). The
//! `self_manage` / `self_config` tools are the LLM-driven *write* path. They
//! are siblings: the LLM may call `doctor` to detect, then route fixes it
//! cannot mechanize back through self-management.
//!
//! Consumers: the `aleph doctor` CLI command and the `doctor` builtin tool.

pub mod check;
pub mod checks;
pub mod finding;

pub use check::{HealthCheck, Posture};
pub use finding::{Finding, RepairOutcome, Severity};

use std::sync::Arc;

use futures::future::join_all;
use serde::Serialize;

use crate::error::Result;

/// Registry of health checks. Cheap to clone (checks are `Arc`-shared).
#[derive(Clone)]
pub struct DiagnosticEngine {
    checks: Vec<Arc<dyn HealthCheck>>,
}

impl DiagnosticEngine {
    /// Construct from an explicit check list (used by tests).
    #[must_use]
    pub fn new(checks: Vec<Arc<dyn HealthCheck>>) -> Self {
        Self { checks }
    }

    /// Build the production registry against the real `~/.aleph` paths.
    pub fn default_registry() -> Result<Self> {
        use crate::utils::paths::get_data_dir;

        let data_dir = get_data_dir()?;
        // The file this process actually runs on, not where config would live
        // by default — a doctor that parses the wrong file reports health about
        // a config nothing is using.
        let config_path = crate::config::Config::effective_path();

        let checks: Vec<Arc<dyn HealthCheck>> = vec![
            Arc::new(checks::DataDirCheck::new(data_dir.clone())),
            Arc::new(checks::LoopGraphCheck::new(data_dir.clone())),
            Arc::new(checks::StaleLockCheck::new(data_dir)),
            Arc::new(checks::ConfigParseCheck::new(config_path)),
            Arc::new(checks::VaultCheck::from_default_path()),
            Arc::new(checks::HooksConsentCheck::from_default_path()),
            Arc::new(checks::BrowserRuntimeCheck::new()),
        ];
        Ok(Self::new(checks))
    }

    /// Append runtime checks that need live daemon handles (config + vault).
    ///
    /// `default_registry()` stays path-only so the offline `aleph-server
    /// doctor` command works without network access; this opt-in upgrade is
    /// used by the `doctor` builtin tool inside the daemon, giving the LLM
    /// repair loop the same provider-connectivity visibility as the CLI's
    /// `providers.healthcheck` rows — so it can verify its own fixes.
    pub fn with_runtime_checks(
        mut self,
        config: Arc<tokio::sync::RwLock<crate::config::Config>>,
        vault: Arc<crate::gateway::security::SharedTokenManager>,
    ) -> Self {
        self.checks
            .push(Arc::new(checks::ProvidersConnectivityCheck::new(
                config, vault,
            )));
        self
    }

    /// Run every check concurrently and collect a report.
    pub async fn run(&self, posture: Posture) -> DiagnosticReport {
        let futures = self.checks.iter().map(|c| c.run(posture));
        let per_check = join_all(futures).await;
        let findings: Vec<Finding> = per_check.into_iter().flatten().collect();
        DiagnosticReport {
            posture: posture_label(posture),
            checks_run: self.checks.len(),
            findings,
        }
    }
}

const fn posture_label(p: Posture) -> &'static str {
    match p {
        Posture::Inspect => "inspect",
        Posture::Lint => "lint",
        Posture::Fix => "fix",
    }
}

/// Outcome of a full diagnostic run.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticReport {
    pub posture: &'static str,
    pub checks_run: usize,
    pub findings: Vec<Finding>,
}

impl DiagnosticReport {
    #[must_use]
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    #[must_use]
    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    /// Count repairs that actually succeeded this run.
    #[must_use]
    pub fn repaired(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| matches!(f.repair_outcome, Some(RepairOutcome::Repaired { .. })))
            .count()
    }

    /// True when no error- or warning-level findings remain unrepaired.
    #[must_use]
    pub fn ok(&self) -> bool {
        !self.findings.iter().any(|f| {
            f.is_problem() && !matches!(f.repair_outcome, Some(RepairOutcome::Repaired { .. }))
        })
    }

    /// Serialize to a stable JSON envelope for `--json` / CI.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "ok": self.ok(),
            "posture": self.posture,
            "checksRun": self.checks_run,
            "errors": self.errors(),
            "warnings": self.warnings(),
            "repaired": self.repaired(),
            "findings": self.findings,
        })
        .to_string()
    }

    /// Render a compact human report. Used by the CLI and the tool's text view.
    #[must_use]
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "aleph doctor ({}): {} check(s), {} finding(s)\n",
            self.posture,
            self.checks_run,
            self.findings.len()
        ));
        for f in &self.findings {
            let tag = match f.severity {
                Severity::Info => "ok",
                Severity::Warning => "warn",
                Severity::Error => "ERROR",
            };
            out.push_str(&format!("  [{tag}] {} {}\n", f.check_id, f.title));
            if f.is_problem() {
                out.push_str(&format!("      {}\n", f.detail));
            }
            if let Some(outcome) = &f.repair_outcome {
                let line = match outcome {
                    RepairOutcome::Repaired { detail } => format!("repaired: {detail}"),
                    RepairOutcome::Failed { error } => format!("repair failed: {error}"),
                    RepairOutcome::Skipped { reason } => format!("repair skipped: {reason}"),
                };
                out.push_str(&format!("      → {line}\n"));
            } else if let Some(hint) = &f.fix_hint {
                out.push_str(&format!("      fix: {hint}\n"));
            }
        }
        let summary = if self.ok() {
            "no unresolved problems".to_string()
        } else {
            format!("{} error(s), {} warning(s)", self.errors(), self.warnings())
        };
        out.push_str(&format!(
            "summary: {summary}{}\n",
            if self.repaired() > 0 {
                format!(" ({} repaired)", self.repaired())
            } else {
                String::new()
            }
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct FakeCheck {
        id: &'static str,
        finding: Finding,
    }

    #[async_trait]
    impl HealthCheck for FakeCheck {
        fn id(&self) -> &'static str {
            self.id
        }
        fn title(&self) -> &'static str {
            "fake"
        }
        async fn run(&self, _posture: Posture) -> Vec<Finding> {
            vec![self.finding.clone()]
        }
    }

    #[tokio::test]
    async fn report_aggregates_and_flags_problems() {
        let engine = DiagnosticEngine::new(vec![
            Arc::new(FakeCheck {
                id: "a",
                finding: Finding::ok("a", "fine", "ok"),
            }),
            Arc::new(FakeCheck {
                id: "b",
                finding: Finding::problem("b", Severity::Error, "boom", "broke"),
            }),
        ]);
        let report = engine.run(Posture::Inspect).await;
        assert_eq!(report.checks_run, 2);
        assert_eq!(report.findings.len(), 2);
        assert_eq!(report.errors(), 1);
        assert!(!report.ok());
        assert!(report.to_json().contains("\"ok\":false"));
    }

    #[tokio::test]
    async fn report_ok_when_repaired() {
        let engine = DiagnosticEngine::new(vec![Arc::new(FakeCheck {
            id: "a",
            finding: Finding::problem("a", Severity::Warning, "stale", "left behind")
                .repairable()
                .with_repair(RepairOutcome::Repaired {
                    detail: "cleaned".into(),
                }),
        })]);
        let report = engine.run(Posture::Fix).await;
        assert!(report.ok(), "a repaired problem should not block ok()");
        assert_eq!(report.repaired(), 1);
    }
}
