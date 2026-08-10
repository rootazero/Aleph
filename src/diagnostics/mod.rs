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
pub mod redact;

pub use check::{HealthCheck, Posture, DEFAULT_CHECK_TIMEOUT};
pub use finding::{Finding, RepairOutcome, Severity};
pub use redact::redact_secrets;

use std::sync::Arc;
use std::time::Instant;

use futures::future::join_all;
use serde::Serialize;

use crate::error::Result;

/// Tag on the synthetic finding emitted when a check blows its deadline.
pub const TAG_CHECK_TIMEOUT: &str = "check-timeout";

/// A check slower than this gets its own line in the human render (codex uses
/// the same 2s threshold to start emitting "still checking…" heartbeats).
const SLOW_CHECK_MS: u64 = 2_000;

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
        // ⚠️ NOT `utils::paths::get_data_dir()`: that helper *creates* the
        // directory as a side effect, so building the registry silently
        // repaired the very condition `core/data-dir` exists to detect —
        // its "Data directory is missing" branch (the flagship repairable
        // finding, with three unit tests) was unreachable in production, and
        // the `doctor --fix` assertion that the dir exists afterwards was
        // satisfied by the constructor, not by any repair. A sensor must not
        // create the thing it measures. `get_config_dir()` is pure lookup.
        let data_dir = crate::utils::paths::get_config_dir()?.join("data");
        // The file this process actually runs on, not where config would live
        // by default — a doctor that parses the wrong file reports health about
        // a config nothing is using.
        let config_path = crate::config::Config::effective_path();

        let checks: Vec<Arc<dyn HealthCheck>> = vec![
            Arc::new(checks::DataDirCheck::new(data_dir.clone())),
            Arc::new(checks::LoopGraphCheck::new(data_dir.clone())),
            Arc::new(checks::StaleLockCheck::new(data_dir.clone())),
            Arc::new(checks::SqliteIntegrityCheck::new(data_dir.clone())),
            Arc::new(checks::DiskSpaceCheck::new(data_dir)),
            Arc::new(checks::ConfigParseCheck::new(config_path)),
            Arc::new(checks::VaultCheck::from_default_path()),
            Arc::new(checks::HooksConsentCheck::from_default_path()),
            Arc::new(checks::BrowserRuntimeCheck::new()),
            Arc::new(checks::DuplicateInstanceCheck::new()),
        ];
        Ok(Self::new(checks))
    }

    /// Append runtime checks that need live daemon handles (config + vault).
    ///
    /// `default_registry()` stays path-only so the offline `aleph-server
    /// doctor` command works without network access; this opt-in upgrade is
    /// used by the `doctor` builtin tool and the `diagnostics.run` RPC inside
    /// the daemon, giving the LLM repair loop and the `aleph doctor` CLI the
    /// same provider-connectivity visibility — so repairs can be verified.
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

    /// Append the `ext/idle-extensions` check.
    ///
    /// Separate from [`Self::with_runtime_checks`] because it needs a
    /// *different* live handle (the MCP manager actor) and has a different
    /// availability story: passing `None` still registers the check, which then
    /// reports the MCP category as UNKNOWN rather than pretending it is clean.
    /// It stays out of `default_registry()` for the same reason
    /// `providers/connectivity` does — an inventory nobody can enumerate is
    /// worse than no inventory line at all.
    #[must_use]
    pub fn with_extension_usage_check(
        mut self,
        mcp: Option<crate::mcp::manager::McpManagerHandle>,
    ) -> Self {
        self.checks
            .push(Arc::new(checks::IdleExtensionsCheck::new(mcp)));
        self
    }

    /// Run every check concurrently and collect a report.
    pub async fn run(&self, posture: Posture) -> DiagnosticReport {
        self.run_with_filter(posture, None, &[]).await
    }

    /// Run a filtered subset of checks and collect a report.
    ///
    /// `only` selects checks by id (e.g. `core/data-dir`); `skip` excludes
    /// them. When both are given, `only` wins (a whitelist is the stronger
    /// statement of intent). Unknown ids are ignored unless no checks remain,
    /// which produces a warning finding.
    ///
    /// In `Fix` posture the engine additionally revalidates: checks that
    /// reported a successful repair are re-run in `Inspect` posture, and any
    /// problem that persists is appended tagged `post-repair-residual` (the
    /// openclaw "re-run detect to validate the repair" pattern) so a repair
    /// that only *claims* success can't turn the report green.
    pub async fn run_with_filter(
        &self,
        posture: Posture,
        only: Option<&[String]>,
        skip: &[String],
    ) -> DiagnosticReport {
        let selected: Vec<&Arc<dyn HealthCheck>> = self
            .checks
            .iter()
            .filter(|c| match only {
                Some(only) => only.iter().any(|id| id == c.id()),
                None => !skip.iter().any(|id| id == c.id()),
            })
            .collect();

        if selected.is_empty() {
            return DiagnosticReport {
                posture: posture_label(posture),
                checks_run: 0,
                findings: vec![Finding::problem(
                    "diagnostics/filter",
                    Severity::Warning,
                    "No diagnostic checks selected",
                    "The requested filters matched no registered diagnostic checks.",
                )],
                timings: Vec::new(),
            };
        }

        let futures = selected.iter().map(|c| run_bounded(c, posture));
        let per_check = join_all(futures).await;

        let mut timings = Vec::with_capacity(per_check.len());
        let mut findings: Vec<Finding> = Vec::new();
        for outcome in per_check {
            timings.push(outcome.timing);
            findings.extend(outcome.findings);
        }

        // Post-repair revalidation: re-inspect just the checks that repaired
        // something and surface whatever still looks broken.
        if posture.allows_repair() {
            let repaired_ids: std::collections::HashSet<&'static str> = findings
                .iter()
                .filter(|f| matches!(f.repair_outcome, Some(RepairOutcome::Repaired { .. })))
                .map(|f| f.check_id)
                .collect();
            if !repaired_ids.is_empty() {
                let rechecks = selected
                    .iter()
                    .filter(|c| repaired_ids.contains(c.id()))
                    .map(|c| run_bounded(c, Posture::Inspect));
                let re_per_check = join_all(rechecks).await;
                for outcome in re_per_check {
                    for f in outcome.findings {
                        if f.is_problem() {
                            findings.push(f.with_tag("post-repair-residual"));
                        }
                    }
                }
            }
        }

        // Redaction chokepoint. Findings reach three consumers that all
        // travel — the CLI, `--json` support bundles, and LLM tool output —
        // and any check can embed a provider error body, a keyed URL, or an
        // io error quoting a config line. Masking here (rather than at each
        // call site that remembered) is what makes the guarantee hold for
        // checks whose authors never thought about credentials.
        let findings = findings.into_iter().map(Finding::redacted).collect();

        DiagnosticReport {
            posture: posture_label(posture),
            checks_run: selected.len(),
            findings,
            timings,
        }
    }
}

