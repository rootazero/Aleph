//! Post-install verification (spec §10). MCP: started + lists ≥1 tool.
//! Plugin: artifact present on disk. Honest report — never silent "success".
use crate::hub::install::InstallOutcome;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub detail: String,
}

/// Pure verdict from an MCP server's observed state.
#[must_use]
pub fn verdict(running: bool, tool_count: usize) -> VerifyReport {
    if !running {
        VerifyReport {
            ok: false,
            detail: "server not running".into(),
        }
    } else if tool_count == 0 {
        VerifyReport {
            ok: false,
            detail: "running but exposes 0 tools".into(),
        }
    } else {
        VerifyReport {
            ok: true,
            detail: format!("running; {tool_count} tools"),
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
                        let running =
                            matches!(info.health, crate::mcp::manager::HealthStatus::Healthy);
                        verdict(running, info.tool_count)
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_with_tools_is_ok() {
        let r = verdict(true, 3);
        assert!(r.ok);
        assert!(r.detail.contains('3'));
    }

    #[test]
    fn running_without_tools_is_warn() {
        let r = verdict(true, 0);
        assert!(!r.ok);
    }

    #[test]
    fn not_running_is_fail() {
        let r = verdict(false, 0);
        assert!(!r.ok);
    }
}
