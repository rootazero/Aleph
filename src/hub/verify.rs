//! Post-install verification (spec §10). MCP: started + lists ≥1 tool.
//! Plugin: artifact present on disk. Honest report — never silent "success".
use crate::hub::install::InstallOutcome;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub detail: String,
}

/// Distinguish "the process is up" from "the process is up AND behaving". A
/// `Degraded` server is running — it answers MCP requests, it's just failed a
/// probe recently — so verdict() must not treat it as "not running". Callers
/// that want strict health gating should read `health` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthObservation {
    Healthy,
    Degraded,
    /// The server is up but the system could not classify its health (e.g. the
    /// MCP manager handle isn't available). Treated as "running but unknown
    /// health" for verdict purposes.
    Unknown,
}

/// Pure verdict from an MCP server's observed state.
///
/// `running` means "the manager reports the process is up" — this is `true`
/// for both `Healthy` and `Degraded { .. }`.
///
/// `other_capability_count` is the server's resources + resource templates +
/// prompts. A server that exposes only those is a legitimate MCP server (a docs
/// or resource provider), so the failure condition is "exposes *nothing*" rather
/// than "exposes no tools" — otherwise a working install gets defamed.
#[must_use]
pub fn verdict(
    running: bool,
    tool_count: usize,
    other_capability_count: usize,
) -> VerifyReport {
    verdict_with_health(running, HealthObservation::Unknown, tool_count, other_capability_count)
}

/// Same as [`verdict`] but exposes the health-classification alongside `ok` so
/// callers (e.g. `extensions.install` and the verification tool) can render
/// the distinction without re-deriving it from `HealthStatus`.
#[must_use]
pub fn verdict_with_health(
    running: bool,
    health: HealthObservation,
    tool_count: usize,
    other_capability_count: usize,
) -> VerifyReport {
    if !running {
        return VerifyReport {
            ok: false,
            detail: "server not running".into(),
        };
    }
    match (tool_count, other_capability_count) {
        (0, 0) => VerifyReport {
            ok: false,
            detail: "running but exposes no tools, resources or prompts".into(),
        },
        (0, other) => {
            let health_note = match health {
                HealthObservation::Degraded => " (degraded)",
                HealthObservation::Healthy | HealthObservation::Unknown => "",
            };
            VerifyReport {
                ok: true,
                detail: format!(
                    "running; 0 tools, {other} resources/prompts{health_note}"
                ),
            }
        }
        (tools, _) => {
            let health_note = match health {
                HealthObservation::Degraded => " (degraded)",
                HealthObservation::Healthy | HealthObservation::Unknown => "",
            };
            VerifyReport {
                ok: true,
                detail: format!("running; {tools} tools{health_note}"),
            }
        }
    }
}

/// Verify an install outcome. MCP uses the manager handle; plugin checks disk.
pub async fn verify_install(
    outcome: &InstallOutcome,
    mcp: Option<&crate::mcp::manager::McpManagerHandle>,
) -> VerifyReport {
    match outcome {
        InstallOutcome::Mcp { id } => {
            let Some(mcp) = mcp else {
                return VerifyReport {
                    ok: false,
                    detail: "MCP manager unavailable".into(),
                };
            };
            // list_servers returns all registered servers; find by id.
            match mcp.list_servers().await {
                Ok(servers) => match servers.into_iter().find(|s| &s.id == id) {
                    Some(info) => {
                        // Healthy and Degraded are both "running" — the manager's
                        // circuit breaker puts Degraded below Healthy but above
                        // Unhealthy/Restarting/Dead/Stopped. Treating Degraded
                        // as "not running" caused verify_install to mis-report a
                        // recoverable post-install blip as a hard failure (see
                        // review/hub-statics).
                        let (running, observation) = match info.health {
                            crate::mcp::manager::HealthStatus::Healthy => {
                                (true, HealthObservation::Healthy)
                            }
                            crate::mcp::manager::HealthStatus::Degraded { .. } => {
                                (true, HealthObservation::Degraded)
                            }
                            _ => (false, HealthObservation::Unknown),
                        };
                        let other =
                            info.resource_count + info.resource_template_count + info.prompt_count;
                        verdict_with_health(running, observation, info.tool_count, other)
                    }
                    None => VerifyReport {
                        ok: false,
                        detail: format!("server '{id}' not found"),
                    },
                },
                Err(e) => VerifyReport {
                    ok: false,
                    detail: format!("failed to query MCP manager: {e}"),
                },
            }
        }
        InstallOutcome::Plugin { path } => {
            if std::path::Path::new(path).exists() {
                VerifyReport {
                    ok: true,
                    detail: format!("plugin present at {path}"),
                }
            } else {
                VerifyReport {
                    ok: false,
                    detail: format!("plugin path missing: {path}"),
                }
            }
        }
        InstallOutcome::Skill { path } => {
            if std::path::Path::new(path).exists() {
                VerifyReport {
                    ok: true,
                    detail: format!("skill present at {path}"),
                }
            } else {
                VerifyReport {
                    ok: false,
                    detail: format!("skill path missing: {path}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_with_tools_is_ok() {
        let r = verdict(true, 3, 0);
        assert!(r.ok);
        assert!(r.detail.contains('3'));
    }

    #[test]
    fn running_with_nothing_exposed_is_fail() {
        let r = verdict(true, 0, 0);
        assert!(!r.ok);
        assert!(r.detail.contains("no tools"));
    }

    /// A resources-only MCP server is a real server. Judging on tool count alone
    /// reported a working install as broken.
    #[test]
    fn running_with_only_resources_is_ok() {
        let r = verdict(true, 0, 4);
        assert!(r.ok);
        assert!(r.detail.contains('4'));
    }

    #[test]
    fn not_running_is_fail() {
        let r = verdict(false, 0, 0);
        assert!(!r.ok);
    }
}
