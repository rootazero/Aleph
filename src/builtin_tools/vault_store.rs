//! `VaultStoreTool` — manage encrypted secret vault

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::sync_primitives::Arc;

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::gateway::security::SharedTokenManager;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VaultStoreArgs {
    /// Action to perform
    #[schemars(
        description = "Action: 'store' to save a secret, 'delete' to remove one, 'list' to see all key names"
    )]
    pub action: VaultAction,
    /// Secret key name (e.g., "ai:openai", "gen:stability"). Required for store/delete.
    #[schemars(
        description = "Key name for the secret. Convention (exact prefixes the runtime reads): ai:{name} for LLM providers, gen:{name} for generation providers, embed:{id} for embeddings, channel:{instance_id}:{field} for channels"
    )]
    pub key: Option<String>,
    /// Secret value. Required for 'store' action only.
    #[schemars(description = "The secret value to store. Only used with 'store' action.")]
    pub secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VaultAction {
    Store,
    Delete,
    List,
}

#[derive(Debug, Serialize)]
pub struct VaultStoreOutput {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct VaultStoreTool {
    manager: Arc<SharedTokenManager>,
}

impl VaultStoreTool {
    pub const fn new(manager: Arc<SharedTokenManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for VaultStoreTool {
    const NAME: &'static str = "vault_store";
    const DESCRIPTION: &'static str = "Manage encrypted secret vault. API keys and sensitive credentials must be stored via this tool, never written directly to config files. Use 'store' to save, 'delete' to remove, 'list' to see key names (values are never returned).";

    type Args = VaultStoreArgs;
    type Output = VaultStoreOutput;

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &format!("{:?}", args.action));

        let result = match args.action {
            VaultAction::Store => {
                let key = args.key.as_deref().ok_or_else(|| {
                    crate::error::AlephError::tool("'key' is required for store action")
                })?;
                let secret = args.secret.as_deref().ok_or_else(|| {
                    crate::error::AlephError::tool("'secret' is required for store action")
                })?;
                self.manager
                    .store_secret(key, secret)
                    .map_err(|e| crate::error::AlephError::tool(e.to_string()))?;
                VaultStoreOutput {
                    success: true,
                    message: format!("Secret '{key}' stored successfully"),
                    keys: None,
                }
            }
            VaultAction::Delete => {
                let key = args.key.as_deref().ok_or_else(|| {
                    crate::error::AlephError::tool("'key' is required for delete action")
                })?;
                let deleted = self
                    .manager
                    .delete_secret(key)
                    .map_err(|e| crate::error::AlephError::tool(e.to_string()))?;
                VaultStoreOutput {
                    success: deleted,
                    message: if deleted {
                        format!("Secret '{key}' deleted")
                    } else {
                        format!("Secret '{key}' not found")
                    },
                    keys: None,
                }
            }
            VaultAction::List => {
                let names = self
                    .manager
                    .list_secret_names()
                    .map_err(|e| crate::error::AlephError::tool(e.to_string()))?;
                VaultStoreOutput {
                    success: true,
                    message: format!("{} secrets stored", names.len()),
                    keys: Some(names),
                }
            }
        };

        notify_tool_result(Self::NAME, &result.message, result.success);
        Ok(result)
    }
}