/// One check's findings plus how long it took (and whether it blew its
/// deadline).
struct BoundedOutcome {
    findings: Vec<Finding>,
    timing: CheckTiming,
}

/// Run one check under its own [`HealthCheck::timeout`], recording elapsed
/// wall clock. A check that blows the deadline yields a `Warning` finding
/// naming it instead of hanging the whole report — the failure the deadline
/// exists to prevent is a silent stall inside an agent turn, so it must be
/// *reported*, not just avoided.
async fn run_bounded(check: &Arc<dyn HealthCheck>, posture: Posture) -> BoundedOutcome {
    let deadline = check.timeout();
    let started = Instant::now();
    let result = tokio::time::timeout(deadline, check.run(posture)).await;
    let duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

    match result {
        Ok(findings) => BoundedOutcome {
            findings,
            timing: CheckTiming {
                check_id: check.id(),
                duration_ms,
                timed_out: false,
            },
        },
        Err(_) => BoundedOutcome {
            findings: vec![Finding::problem(
                check.id(),
                Severity::Warning,
                format!("{}: check timed out", check.id()),
                format!(
                    "The check did not finish within {}s and was abandoned; its domain is \
                     UNKNOWN, not healthy.",
                    deadline.as_secs()
                ),
            )
            .with_fix_hint(
                "Re-run doctor with this check skipped to get the rest of the report, and \
                 investigate what it waits on (a wedged filesystem, a locked SQLite file, or \
                 an unresponsive endpoint).",
            )
            .with_tag(TAG_CHECK_TIMEOUT)],
            timing: CheckTiming {
                check_id: check.id(),
                duration_ms,
                timed_out: true,
            },
        },
    }
}

