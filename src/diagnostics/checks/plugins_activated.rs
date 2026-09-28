//! `extension/plugins-activated` — which mounted plugins never reached a
//! terminal status, and what each is waiting for.
//!
//! The doctor face of `extension::activation_gate`: the boot task logs the
//! same report once; this check re-asks the live registry on every run so an
//! operator who missed the log line still gets the answer, with the
//! dependency named. It reads `PluginStatus::Pending { waiting_on }` as
//! written by `readiness::write_readiness` from `lifecycle.rs`; it never re-derives.
//!
//! Registered by the two daemon faces only (`doctor` tool, `diagnostics.run`)
//! through [`DiagnosticEngine::with_plugins_activated_check`]; the cold
//! `aleph-server doctor` has no extension manager and must not pretend the
//! plugin set is clean — hence UNKNOWN when the handle is absent.

use async_trait::async_trait;

use crate::diagnostics::check::{unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};
use crate::extension::activation_gate::{assess, ActivationReport};
use crate::extension::{PluginRecord, PluginRegistry};

const ID: &str = "extension/plugins-activated";
const SUBJECT: &str = "Plugin activation";

pub struct PluginsActivatedCheck {
    source: Source,
}

enum Source {
    /// The daemon: ask the live registry on every run.
    Live,
    /// A snapshot (tests), or `None` = no manager (the cold process).
    Records(Option<Vec<PluginRecord>>),
}

impl PluginsActivatedCheck {
    /// The daemon face: reads `extension::try_extension_manager()` at run time.
    #[must_use]
    pub const fn live() -> Self {
        Self {
            source: Source::Live,
        }
    }

    /// A fixed snapshot; `None` reproduces "no extension manager".
    #[must_use]
    pub const fn from_records(records: Option<Vec<PluginRecord>>) -> Self {
        Self {
            source: Source::Records(records),
        }
    }

    async fn report(&self) -> Option<ActivationReport> {
        match &self.source {
            Source::Live => {
                let manager = crate::extension::try_extension_manager()?;
                let registry = manager.get_plugin_registry().await;
                Some(assess(&registry))
            }
            Source::Records(None) => None,
            Source::Records(Some(records)) => {
                let mut registry = PluginRegistry::new();
                for r in records {
                    registry.register_plugin(r.clone());
                }
                Some(assess(&registry))
            }
        }
    }
}

#[async_trait]
impl HealthCheck for PluginsActivatedCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Plugin activation"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        let Some(report) = self.report().await else {
            return vec![unknown_finding(
                ID,
                SUBJECT,
                "this process has no extension manager, so no plugin's status could be \
                 read. Run `aleph doctor` against the running daemon rather than \
                 `aleph-server doctor`, which is a cold process.",
            )];
        };
        if report.is_clean() {
            return vec![Finding::ok(
                ID,
                "Every plugin reached a terminal status",
                "no plugin is still waiting on a dependency",
            )];
        }
        vec![Finding::problem(
            ID,
            Severity::Warning,
            "Plugins still waiting on a dependency",
            report.render_lines().join("; "),
        )
        .with_fix_hint(
            "each line names what the plugin waits for: `mcp:manager` = the MCP subsystem \
             never attached (is `[mcp]` enabled?); `mcp:<server>` = that server never \
             answered initialize (see `mcp.list` / the server's log). Statuses change only \
             when the dependency reports — there is no timeout.",
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::check::{HealthCheck, Posture};
    use crate::diagnostics::finding::Severity;
    use crate::extension::{PluginKind, PluginOrigin, PluginRecord, PluginStatus};

    fn snapshot(statuses: &[(&str, PluginStatus)]) -> Vec<PluginRecord> {
        statuses
            .iter()
            .map(|(id, s)| {
                let mut rec = PluginRecord::new(
                    (*id).into(),
                    (*id).into(),
                    PluginKind::Mcp,
                    PluginOrigin::Global,
                );
                rec.status = s.clone();
                rec
            })
            .collect()
    }

    #[tokio::test]
    async fn no_manager_is_unknown_not_clean() {
        let check = PluginsActivatedCheck::from_records(None);
        let f = check.run(Posture::Inspect).await;
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].check_id, "extension/plugins-activated");
        assert!(matches!(f[0].severity, Severity::Warning), "{:?}", f[0]);
        assert!(f[0].title.contains("unknown"));
    }

    #[tokio::test]
    async fn a_pending_plugin_is_a_warning_that_names_the_dependency() {
        let check = PluginsActivatedCheck::from_records(Some(snapshot(&[
            ("ok", PluginStatus::Loaded),
            (
                "wait",
                PluginStatus::Pending {
                    waiting_on: vec!["mcp:manager".into()],
                },
            ),
        ])));
        let f = check.run(Posture::Inspect).await;
        assert_eq!(f.len(), 1);
        assert!(matches!(f[0].severity, Severity::Warning));
        assert!(
            f[0].detail.contains("wait") && f[0].detail.contains("mcp:manager"),
            "{}",
            f[0].detail
        );
    }

    #[tokio::test]
    async fn all_terminal_is_ok() {
        let check = PluginsActivatedCheck::from_records(Some(snapshot(&[
            ("ok", PluginStatus::Loaded),
            ("bad", PluginStatus::Error("e".into())),
        ])));
        let f = check.run(Posture::Inspect).await;
        assert_eq!(f.len(), 1);
        // `Finding::ok` is `Severity::Info` (`finding.rs:64-68`); there is no `is_ok()`.
        assert!(matches!(f[0].severity, Severity::Info), "{:?}", f[0]);
    }
}
