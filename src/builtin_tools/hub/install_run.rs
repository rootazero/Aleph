//! `hub_install_run` — trust-gated, agent-driven extension install.
//!
//! SECURITY: this tool takes NO `ack` argument. The calling agent is an LLM; an
//! `ack` it controlled would let it fabricate user consent. The pure [`gate`]
//! is the system-enforced core: OCI is always rejected, any ack-required spec
//! bounces to the user (`NeedsUserConsent`) with ZERO install or secret-storage
//! side effects, and only clean (no-ack) specs proceed to a direct install. The
//! `ack=true` install branch lives ONLY in the `extensions.install` RPC path,
//! driven by a real user gesture — it is unreachable from this tool.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{AlephError, Result};
use crate::extension::marketplace::types::MarketplaceConfig;
use crate::extension::marketplace::MarketplaceManager;
use crate::gateway::security::SharedTokenManager;
use crate::hub::cache::{CatalogCache, CatalogFilter};
use crate::hub::install::{run_install, InstallContext, InstallOutcome};
use crate::hub::secrets::field_key;
use crate::hub::trust::{build_disclosure, DisclosurePayload};
use crate::hub::types::InstallSpec;
use crate::mcp::manager::McpManagerHandle;
use crate::tools::AlephTool;

// --------------------------------------------------------------------------
// The security core: the pure install gate.
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome {
    Reject,
    NeedsUserConsent,
    Proceed,
}

/// System-enforced install gate. OCI is always rejected. Any ack-required spec
/// bounces to the user (`NeedsUserConsent`) — the agent has NO way to satisfy
/// the ack, so it cannot self-approve a risky install. Only clean (no-ack)
/// specs proceed to a direct agent-driven install.
#[must_use]
pub fn gate(ack_required: bool, is_oci: bool) -> GateOutcome {
    if is_oci {
        return GateOutcome::Reject;
    }
    if ack_required {
        return GateOutcome::NeedsUserConsent;
    }
    GateOutcome::Proceed
}

/// Plugins (GitDir) write executable code to disk and carry prompt-injection
/// (InstructsAgent) risk, so the agent must never auto-install them — they
/// always require a user gesture via the trust-gated UI. MCP specs may
/// auto-install when their disclosure is not ack-required.
#[must_use]
pub fn requires_user_consent(ack_required: bool, spec: &crate::hub::types::InstallSpec) -> bool {
    ack_required || matches!(spec, crate::hub::types::InstallSpec::GitDir { .. })
}

// --------------------------------------------------------------------------
// Tool
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HubInstallRunArgs {
    /// The catalog entry id to install (e.g. "mcp-official:io.github.acme/foo").
    pub entry_id: String,
    /// Submitted config field values (env vars / headers). Secret fields are
    /// stored in the vault; plain fields flow into the install context.
    #[serde(default)]
    pub config_values: Map<String, Value>,
}

/// Three-way install result. `NeedsUserConsent` carries the disclosure the agent
/// surfaces so the user can complete the risky install through the Extensions UI's
/// ack flow; this tool performs no side effect in that case.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum InstallToolResult {
    NeedsUserConsent {
        disclosure: DisclosurePayload,
    },
    Installed {
        outcome: Value,
        /// Post-install health check, so the model never reports a bare success
        /// for an extension that landed on disk but did not come up. `ok: false`
        /// is not a failed install — the artifact is installed and the `detail`
        /// says what is wrong.
        verify: crate::hub::verify::VerifyReport,
    },
    Rejected {
        reason: String,
    },
}

fn outcome_json(o: &InstallOutcome) -> Value {
    match o {
        InstallOutcome::Mcp { id } => serde_json::json!({ "kind": "mcp", "id": id }),
        InstallOutcome::Plugin { path } => serde_json::json!({ "kind": "plugin", "path": path }),
        InstallOutcome::Skill { path } => {
            let mut out = serde_json::json!({ "kind": "skill", "path": path });
            // Declared automation (never auto-scheduled): the model reads
            // this notice, asks the user, and creates the cron job via
            // cron_manage on consent.
            if let Some(notice) =
                crate::skill::automation_notice(std::path::Path::new(path.as_str()))
            {
                out["automation"] = Value::String(notice);
            }
            out
        }
    }
}

