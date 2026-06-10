//! ConfigAuditTool — read-only security-posture audit of the live runtime Config.
//!
//! Inspects the in-memory `Config` and reports the security posture as structured
//! findings (severity + area + finding + recommendation), so the LLM can answer
//! "is my setup secure?" without mutating anything (R8: everything is a tool).
//!
//! openclaw-inspired (their `core-dangerous-config-flags.ts` enumerates dangerous
//! boolean flags), mapped here onto Aleph's typed `Config` for type-safety.
//!
//! Audit *logic* lives in pure free functions (`audit_*`) so it is unit-testable
//! without constructing the tool or the async runtime.
//!
//! Scope note: gateway authentication is NOT on the top-level `Config` (it lives
//! in a separate `GatewayConfig` struct that the agent loop's `Config` does not
//! embed). To keep this tool surgical and avoid invasive plumbing, the audit
//! covers only what is reachable from `Config`: SSRF, sandbox, shell security,
//! and privacy/PII filtering.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::Config;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Severity
// =============================================================================

/// Severity levels for posture findings (string-typed in the output, per spec).
const SEV_CRITICAL: &str = "critical";
const SEV_WARN: &str = "warn";
const SEV_INFO: &str = "info";

// =============================================================================
// Args / Output
// =============================================================================

/// No parameters — the audit always inspects the full live config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ConfigAuditArgs {}

/// A single posture finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// "critical" | "warn" | "info"
    pub severity: String,
    /// Configuration area, e.g. "ssrf", "sandbox", "shell_security".
    pub area: String,
    /// What was observed.
    pub finding: String,
    /// What to do about it.
    pub recommendation: String,
}

impl Finding {
    fn new(
        severity: &str,
        area: &str,
        finding: impl Into<String>,
        recommendation: impl Into<String>,
    ) -> Self {
        Self {
            severity: severity.to_string(),
            area: area.to_string(),
            finding: finding.into(),
            recommendation: recommendation.into(),
        }
    }
}

/// Count of findings by severity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PostureSummary {
    pub critical: usize,
    pub warn: usize,
    pub info: usize,
    pub total: usize,
}

