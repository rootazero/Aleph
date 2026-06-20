//! `hub_install_verify` — post-install verifier tool (§10, T8).
//!
//! Thin wrapper over `crate::hub::verify::verify_install`. The tool
//! reconstructs an `InstallOutcome` from `{ kind, id_or_path }` args and
//! delegates all verification logic to the T2 backend.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::hub::install::InstallOutcome;
use crate::hub::verify::{verify_install, VerifyReport};
use crate::mcp::manager::McpManagerHandle;
use crate::tools::AlephTool;

// --------------------------------------------------------------------------
// Args
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HubInstallVerifyArgs {
    /// `"mcp"` or `"plugin"`.
    pub kind: String,
    /// MCP server id (when `kind == "mcp"`) or plugin path on disk
    /// (when `kind == "plugin"`).
    pub id_or_path: String,
}

impl HubInstallVerifyArgs {
    /// Map args to an `InstallOutcome`. This is the pure, tested core.
    pub fn to_outcome(&self) -> Result<InstallOutcome> {
        match self.kind.as_str() {
            "mcp" => Ok(InstallOutcome::Mcp {
                id: self.id_or_path.clone(),
            }),
            "plugin" => Ok(InstallOutcome::Plugin {
                path: self.id_or_path.clone(),
            }),
            other => Err(AlephError::tool(format!(
                "unknown kind '{other}'; expected \"mcp\" or \"plugin\""
            ))),
        }
    }
}

// --------------------------------------------------------------------------
// Tool
// --------------------------------------------------------------------------

#[derive(Clone)]
pub struct HubInstallVerifyTool {
    /// Optional live MCP manager handle. `None` → MCP verification reports
    /// "MCP manager unavailable"; plugin verification still works.
    pub mcp: Option<McpManagerHandle>,
}

#[async_trait]
impl AlephTool for HubInstallVerifyTool {
    const NAME: &'static str = "hub_install_verify";
    const DESCRIPTION: &'static str = "Verify that a just-installed extension is healthy. \
         For MCP servers: checks the server is running and exposes ≥1 tool. \
         For plugins: checks the artifact is present on disk.";
    type Args = HubInstallVerifyArgs;
    type Output = VerifyReport;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let outcome = args.to_outcome()?;
        Ok(verify_install(&outcome, self.mcp.as_ref()).await)
    }
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_plugin_outcome_args() {
        let a = HubInstallVerifyArgs {
            kind: "plugin".into(),
            id_or_path: "/tmp/x".into(),
        };
        let outcome = a.to_outcome().unwrap();
        assert!(matches!(
            outcome,
            crate::hub::install::InstallOutcome::Plugin { .. }
        ));
    }

    #[test]
    fn maps_mcp_outcome_args() {
        let a = HubInstallVerifyArgs {
            kind: "mcp".into(),
            id_or_path: "my_server".into(),
        };
        let outcome = a.to_outcome().unwrap();
        assert!(matches!(
            outcome,
            crate::hub::install::InstallOutcome::Mcp { .. }
        ));
    }

    #[test]
    fn unknown_kind_is_error() {
        let a = HubInstallVerifyArgs {
            kind: "oci".into(),
            id_or_path: "irrelevant".into(),
        };
        assert!(a.to_outcome().is_err());
    }
}
