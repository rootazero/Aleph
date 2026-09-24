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
    /// List the profiles available to you.
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
                // The caller's view (r11 N5): configured names only, with the
                // liveness of the caller's own copy. The person is the same
                // derivation every other browser tool resolves a profile with.
                let principal = match super::caller_browser_principal() {
                    Ok(principal) => principal,
                    Err(e) => {
                        return Ok(BrowserProfileOutput {
                            success: false,
                            profiles: None,
                            state: None,
                            message: Some(e.to_string()),
                        });
                    }
                };
                let profiles = self
                    .manager
                    .list_profiles_for(principal.as_deref())
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
                let key = match super::resolve_caller_profile(&self.manager, &name) {
                    Ok(key) => key,
                    Err(e) => {
                        return Ok(BrowserProfileOutput {
                            success: false,
                            profiles: None,
                            state: None,
                            message: Some(e.to_string()),
                        });
                    }
                };
                let active = self.manager.session_active(&key);
                let driver = self
                    .manager
                    .get_driver(&key)
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
        let tool = BrowserProfileTool::new(Arc::clone(&manager));

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
        //
        // The driver word is DERIVED from the manager rather than spelled here:
        // this test is about the `idle (driver: …)` shape, not about which
        // driver a fresh install gets, and a literal made it go red at the
        // dual-engine default flip for a reason that has nothing to do with
        // what it asserts (判据 §1 — one fact, one author).
        let driver = manager.get_driver("default").expect("default profile");
        assert_eq!(
            result.state.as_deref(),
            Some(format!("idle (driver: {driver:?})").as_str())
        );
        // …and the shape really does carry a driver, so the assertion above
        // cannot be satisfied by two empty strings.
        assert_eq!(driver, crate::browser::profile::BrowserDriver::Cdp);
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

    #[tokio::test]
    async fn a_member_lists_configured_names_with_its_own_liveness() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        manager.principal_profile("default", Some("u-bob")).unwrap();
        let tool = BrowserProfileTool::new(Arc::clone(&manager));
        let alice = crate::scope::ScopeAttribution::personal("u-alice");
        let out = crate::scope::with_scope(Some(alice), async {
            tool.call(BrowserProfileArgs {
                action: ProfileAction::List,
            })
            .await
        })
        .await
        .unwrap();
        let names: Vec<String> = out.profiles.unwrap().into_iter().map(|p| p.name).collect();
        assert!(names.contains(&"default".to_string()), "{names:?}");
        assert!(names.iter().all(|n| !n.contains("__")), "{names:?}");
        assert!(!names.contains(&"user".to_string()), "{names:?}");
    }
}