const fn posture_label(p: Posture) -> &'static str {
    match p {
        Posture::Inspect => "inspect",
        Posture::Lint => "lint",
        Posture::Fix => "fix",
    }
}

/// Per-check wall-clock accounting, so "which check is slow / which one hung"
/// is answerable from the report instead of by bisecting `--only`.
///
/// codex's doctor carries the same `durationMs` on every check row; Aleph adds
/// `timedOut` because its engine enforces a deadline (codex's checks are
/// unbounded and only emit a progress heartbeat).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckTiming {
    pub check_id: &'static str,
    pub duration_ms: u64,
    pub timed_out: bool,
}

/// Outcome of a full diagnostic run.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticReport {
    pub posture: &'static str,
    pub checks_run: usize,
    pub findings: Vec<Finding>,
    /// One entry per check that ran, in registry order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timings: Vec<CheckTiming>,
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

    /// Checks that blew their deadline this run.
    #[must_use]
    pub fn timed_out(&self) -> usize {
        self.timings.iter().filter(|t| t.timed_out).count()
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
            "timedOut": self.timed_out(),
            "findings": self.findings,
            "timings": self.timings,
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
        // Only surface timings worth a human's attention: anything that blew
        // its deadline, or took long enough to feel like a stall. Printing a
        // duration for every check would bury the findings.
        let mut notable: Vec<&CheckTiming> = self
            .timings
            .iter()
            .filter(|t| t.timed_out || t.duration_ms >= SLOW_CHECK_MS)
            .collect();
        notable.sort_by_key(|t| std::cmp::Reverse(t.duration_ms));
        for t in notable {
            let suffix = if t.timed_out { " (timed out)" } else { "" };
            out.push_str(&format!(
                "  [slow] {} took {}ms{suffix}\n",
                t.check_id, t.duration_ms
            ));
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

    /// A check whose repair actually sticks: Inspect after Fix comes back clean.
    struct GenuineRepairCheck;

    #[async_trait]
    impl HealthCheck for GenuineRepairCheck {
        fn id(&self) -> &'static str {
            "fake/genuine"
        }
        fn title(&self) -> &'static str {
            "fake"
        }
        async fn run(&self, posture: Posture) -> Vec<Finding> {
            if posture.allows_repair() {
                vec![
                    Finding::problem("fake/genuine", Severity::Warning, "stale", "left behind")
                        .repairable()
                        .with_repair(RepairOutcome::Repaired {
                            detail: "cleaned".into(),
                        }),
                ]
            } else {
                vec![Finding::ok("fake/genuine", "fine", "ok")]
            }
        }
    }

    #[tokio::test]
    async fn report_ok_when_repaired() {
        let engine = DiagnosticEngine::new(vec![Arc::new(GenuineRepairCheck)]);
        let report = engine.run(Posture::Fix).await;
        assert!(report.ok(), "a repaired problem should not block ok()");
        assert_eq!(report.repaired(), 1);
    }

    #[tokio::test]
    async fn filter_only_selects_named_checks() {
        let engine = DiagnosticEngine::new(vec![
            Arc::new(FakeCheck {
                id: "a",
                finding: Finding::ok("a", "fine", "ok"),
            }),
            Arc::new(FakeCheck {
                id: "b",
                finding: Finding::ok("b", "fine", "ok"),
            }),
        ]);
        let only = vec!["a".to_string()];
        let report = engine
            .run_with_filter(Posture::Inspect, Some(&only), &[])
            .await;
        assert_eq!(report.checks_run, 1);
        assert!(report.findings.iter().all(|f| f.check_id == "a"));
    }

    #[tokio::test]
    async fn filter_matching_no_checks_returns_warning() {
        let engine = DiagnosticEngine::new(vec![Arc::new(FakeCheck {
            id: "a",
            finding: Finding::ok("a", "fine", "ok"),
        })]);
        let only = vec!["missing".to_string()];
        let report = engine
            .run_with_filter(Posture::Inspect, Some(&only), &[])
            .await;
        assert_eq!(report.checks_run, 0);
        assert_eq!(report.warnings(), 1);
        assert_eq!(report.findings[0].check_id, "diagnostics/filter");
        assert!(!report.ok());
        assert!(report.to_json().contains("\"ok\":false"));
    }

    #[tokio::test]
    async fn filter_skip_excludes_named_checks() {
        let engine = DiagnosticEngine::new(vec![
            Arc::new(FakeCheck {
                id: "a",
                finding: Finding::ok("a", "fine", "ok"),
            }),
            Arc::new(FakeCheck {
                id: "b",
                finding: Finding::ok("b", "fine", "ok"),
            }),
        ]);
        let skip = vec!["a".to_string()];
        let report = engine.run_with_filter(Posture::Inspect, None, &skip).await;
        assert_eq!(report.checks_run, 1);
        assert!(report.findings.iter().all(|f| f.check_id == "b"));
    }

    #[tokio::test]
    async fn filter_only_wins_over_skip() {
        let engine = DiagnosticEngine::new(vec![Arc::new(FakeCheck {
            id: "a",
            finding: Finding::ok("a", "fine", "ok"),
        })]);
        let only = vec!["a".to_string()];
        let skip = vec!["a".to_string()];
        let report = engine
            .run_with_filter(Posture::Inspect, Some(&only), &skip)
            .await;
        assert_eq!(report.checks_run, 1, "only must win when both are given");
    }

    /// A check whose "repair" is theatre: Fix reports success, but the
    /// problem is still there on re-inspection.
    struct RepairTheatreCheck;

    #[async_trait]
    impl HealthCheck for RepairTheatreCheck {
        fn id(&self) -> &'static str {
            "fake/theatre"
        }
        fn title(&self) -> &'static str {
            "fake"
        }
        async fn run(&self, posture: Posture) -> Vec<Finding> {
            let f = Finding::problem("fake/theatre", Severity::Warning, "stale", "left behind")
                .repairable();
            if posture.allows_repair() {
                vec![f.with_repair(RepairOutcome::Repaired {
                    detail: "cleaned".into(),
                })]
            } else {
                vec![f]
            }
        }
    }

    #[tokio::test]
    async fn fix_revalidates_and_flags_repair_theatre() {
        let engine = DiagnosticEngine::new(vec![Arc::new(RepairTheatreCheck)]);
        let report = engine.run(Posture::Fix).await;
        assert_eq!(report.repaired(), 1);
        let residual: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.has_tag("post-repair-residual"))
            .collect();
        assert_eq!(residual.len(), 1, "the persisting problem must resurface");
        assert_eq!(
            residual[0].severity,
            Severity::Warning,
            "severity preserved"
        );
        assert!(
            !report.ok(),
            "a failed repair-verify must keep the report red"
        );
    }

    /// A check that leaks a credential shape and knows nothing about masking —
    /// which is exactly the check the chokepoint has to cover.
    struct LeakyCheck;

    #[async_trait]
    impl HealthCheck for LeakyCheck {
        fn id(&self) -> &'static str {
            "fake/leaky"
        }
        fn title(&self) -> &'static str {
            "fake"
        }
        async fn run(&self, _posture: Posture) -> Vec<Finding> {
            vec![Finding::problem(
                "fake/leaky",
                Severity::Warning,
                "probe failed",
                "server rejected sk-abcdefgh12345678",
            )
            .with_fix_hint("try Bearer eyJhbGciOi again")]
        }
    }

    #[tokio::test]
    async fn engine_redacts_findings_from_checks_that_never_asked() {
        let engine = DiagnosticEngine::new(vec![Arc::new(LeakyCheck)]);
        let report = engine.run(Posture::Inspect).await;
        assert_eq!(report.findings[0].detail, "server rejected ***");
        assert_eq!(
            report.findings[0].fix_hint.as_deref(),
            Some("try *** again")
        );
        assert!(
            !report.to_json().contains("sk-abcdefgh"),
            "the JSON envelope must not carry the raw credential"
        );
    }

    /// A check that never returns. Without a deadline it would hang the whole
    /// report — and, inside the `doctor` tool, the whole agent turn.
    struct WedgedCheck;

    #[async_trait]
    impl HealthCheck for WedgedCheck {
        fn id(&self) -> &'static str {
            "fake/wedged"
        }
        fn title(&self) -> &'static str {
            "fake"
        }
        async fn run(&self, _posture: Posture) -> Vec<Finding> {
            std::future::pending::<()>().await;
            unreachable!()
        }
        fn timeout(&self) -> std::time::Duration {
            std::time::Duration::from_millis(50)
        }
    }

    #[tokio::test]
    async fn a_wedged_check_is_reported_not_waited_on() {
        let engine = DiagnosticEngine::new(vec![
            Arc::new(WedgedCheck),
            Arc::new(FakeCheck {
                id: "a",
                finding: Finding::ok("a", "fine", "ok"),
            }),
        ]);
        let report = engine.run(Posture::Inspect).await;

        // The healthy sibling still reported.
        assert!(report.findings.iter().any(|f| f.check_id == "a"));
        // The wedged one is a finding, not silence: "unknown" must not read
        // as "healthy".
        let timeout_finding = report
            .findings
            .iter()
            .find(|f| f.has_tag(TAG_CHECK_TIMEOUT))
            .expect("a blown deadline must surface as a finding");
        assert_eq!(timeout_finding.severity, Severity::Warning);
        assert!(
            !report.ok(),
            "an unknown domain cannot make the report green"
        );
        assert_eq!(report.timed_out(), 1);
        assert!(report
            .timings
            .iter()
            .any(|t| t.check_id == "fake/wedged" && t.timed_out));
        assert!(report.to_json().contains("\"timedOut\":1"));
    }

    #[tokio::test]
    async fn timings_cover_every_selected_check() {
        let engine = DiagnosticEngine::new(vec![
            Arc::new(FakeCheck {
                id: "a",
                finding: Finding::ok("a", "fine", "ok"),
            }),
            Arc::new(FakeCheck {
                id: "b",
                finding: Finding::ok("b", "fine", "ok"),
            }),
        ]);
        let skip = vec!["a".to_string()];
        let report = engine.run_with_filter(Posture::Inspect, None, &skip).await;
        assert_eq!(report.timings.len(), 1, "timings follow the filter");
        assert_eq!(report.timings[0].check_id, "b");
        assert!(!report.timings[0].timed_out);
    }

    #[tokio::test]
    async fn fix_revalidation_passes_for_genuine_repairs() {
        let engine = DiagnosticEngine::new(vec![Arc::new(GenuineRepairCheck)]);
        let report = engine.run(Posture::Fix).await;
        assert_eq!(report.repaired(), 1);
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.has_tag("post-repair-residual")),
            "a genuine repair must not produce residuals"
        );
        assert!(report.ok());
    }
}