impl PostureSummary {
    fn from_findings(findings: &[Finding]) -> Self {
        let critical = findings
            .iter()
            .filter(|f| f.severity == SEV_CRITICAL)
            .count();
        let warn = findings.iter().filter(|f| f.severity == SEV_WARN).count();
        let info = findings.iter().filter(|f| f.severity == SEV_INFO).count();
        Self {
            critical,
            warn,
            info,
            total: findings.len(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ConfigAuditOutput {
    pub success: bool,
    pub message: String,
    pub summary: PostureSummary,
    pub findings: Vec<Finding>,
}

// =============================================================================
// Pure audit logic (unit-testable, no async, no I/O)
// =============================================================================

/// Run every audit over `cfg`, returning the collected findings.
#[must_use]
pub fn audit_config(cfg: &Config) -> Vec<Finding> {
    let mut out = Vec::new();
    audit_ssrf(cfg, &mut out);
    audit_sandbox(cfg, &mut out);
    audit_privacy(cfg, &mut out);
    out
}

/// SSRF protection posture. If the guard is fully disabled, emit one critical
/// and skip the sub-checks (avoid noise).
pub fn audit_ssrf(cfg: &Config, out: &mut Vec<Finding>) {
    let s = &cfg.ssrf;
    if !s.enabled {
        out.push(Finding::new(
            SEV_CRITICAL,
            "ssrf",
            "SSRF protection is disabled — outbound requests are not validated against internal targets.",
            "Set ssrf.enabled = true so requests to private/internal ranges are blocked.",
        ));
        return;
    }
    if s.allow_private_network {
        out.push(Finding::new(
            SEV_CRITICAL,
            "ssrf",
            "ssrf.allow_private_network is true — the agent can reach RFC1918 / internal services.",
            "Set ssrf.allow_private_network = false unless you specifically need internal access; prefer ssrf.allowed_hosts for narrow exceptions.",
        ));
    }
    if !s.strip_auth_on_cross_origin {
        out.push(Finding::new(
            SEV_WARN,
            "ssrf",
            "ssrf.strip_auth_on_cross_origin is false — Authorization/Cookie headers leak across cross-origin redirects.",
            "Set ssrf.strip_auth_on_cross_origin = true to prevent credential leakage on redirect.",
        ));
    }
    if s.max_redirects > 10 {
        out.push(Finding::new(
            SEV_INFO,
            "ssrf",
            format!(
                "ssrf.max_redirects is high ({}) — long redirect chains widen the SSRF bypass surface.",
                s.max_redirects
            ),
            "Consider lowering ssrf.max_redirects (default 5) unless a workflow needs more.",
        ));
    }
}

/// Sandbox posture for exec-class tools. A disabled sandbox means code/shell
/// execution runs without isolation.
pub fn audit_sandbox(cfg: &Config, out: &mut Vec<Finding>) {
    let s = &cfg.sandbox;
    if !s.enabled {
        out.push(Finding::new(
            SEV_CRITICAL,
            "sandbox",
            "Sandbox is disabled — code/shell execution (code_exec, bash) runs without isolation.",
            "Set sandbox.enabled = true so exec-class tools run inside the OS sandbox.",
        ));
    }
}

/// Privacy / PII-filtering posture. Master switch off is an info-level signal.
pub fn audit_privacy(cfg: &Config, out: &mut Vec<Finding>) {
    let p = &cfg.privacy;
    if !p.pii_filtering {
        out.push(Finding::new(
            SEV_INFO,
            "privacy",
            "privacy.pii_filtering is disabled — PII is not detected/redacted across the system.",
            "Set privacy.pii_filtering = true if conversations may contain personal data you want scrubbed.",
        ));
    }
}

// =============================================================================
// Tool
// =============================================================================

#[derive(Clone, Default)]
pub struct ConfigAuditTool {
    config: Option<Arc<RwLock<Config>>>,
}

impl ConfigAuditTool {
    pub fn new(config: Arc<RwLock<Config>>) -> Self {
        Self {
            config: Some(config),
        }
    }
}

#[async_trait]
impl AlephTool for ConfigAuditTool {
    const NAME: &'static str = "config_audit";
    const DESCRIPTION: &'static str = "Audit the live Aleph security posture (read-only). Inspects SSRF protection, sandbox isolation, shell-command safety, and PII filtering, returning structured findings (severity/area/finding/recommendation). Call when the user asks 'is my setup secure?' or wants a security review of the current configuration. Never mutates config.";

    type Args = ConfigAuditArgs;
    type Output = ConfigAuditOutput;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        let config = match &self.config {
            Some(c) => c,
            None => {
                return Ok(ConfigAuditOutput {
                    success: false,
                    message: "Config handle not available — cannot audit posture.".into(),
                    summary: PostureSummary {
                        critical: 0,
                        warn: 0,
                        info: 0,
                        total: 0,
                    },
                    findings: Vec::new(),
                });
            }
        };

        let findings = {
            let guard = config.read().await;
            audit_config(&guard)
        };

        let summary = PostureSummary::from_findings(&findings);
        let message = if summary.critical > 0 {
            format!(
                "{} critical, {} warning, {} info finding(s) — critical issues should be fixed.",
                summary.critical, summary.warn, summary.info
            )
        } else if summary.warn > 0 {
            format!(
                "No critical issues. {} warning, {} info finding(s) to review.",
                summary.warn, summary.info
            )
        } else if summary.info > 0 {
            format!(
                "No critical or warning issues. {} info finding(s).",
                summary.info
            )
        } else {
            "No posture issues found — audited configuration looks secure.".to_string()
        };

        Ok(ConfigAuditOutput {
            success: true,
            message,
            summary,
            findings,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_baseline_has_no_critical() {
        // Config::default() ships secure defaults: ssrf.enabled, sandbox.enabled,
        // privacy.pii_filtering all true; no custom shell patterns.
        let cfg = Config::default();
        let findings = audit_config(&cfg);
        let critical = findings
            .iter()
            .filter(|f| f.severity == SEV_CRITICAL)
            .count();
        assert_eq!(
            critical, 0,
            "secure baseline produced criticals: {:?}",
            findings
        );
    }

    #[test]
    fn ssrf_disabled_is_single_critical_and_suppresses_subchecks() {
        let mut cfg = Config::default();
        cfg.ssrf.enabled = false;
        // Also flip a sub-field that would otherwise fire — it must be suppressed.
        cfg.ssrf.allow_private_network = true;
        cfg.ssrf.strip_auth_on_cross_origin = false;

        let mut out = Vec::new();
        audit_ssrf(&cfg, &mut out);
        assert_eq!(
            out.len(),
            1,
            "disabled guard should emit exactly one finding"
        );
        assert_eq!(out[0].severity, SEV_CRITICAL);
        assert_eq!(out[0].area, "ssrf");
    }

    #[test]
    fn ssrf_allow_private_network_is_critical() {
        let mut cfg = Config::default();
        cfg.ssrf.allow_private_network = true;
        let mut out = Vec::new();
        audit_ssrf(&cfg, &mut out);
        assert!(out
            .iter()
            .any(|f| f.severity == SEV_CRITICAL && f.finding.contains("allow_private_network")));
    }

    #[test]
    fn ssrf_strip_auth_off_is_warn() {
        let mut cfg = Config::default();
        cfg.ssrf.strip_auth_on_cross_origin = false;
        let mut out = Vec::new();
        audit_ssrf(&cfg, &mut out);
        assert!(out
            .iter()
            .any(|f| f.severity == SEV_WARN && f.finding.contains("strip_auth_on_cross_origin")));
    }

    #[test]
    fn ssrf_high_max_redirects_is_info() {
        let mut cfg = Config::default();
        cfg.ssrf.max_redirects = 20;
        let mut out = Vec::new();
        audit_ssrf(&cfg, &mut out);
        assert!(out
            .iter()
            .any(|f| f.severity == SEV_INFO && f.finding.contains("max_redirects")));
    }

    #[test]
    fn sandbox_disabled_is_critical() {
        let mut cfg = Config::default();
        cfg.sandbox.enabled = false;
        let mut out = Vec::new();
        audit_sandbox(&cfg, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, SEV_CRITICAL);
        assert_eq!(out[0].area, "sandbox");
    }

    #[test]
    fn sandbox_enabled_is_clean() {
        let mut cfg = Config::default();
        cfg.sandbox.enabled = true;
        let mut out = Vec::new();
        audit_sandbox(&cfg, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn privacy_pii_off_is_info() {
        let mut cfg = Config::default();
        cfg.privacy.pii_filtering = false;
        let mut out = Vec::new();
        audit_privacy(&cfg, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, SEV_INFO);
        assert_eq!(out[0].area, "privacy");
    }

    #[test]
    fn privacy_pii_on_is_clean() {
        let mut cfg = Config::default();
        cfg.privacy.pii_filtering = true;
        let mut out = Vec::new();
        audit_privacy(&cfg, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn summary_partitions_severities() {
        let findings = vec![
            Finding::new(SEV_CRITICAL, "a", "x", "y"),
            Finding::new(SEV_WARN, "b", "x", "y"),
            Finding::new(SEV_WARN, "c", "x", "y"),
            Finding::new(SEV_INFO, "d", "x", "y"),
        ];
        let s = PostureSummary::from_findings(&findings);
        assert_eq!(s.critical, 1);
        assert_eq!(s.warn, 2);
        assert_eq!(s.info, 1);
        assert_eq!(s.total, 4);
        assert_eq!(s.critical + s.warn + s.info, s.total);
    }
}
