// Browser wait_for tool — waits for text to appear on the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

const fn default_timeout() -> u64 {
    5000
}

/// Clamp bounds for the model-supplied wait timeout (milliseconds).
///
/// A wait polls the page for text, so its budget must stay in a sane window:
/// too short starves a slow render; too long (e.g. `u64::MAX`) overflows the
/// polling backend's `Instant + Duration::from_millis(timeout_ms)` — a panic —
/// and would pin a tab for the agent's whole session. openclaw clamps the
/// equivalent act-wait to this same 0.5s–120s window (`resolveActWaitTimeoutMs`).
const MIN_TIMEOUT_MS: u64 = 500;
const MAX_TIMEOUT_MS: u64 = 120_000;

/// Clamp a model-supplied wait timeout to the safe `[MIN, MAX]` window.
/// Hand-rolled rather than `Ord::clamp` so it can stay a `const fn`
/// (`Ord::clamp` is not const).
#[allow(clippy::manual_clamp)]
const fn clamp_timeout(ms: u64) -> u64 {
    if ms < MIN_TIMEOUT_MS {
        MIN_TIMEOUT_MS
    } else if ms > MAX_TIMEOUT_MS {
        MAX_TIMEOUT_MS
    } else {
        ms
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserWaitForArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Text to wait for on the page.
    pub text: String,
    /// Timeout in milliseconds (default: 5000; clamped to 500–120000).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct BrowserWaitForOutput {
    pub success: bool,
    pub found: bool,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct BrowserWaitForTool {
    manager: Arc<ProfileManager>,
}

impl BrowserWaitForTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserWaitForTool {
    const NAME: &'static str = "browser_wait_for";
    const DESCRIPTION: &'static str =
        "Wait for specific text to appear on the page (useful after navigation or actions)";
    type Args = BrowserWaitForArgs;
    type Output = BrowserWaitForOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Clamp at the system boundary: an unbounded model-supplied timeout
        // would overflow the polling backend's `Instant + Duration` (panic) and
        // pin a tab indefinitely.
        let timeout_ms = clamp_timeout(args.timeout_ms);
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                match backend
                    .wait_for_text(&tab_id, &args.text, timeout_ms)
                    .await
                {
                    Ok(found) => Ok(BrowserWaitForOutput {
                        success: true,
                        found,
                        message: Some(format!(
                            "Text '{}' {}",
                            args.text,
                            if found { "found" } else { "not found" }
                        )),
                    }),
                    Err(e) => Ok(BrowserWaitForOutput {
                        success: false,
                        found: false,
                        message: Some(format!("Wait failed: {e}")),
                    }),
                }
            }
            Err(e) => Ok(BrowserWaitForOutput {
                success: false,
                found: false,
                message: Some(format!("{e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[test]
    fn timeout_is_clamped_to_safe_window() {
        // Below floor → floor; the panic-inducing u64::MAX → ceiling; in-range untouched.
        assert_eq!(clamp_timeout(0), MIN_TIMEOUT_MS);
        assert_eq!(clamp_timeout(u64::MAX), MAX_TIMEOUT_MS);
        assert_eq!(clamp_timeout(5000), 5000);
        assert_eq!(clamp_timeout(MAX_TIMEOUT_MS + 1), MAX_TIMEOUT_MS);
    }

    #[tokio::test]
    async fn test_wait_for() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserWaitForTool::new(manager);
        let result = tool
            .call(BrowserWaitForArgs {
                profile: "default".into(),
                text: "Loading".into(),
                timeout_ms: 1000,
            })
            .await
            .unwrap();
        assert!(!result.success); // No browser running
    }
}
