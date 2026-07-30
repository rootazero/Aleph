// Browser profile tool — list and manage browser profiles.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Information about a browser profile.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProfileInfo {
    /// Profile name.
    pub name: String,
    /// Derived liveness: "active" (live session) or "idle".
    pub state: String,
    /// Driver mode: "managed" (headless, default) or "`existing_session`" (visible browser, use only when user explicitly requests).
    pub driver: String,
}

/// Action to perform on browser profiles.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAction {
    /// List all available profiles.
    List,
    /// Get the liveness and driver of a specific profile.
    GetState {
        /// Profile name to query.
        name: String,
    },
}

/// Arguments for the `browser_profile` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserProfileArgs {
    /// Profile action to perform.
    pub action: ProfileAction,
}

/// Output from the `browser_profile` tool.
#[derive(Debug, Serialize)]
pub struct BrowserProfileOutput {
    pub success: bool,
    pub profiles: Option<Vec<ProfileInfo>>,
    pub state: Option<String>,
    pub message: Option<String>,
}

/// Lists and manages browser profiles.
#[derive(Clone)]
pub struct BrowserProfileTool {
    manager: Arc<ProfileManager>,
}

impl BrowserProfileTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserProfileTool {
    const NAME: &'static str = "browser_profile";
    const DESCRIPTION: &'static str = "List and manage browser profiles";
    type Args = BrowserProfileArgs;
    type Output = BrowserProfileOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            ProfileAction::List => {
                let profiles = self
                    .manager
                    .list_profiles()
                    .into_iter()
                    .map(|(name, active)| {
                        let driver = self
                            .manager
                            .get_driver(&name)
                            .map_or_else(|| "unknown".to_string(), |d| format!("{d:?}"));
                        ProfileInfo {
                            name,
                            state: if active { "active" } else { "idle" }.to_string(),
                            driver,
                        }
                    })
                    .collect::<Vec<_>>();

                Ok(BrowserProfileOutput {
                    success: true,
                    profiles: Some(profiles),
                    state: None,
                    message: None,
                })
            }
            ProfileAction::GetState { name } => {
                if self.manager.get_config(&name).is_none() {
                    return Ok(BrowserProfileOutput {
                        success: false,
                        profiles: None,
                        state: None,
                        message: Some(format!("Profile '{name}' not found")),
                    });
                }
                let active = self.manager.session_active(&name);
                let driver = self
                    .manager
                    .get_driver(&name)
                    .map_or_else(|| "unknown".to_string(), |d| format!("{d:?}"));
                Ok(BrowserProfileOutput {
                    success: true,
                    profiles: None,
                    state: Some(format!(
                        "{} (driver: {driver})",
                        if active { "active" } else { "idle" }
                    )),
                    message: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_profile_list() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserProfileTool::new(manager);

        let result = tool
            .call(BrowserProfileArgs {
                action: ProfileAction::List,
            })
            .await
            .unwrap();

        assert!(result.success);
        let profiles = result.profiles.unwrap();
        assert!(!profiles.is_empty());
        assert!(profiles.iter().any(|p| p.name == "default"));
    }

    #[tokio::test]
    async fn test_profile_get_state_existing() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserProfileTool::new(manager);

        let result = tool
            .call(BrowserProfileArgs {
                action: ProfileAction::GetState {
                    name: "default".into(),
                },
            })
            .await
            .unwrap();

        assert!(result.success);
        // "default" has never been used → idle (with driver info attached).
        assert_eq!(result.state.as_deref(), Some("idle (driver: Managed)"));
    }

    #[tokio::test]
    async fn test_profile_get_state_missing() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserProfileTool::new(manager);

        let result = tool
            .call(BrowserProfileArgs {
                action: ProfileAction::GetState {
                    name: "nonexistent".into(),
                },
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.message.unwrap().contains("not found"));
    }
}
