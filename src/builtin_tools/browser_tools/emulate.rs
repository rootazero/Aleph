// Browser emulate tool — apply environment/device overrides to a tab
// (color scheme, geolocation, network/CPU throttling, HTTP headers, user-agent).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::browser::types::EmulateOptions;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the browser_emulate tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserEmulateArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Emulation overrides to apply (only the set fields take effect).
    #[serde(flatten)]
    pub options: EmulateOptions,
}

/// Output from the browser_emulate tool.
#[derive(Debug, Serialize)]
pub struct BrowserEmulateOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Emulates color scheme, geolocation, network/CPU throttling, extra HTTP
/// headers, and user-agent on the active tab. Full support requires an
/// existing-session profile; the managed profile supports network state only.
#[derive(Clone)]
pub struct BrowserEmulateTool {
    manager: Arc<ProfileManager>,
}

impl BrowserEmulateTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserEmulateTool {
    const NAME: &'static str = "browser_emulate";
    const DESCRIPTION: &'static str =
        "Emulate environment overrides on the active tab: color scheme (dark/light), \
         geolocation, network condition (offline/3G/4G), CPU throttle, extra HTTP headers, \
         and user-agent";
    type Args = BrowserEmulateArgs;
    type Output = BrowserEmulateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate at the boundary before touching the browser.
        if let Err(reason) = args.options.validate() {
            return Ok(BrowserEmulateOutput {
                success: false,
                message: Some(reason),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.emulate(&tab_id, &args.options).await {
                Ok(()) => Ok(BrowserEmulateOutput {
                    success: true,
                    message: Some(format!("Emulation applied in profile '{}'", args.profile)),
                }),
                Err(e) => Ok(BrowserEmulateOutput {
                    success: false,
                    message: Some(format!("Emulate failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserEmulateOutput {
                success: false,
                message: Some(format!("{e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;
    use crate::browser::types::{ColorScheme, NetworkCondition};

    fn tool() -> BrowserEmulateTool {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        BrowserEmulateTool::new(manager)
    }

    #[tokio::test]
    async fn test_empty_options_is_rejected_without_browser() {
        let result = tool()
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: EmulateOptions::default(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("at least one option"));
    }

    #[tokio::test]
    async fn test_out_of_range_cpu_throttle_is_rejected() {
        let result = tool()
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: EmulateOptions {
                    cpu_throttle: Some(99.0),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("out of range"));
    }

    #[tokio::test]
    async fn test_valid_options_degrade_without_browser() {
        // Valid request, but no browser is running → graceful failure, not panic.
        let result = tool()
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: EmulateOptions {
                    color_scheme: Some(ColorScheme::Dark),
                    network_condition: Some(NetworkCondition::Offline),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[test]
    fn test_args_flatten_deserialization() {
        // Options flatten alongside `profile` in the JSON-RPC payload.
        let json = serde_json::json!({
            "profile": "user",
            "color_scheme": "dark",
            "geolocation": { "latitude": 37.77, "longitude": -122.41 },
            "network_condition": "fast3g"
        });
        let args: BrowserEmulateArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.profile, "user");
        assert!(matches!(args.options.color_scheme, Some(ColorScheme::Dark)));
        assert!(matches!(
            args.options.network_condition,
            Some(NetworkCondition::Fast3g)
        ));
        let geo = args.options.geolocation.unwrap();
        assert!((geo.latitude - 37.77).abs() < 1e-9);
    }
}
