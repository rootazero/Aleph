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

    /// Whitelist of check ids to run. Wins over `skip` when both are given.
    #[serde(default)]
    #[schemars(
        description = "Run only these check ids (e.g. [\"core/data-dir\"]). Wins over `skip`."
    )]
    pub only: Option<Vec<String>>,

    /// Blacklist of check ids to leave out.
    #[serde(default)]
    #[schemars(
        description = "Skip these check ids. Use [\"providers/connectivity\"] to answer a \
                       filesystem/config question without paying one network probe per \
                       configured LLM provider."
    )]
    pub skip: Vec<String>,
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
    // ⚠️ This constant ships in every request that lists the tool, and its
    // bytes are ratcheted (`definitions.rs::catalog_description_bytes_ratchet`).
    // The 2026-08-05 round ADDED four checks and two arguments while coming in
    // 4 bytes UNDER the previous text — by dropping prose the model can get
    // from the tool's own output (the findings enumerate every check id) and
    // keeping only what it cannot: that `only`/`skip` exist, and that
    // `providers/connectivity` is the expensive one.
    const DESCRIPTION: &'static str = "Self-diagnose Aleph runtime health: paths, disk space, instance lock, duplicate daemons, SQLite store integrity, config.toml parse, secret vault, hook consent, browser prerequisites, LLM provider reachability. Structured findings with fix hints. fix=true applies safe mechanical repairs (missing data dir, stale lock) and re-verifies them. only=/skip=[check ids] narrow it; skip=[\"providers/connectivity\"] avoids a network probe per provider. Read-only by default — also use it to VERIFY a repair after a credential or config fix.";

    type Args = DoctorArgs;
    type Output = DoctorOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, if args.fix { "fix" } else { "inspect" });
        // `fix` is the only argument the concurrency claim reads
        // (`registry_adapter::doctor_claim`); `only` / `skip` narrow the same
        // run and never widen what a repair may touch.

        let mut engine = DiagnosticEngine::default_registry()?;
        if let (Some(config), Some(vault)) = (self.config.as_ref(), self.token_manager.as_ref()) {
            engine = engine.with_runtime_checks(Arc::clone(config), Arc::clone(vault));
        }
        let posture = if args.fix {
            Posture::Fix
        } else {
            Posture::Inspect
        };
        let report = engine
            .run_with_filter(posture, args.only.as_deref(), &args.skip)
            .await;
        let ok = report.ok();
        let timed_out = report.timed_out();
        let summary = format!(
            "{} error(s), {} warning(s), {} repaired{} — see report.findings",
            report.errors(),
            report.warnings(),
            report.repaired(),
            if timed_out > 0 {
                format!(", {timed_out} check(s) timed out")
            } else {
                String::new()
            }
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

    /// Registered-check count. Asserted rather than derived so adding a check
    /// is a deliberate edit here too — the alternative (`>= 1`) would let a
    /// check silently drop out of `default_registry`.
    const REGISTERED_CHECKS: usize = 10;

    fn inspect_args() -> DoctorArgs {
        DoctorArgs::default()
    }

    #[tokio::test]
    async fn inspect_run_returns_structured_output() {
        let _home = IsolatedAlephHome::new();
        let tool = DoctorTool::default();
        let out = tool.call(inspect_args()).await.unwrap();

        // Structured payload: every registered check ran and reported.
        assert_eq!(out.report.posture, "inspect");
        assert_eq!(out.report.checks_run, REGISTERED_CHECKS);
        assert_eq!(out.report.timings.len(), REGISTERED_CHECKS);
        assert!(!out.report.findings.is_empty());
        // Summary is a compact one-line tally, not the full human render.
        assert!(!out.summary.contains('\n'));
        assert!(out.summary.contains("error(s)"));
        assert!(out.summary.len() < out.report.render_human().len());
        assert_eq!(out.ok, out.report.ok());
    }

    #[tokio::test]
    async fn only_and_skip_narrow_the_battery() {
        let _home = IsolatedAlephHome::new();
        let tool = DoctorTool::default();

        let only = tool
            .call(DoctorArgs {
                only: Some(vec!["core/data-dir".to_string()]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(only.report.checks_run, 1);
        assert!(only
            .report
            .findings
            .iter()
            .all(|f| f.check_id == "core/data-dir"));

        let skipped = tool
            .call(DoctorArgs {
                skip: vec!["core/data-dir".to_string()],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(skipped.report.checks_run, REGISTERED_CHECKS - 1);
        assert!(skipped
            .report
            .findings
            .iter()
            .all(|f| f.check_id != "core/data-dir"));
    }

    /// The `core/data-dir` "missing" branch must be reachable in production.
    /// It was not: `default_registry()` resolved the path through
    /// `get_data_dir()`, which creates the directory — so building the engine
    /// repaired the condition before the check could see it, and the Fix
    /// posture's flagship repair could never fire.
    #[tokio::test]
    async fn building_the_registry_does_not_create_the_data_dir() {
        let _home = IsolatedAlephHome::new();
        let data_dir = crate::utils::paths::get_config_dir().unwrap().join("data");
        let _ = std::fs::remove_dir_all(&data_dir);
        assert!(!data_dir.exists(), "precondition: data dir removed");

        let tool = DoctorTool::default();
        let out = tool.call(inspect_args()).await.unwrap();

        assert!(
            !data_dir.exists(),
            "a read-only inspect must not create the directory it inspects"
        );
        let finding = out
            .report
            .findings
            .iter()
            .find(|f| f.check_id == "core/data-dir")
            .expect("data-dir check must report");
        assert!(finding.is_problem(), "missing data dir must be a problem");
        assert!(finding.repairable);
        assert!(finding.repair_outcome.is_none(), "inspect never repairs");
    }

    #[tokio::test]
    async fn fix_actually_creates_the_missing_data_dir() {
        let _home = IsolatedAlephHome::new();
        let data_dir = crate::utils::paths::get_config_dir().unwrap().join("data");
        let _ = std::fs::remove_dir_all(&data_dir);
        assert!(!data_dir.exists());

        let tool = DoctorTool::default();
        let out = tool
            .call(DoctorArgs {
                fix: true,
                ..Default::default()
            })
            .await
            .unwrap();

        assert!(data_dir.exists(), "fix posture must create it");
        assert_eq!(out.report.repaired(), 1, "exactly the data-dir repair");
        assert!(
            !out.report
                .findings
                .iter()
                .any(|f| f.has_tag("post-repair-residual")),
            "a genuine repair leaves no residual: {:?}",
            out.report.findings
        );
    }

    #[tokio::test]
    async fn fix_run_on_temp_home_completes() {
        let _home = IsolatedAlephHome::new();
        let tool = DoctorTool::default();
        let out = tool
            .call(DoctorArgs {
                fix: true,
                ..Default::default()
            })
            .await
            .unwrap();

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
        // Resolved with the NON-creating helper on purpose: `get_data_dir()`
        // would satisfy this assertion by creating the directory itself, which
        // is exactly the bug `building_the_registry_does_not_create_the_data_dir`
        // covers — an assertion that manufactures its own subject proves nothing.
        let data_dir = crate::utils::paths::get_config_dir().unwrap().join("data");
        assert!(data_dir.exists(), "data dir must exist after a fix run");
    }
}