/// Coerce a submitted JSON value to the string form the install pipeline stores.
fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[derive(Clone)]
pub struct HubInstallRunTool {
    pub cache: Arc<CatalogCache>,
    pub marketplaces: HashMap<String, MarketplaceConfig>,
    pub vault: Arc<SharedTokenManager>,
    /// Live MCP manager handle. `None` → MCP-spec installs return a graceful
    /// "MCP manager unavailable" error from `run_install`; plugin installs and
    /// secret storage still work.
    pub mcp: Option<McpManagerHandle>,
}

#[async_trait]
impl AlephTool for HubInstallRunTool {
    const NAME: &'static str = "hub_install_run";
    const DESCRIPTION: &'static str =
        "Install a catalog entry by id (trust-gated). Get the id from hub_catalog_search. Clean \
         specs install directly and come back with a post-install `verify` verdict; ack-required \
         specs and anything that writes code to disk (skills, plugins) bounce to the user for \
         consent via the Extensions UI without any side effect; OCI is rejected. \
         `hub_catalog_search` tells you in advance which of those an entry will be.";
    type Args = HubInstallRunArgs;
    type Output = InstallToolResult;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // (1) Load the catalog entry by id.
        let entries = self
            .cache
            .query(&CatalogFilter {
                id: Some(args.entry_id.clone()),
                ..Default::default()
            })
            .await
            .map_err(|e| AlephError::other(format!("catalog query failed: {e}")))?;
        let entry = entries.into_iter().next().ok_or_else(|| {
            AlephError::other(format!("entry '{}' not found in catalog", args.entry_id))
        })?;

        // (2) Resolve the install spec from the cached entry.
        let entry_id = args.entry_id.clone();
        let spec = entry
            .install_spec
            .clone()
            .ok_or_else(|| AlephError::other(format!("no install spec cached for {entry_id}")))?;

        // (3) Build the disclosure and run the system-enforced gate.
        let disclosure = build_disclosure(&entry, &spec);
        let is_oci = matches!(spec, InstallSpec::OciImage { .. });
        match gate(
            requires_user_consent(disclosure.ack_required, &spec),
            is_oci,
        ) {
            GateOutcome::Reject => Ok(InstallToolResult::Rejected {
                reason: if is_oci {
                    "OCI/Docker MCP containers are not installable in this version".to_string()
                } else {
                    "install rejected by trust gate".to_string()
                },
            }),
            // RETURN HERE — no install, no secret storage. The agent surfaces
            // the disclosure and directs the user to install via the Extensions UI.
            GateOutcome::NeedsUserConsent => Ok(InstallToolResult::NeedsUserConsent { disclosure }),
            GateOutcome::Proceed => {
                self.proceed(&entry, &spec, &disclosure, &args.config_values)
                    .await
            }
        }
    }
}

