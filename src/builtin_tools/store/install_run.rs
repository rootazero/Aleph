//! `store_install_run` — trust-gated, agent-driven extension install.
//!
//! SECURITY: this tool takes NO `ack` argument. The store agent is an LLM; an
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
use crate::mcp::manager::McpManagerHandle;
use crate::store::cache::{CatalogCache, CatalogFilter};
use crate::store::install::{run_install, InstallContext, InstallOutcome};
use crate::store::provider::registry_builder::build_default_registry;
use crate::store::secrets::field_key;
use crate::store::trust::{build_disclosure, DisclosurePayload};
use crate::store::types::InstallSpec;
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

// --------------------------------------------------------------------------
// Tool
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StoreInstallRunArgs {
    /// The catalog entry id to install (e.g. "mcp-official:io.github.acme/foo").
    pub entry_id: String,
    /// Submitted config field values (env vars / headers). Secret fields are
    /// stored in the vault; plain fields flow into the install context.
    #[serde(default)]
    pub config_values: Map<String, Value>,
}

/// Three-way install result. `NeedsUserConsent` carries the disclosure the agent
/// surfaces so the user can complete the risky install through the store UI's
/// ack flow; this tool performs no side effect in that case.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum InstallToolResult {
    NeedsUserConsent { disclosure: DisclosurePayload },
    Installed { outcome: Value },
    Rejected { reason: String },
}

fn outcome_json(o: &InstallOutcome) -> Value {
    match o {
        InstallOutcome::Mcp { id } => serde_json::json!({ "kind": "mcp", "id": id }),
        InstallOutcome::Plugin { path } => serde_json::json!({ "kind": "plugin", "path": path }),
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
pub struct StoreInstallRunTool {
    pub cache: Arc<CatalogCache>,
    pub marketplaces: HashMap<String, MarketplaceConfig>,
    pub vault: Arc<SharedTokenManager>,
    /// Live MCP manager handle. `None` → MCP-spec installs return a graceful
    /// "MCP manager unavailable" error from `run_install`; plugin installs and
    /// secret storage still work.
    pub mcp: Option<McpManagerHandle>,
}

#[async_trait]
impl AlephTool for StoreInstallRunTool {
    const NAME: &'static str = "store_install_run";
    const DESCRIPTION: &'static str =
        "Install a catalog entry by id (trust-gated). Clean specs install directly; \
         ack-required specs bounce to the user for consent via the store UI; OCI is rejected.";
    type Args = StoreInstallRunArgs;
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

        // (2) Resolve the install spec via the matching source provider.
        let registry = build_default_registry(self.marketplaces.clone());
        let spec = registry
            .resolve_for_entry(&entry)
            .await
            .map_err(|e| AlephError::other(format!("resolve_for_entry failed: {e}")))?;

        // (3) Build the disclosure and run the system-enforced gate.
        let disclosure = build_disclosure(&entry, &spec);
        let is_oci = matches!(spec, InstallSpec::OciImage { .. });
        match gate(disclosure.ack_required, is_oci) {
            GateOutcome::Reject => Ok(InstallToolResult::Rejected {
                reason: if is_oci {
                    "OCI/Docker MCP containers are not installable in this version".to_string()
                } else {
                    "install rejected by trust gate".to_string()
                },
            }),
            // RETURN HERE — no install, no secret storage. The agent surfaces
            // the disclosure and directs the user to install via the store UI.
            GateOutcome::NeedsUserConsent => {
                Ok(InstallToolResult::NeedsUserConsent { disclosure })
            }
            GateOutcome::Proceed => {
                self.proceed(&entry, &spec, &disclosure, &args.config_values)
                    .await
            }
        }
    }
}

impl StoreInstallRunTool {
    /// The clean-spec install path: split secrets from plain values, store
    /// secrets in the vault, then route the install. Reached ONLY for specs the
    /// gate cleared (no ack required, not OCI).
    async fn proceed(
        &self,
        entry: &crate::store::types::ExtensionEntry,
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
        Ok(InstallToolResult::Installed {
            outcome: outcome_json(&outcome),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
