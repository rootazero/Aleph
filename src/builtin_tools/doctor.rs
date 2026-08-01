//! `DoctorTool` — LLM-facing self-diagnosis (and optional self-repair).
//!
//! Lets the model answer "is my runtime healthy?" and, when asked, apply the
//! same deterministic mechanical repairs as `aleph doctor --fix` (recreate a
//! missing data dir, clear a stale instance lock). This is the R8 "everything
//! is a tool" face of the diagnostics engine: the LLM detects via this tool,
//! then routes any non-mechanical fix through `self_manage` / `self_config`.
//!
//! Thin wrapper — all detection/repair logic lives in
//! [`crate::diagnostics::DiagnosticEngine`]; this only adapts it to the tool
//! contract.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{notify_tool_result, notify_tool_start};
use crate::config::Config;
use crate::diagnostics::{DiagnosticEngine, DiagnosticReport, Posture};
use crate::error::Result;
use crate::gateway::security::SharedTokenManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DoctorArgs {
    /// When true, apply mechanical repairs for repairable findings
    /// (recreate the data directory, clear a stale instance lock). Default
    /// false = read-only inspection.
    #[serde(default)]
    #[schemars(description = "Apply safe, deterministic repairs (default false = read-only).")]
    pub fix: bool,
}

#[derive(Debug, Serialize)]
pub struct DoctorOutput {
    /// True when no unresolved problems remain (errors/warnings, minus repairs).
    pub ok: bool,
    /// One-line tally — the structured `report` is the payload; duplicating
    /// the full human render here wastes tool-output tokens.
    pub summary: String,
    /// Full structured report (posture, counts, per-check findings).
    pub report: DiagnosticReport,
}

#[derive(Clone, Default)]
pub struct DoctorTool {
    /// Live daemon config handle. When present (together with the vault),
    /// the engine gains the `providers/connectivity` runtime check so the
    /// LLM can probe provider reachability — and verify its own repairs
    /// after fixing a credential via `vault_store` / `self_config`.
    config: Option<Arc<RwLock<Config>>>,
    /// Shared vault handle for resolving provider API keys during probes.
    token_manager: Option<Arc<SharedTokenManager>>,
}

impl DoctorTool {
    /// Attach the live daemon handles that unlock runtime checks. Without
    /// them the tool behaves exactly as before (path-based checks only).
    pub fn with_runtime(
        mut self,
        config: Arc<RwLock<Config>>,
        token_manager: Arc<SharedTokenManager>,
    ) -> Self {
        self.config = Some(config);
        self.token_manager = Some(token_manager);
        self
    }
}

#[async_trait]
impl AlephTool for DoctorTool {
    const NAME: &'static str = "doctor";
    const DESCRIPTION: &'static str = "Self-diagnose Aleph runtime health: data directory, instance lock, config.toml parse, secret vault integrity, shell-hook consent registry, browser runtime prerequisites, and live LLM provider connectivity (one ping per enabled provider). Returns structured findings with fix hints. Pass fix=true to apply safe, deterministic repairs (recreate a missing data dir, clear a stale lock). Use this to answer 'is something wrong with my setup?' and to VERIFY your repairs after fixing a provider credential or config — read-only by default.";

    type Args = DoctorArgs;
    type Output = DoctorOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, if args.fix { "fix" } else { "inspect" });

        let mut engine = DiagnosticEngine::default_registry()?;
        if let (Some(config), Some(vault)) = (self.config.as_ref(), self.token_manager.as_ref()) {
            engine = engine.with_runtime_checks(Arc::clone(config), Arc::clone(vault));
        }
        let posture = if args.fix {
            Posture::Fix
        } else {
            Posture::Inspect
        };
        let report = engine.run(posture).await;
        let ok = report.ok();
        let summary = format!(
            "{} error(s), {} warning(s), {} repaired — see report.findings",
            report.errors(),
            report.warnings(),
            report.repaired()
        );

        notify_tool_result(Self::NAME, &summary, ok);

        Ok(DoctorOutput {
            ok,
            summary,
            report,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::paths::IsolatedAlephHome;

    #[tokio::test]
    async fn inspect_run_returns_structured_output() {
        let _home = IsolatedAlephHome::new();
        let tool = DoctorTool::default();
        let out = tool.call(DoctorArgs { fix: false }).await.unwrap();

        // Structured payload: every registered check ran and reported.
        assert_eq!(out.report.posture, "inspect");
        assert_eq!(out.report.checks_run, 9);
        assert!(!out.report.findings.is_empty());
        // Summary is a compact one-line tally, not the full human render.
        assert!(!out.summary.contains('\n'));
        assert!(out.summary.contains("error(s)"));
        assert!(out.summary.len() < out.report.render_human().len());
        assert_eq!(out.ok, out.report.ok());
    }

    #[tokio::test]
    async fn fix_run_on_temp_home_completes() {
        let _home = IsolatedAlephHome::new();
        let tool = DoctorTool::default();
        let out = tool.call(DoctorArgs { fix: true }).await.unwrap();

        assert_eq!(out.report.posture, "fix");
        // Any repair that only claimed success would have been re-flagged by
        // the engine's post-repair revalidation.
        assert!(
            !out.report
                .findings
                .iter()
                .any(|f| f.has_tag("post-repair-residual")),
            "unexpected post-repair residual: {:?}",
            out.report.findings
        );
        // The data dir the engine resolved lives under the temp ALEPH_HOME.
        let data_dir = crate::utils::paths::get_data_dir().unwrap();
        assert!(data_dir.exists(), "data dir must exist after a fix run");
    }
}