impl HubInstallRunTool {
    /// The clean-spec install path: split secrets from plain values, store
    /// secrets in the vault, then route the install. Reached ONLY for specs the
    /// gate cleared (no ack required, not OCI).
    async fn proceed(
        &self,
        entry: &crate::hub::types::ExtensionEntry,
        spec: &InstallSpec,
        disclosure: &DisclosurePayload,
        config_values: &Map<String, Value>,
    ) -> Result<InstallToolResult> {
        // Field name → declared `sensitive` flag, from the disclosure secrets.
        let secret_names: std::collections::HashSet<&str> = disclosure
            .secrets
            .iter()
            .filter(|s| s.sensitive)
            .map(|s| s.name.as_str())
            .collect();

        let mut secret_refs: HashMap<String, String> = HashMap::new();
        let mut plain_values: HashMap<String, String> = HashMap::new();
        for (name, raw) in config_values {
            let Some(val) = value_to_string(raw) else {
                continue;
            };
            if secret_names.contains(name.as_str()) {
                let key = field_key(entry.kind, &entry.id, name);
                self.vault.store_secret(&key, &val).map_err(|e| {
                    AlephError::other(format!("failed to store secret '{name}': {e}"))
                })?;
                secret_refs.insert(name.clone(), key);
            } else {
                plain_values.insert(name.clone(), val);
            }
        }

        // Build the marketplace manager from the same configs the gateway uses
        // (SHA256 verification + atomic copy happen inside install_to_scope).
        let marketplace = MarketplaceManager::new(self.marketplaces.clone(), None);
        let ctx = InstallContext {
            entry,
            mcp: self.mcp.as_ref(),
            marketplace: Some(&marketplace),
            secret_refs,
            plain_values,
        };
        let outcome = run_install(spec, &ctx)
            .await
            .map_err(|e| AlephError::other(format!("install failed: {e}")))?;
        // Same provenance row the RPC install path writes — an agent-driven
        // install must be exactly as traceable (and as update-checkable) as one
        // the user clicked through.
        crate::hub::origin::record_install(&self.cache, entry, spec, &outcome).await;
        let verify = crate::hub::verify::verify_install(&outcome, self.mcp.as_ref()).await;
        Ok(InstallToolResult::Installed {
            outcome: outcome_json(&outcome),
            verify,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::InstallSpec;

    // --- gate (existing 4 tests — unchanged) ---------------------------------

    #[test]
    fn oci_is_rejected() {
        assert_eq!(gate(false, true), GateOutcome::Reject);
    }
    #[test]
    fn oci_rejected_even_when_ack_required() {
        assert_eq!(gate(true, true), GateOutcome::Reject);
    }
    #[test]
    fn ack_required_bounces_to_user() {
        assert_eq!(gate(true, false), GateOutcome::NeedsUserConsent);
    }
    #[test]
    fn clean_spec_proceeds() {
        assert_eq!(gate(false, false), GateOutcome::Proceed);
    }

    // --- requires_user_consent -----------------------------------------------

    fn git_dir_spec() -> InstallSpec {
        InstallSpec::GitDir {
            git_url: "https://github.com/acme/plugin".into(),
            subdir: None,
            git_ref: None,
            sha256: None,
        }
    }

    fn mcp_stdio_spec() -> InstallSpec {
        InstallSpec::McpStdio {
            command: "npx".into(),
            args: vec![],
            env: vec![],
        }
    }

    fn mcp_remote_spec() -> InstallSpec {
        InstallSpec::McpRemote {
            url: "https://mcp.example.com".into(),
            transport: crate::hub::types::McpTransport::StreamableHttp,
            headers: vec![],
        }
    }

    /// GitDir (plugin) must always require user consent, even when not ack_required.
    #[test]
    fn git_dir_always_requires_consent() {
        assert!(requires_user_consent(false, &git_dir_spec()));
    }

    /// MCP spec with no ack_required must NOT require user consent.
    #[test]
    fn mcp_stdio_no_ack_does_not_require_consent() {
        assert!(!requires_user_consent(false, &mcp_stdio_spec()));
    }

    /// MCP remote spec with no ack_required must NOT require user consent.
    #[test]
    fn mcp_remote_no_ack_does_not_require_consent() {
        assert!(!requires_user_consent(false, &mcp_remote_spec()));
    }

    /// Any spec with ack_required=true must require user consent.
    #[test]
    fn ack_required_always_requires_consent_regardless_of_spec() {
        assert!(requires_user_consent(true, &mcp_stdio_spec()));
        assert!(requires_user_consent(true, &mcp_remote_spec()));
        assert!(requires_user_consent(true, &git_dir_spec()));
    }
}
