// Browser wait_for tool — waits for a condition (text / selector / URL) on the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::browser::types::WaitCondition;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

const fn default_timeout() -> u64 {
    5000
}

/// Clamp bounds for the model-supplied wait timeout (milliseconds).
///
/// A wait polls the page for a condition, so its budget must stay in a sane
/// window: too short starves a slow render; too long (e.g. `u64::MAX`)
/// overflows the polling backend's `Instant + Duration::from_millis(timeout_ms)`
/// — a panic — and would pin a tab for the agent's whole session. openclaw
/// clamps the equivalent act-wait to this same 0.5s–120s window
/// (`resolveActWaitTimeoutMs`).
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
    #[serde(default)]
    pub text: Option<String>,
    /// CSS selector to wait for (at least one matching element).
    #[serde(default)]
    pub selector: Option<String>,
    /// Substring to wait for in the tab's current URL.
    #[serde(default)]
    pub url_contains: Option<String>,
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

/// Build the wait condition from the model-supplied args. Exactly one of
/// `text` / `selector` / `url_contains` must be set — the conditions are
/// mutually exclusive because the backends poll them with different probes
/// and a combined "any-of" semantic would be ambiguous in the result message.
fn resolve_condition(args: &BrowserWaitForArgs) -> std::result::Result<WaitCondition, String> {
    let set = [
        args.text.as_ref().map(|t| ("text", t)),
        args.selector.as_ref().map(|s| ("selector", s)),
        args.url_contains.as_ref().map(|u| ("url_contains", u)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    match set.as_slice() {
        [("text", t)] => Ok(WaitCondition::Text((*t).clone())),
        [("selector", s)] => Ok(WaitCondition::Selector((*s).clone())),
        [("url_contains", u)] => Ok(WaitCondition::UrlContains((*u).clone())),
        [] => Err(
            "exactly one wait condition is required: set one of 'text', 'selector' or \
             'url_contains'"
                .into(),
        ),
        _ => Err(
            "wait conditions are mutually exclusive: set exactly one of 'text', 'selector' or \
             'url_contains', not several"
                .into(),
        ),
    }
}

/// Human-readable "what was waited on" for the output message.
fn describe_condition(condition: &WaitCondition) -> String {
    match condition {
        WaitCondition::Text(t) => format!("Text '{t}'"),
        WaitCondition::Selector(s) => format!("Selector '{s}'"),
        WaitCondition::UrlContains(u) => format!("URL containing '{u}'"),
    }
}

#[async_trait]
impl AlephTool for BrowserWaitForTool {
    const NAME: &'static str = "browser_wait_for";
    const DESCRIPTION: &'static str =
        "Wait for a condition on the page (useful after navigation or actions): text appearing, \
         a CSS selector matching an element, or the URL containing a substring. \
         Set exactly one of text / selector / url_contains.";
    type Args = BrowserWaitForArgs;
    type Output = BrowserWaitForOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate before touching a backend: a malformed request degrades to
        // success:false with the contract spelled out, never a hard Err.
        let condition = match resolve_condition(&args) {
            Ok(c) => c,
            Err(message) => {
                return Ok(BrowserWaitForOutput {
                    success: false,
                    found: false,
                    message: Some(message),
                });
            }
        };
        // Clamp at the system boundary: an unbounded model-supplied timeout
        // would overflow the polling backend's `Instant + Duration` (panic) and
        // pin a tab indefinitely.
        let timeout_ms = clamp_timeout(args.timeout_ms);
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                match backend.wait_for(&tab_id, &condition, timeout_ms).await {
                    Ok(found) => Ok(BrowserWaitForOutput {
                        success: true,
                        found,
                        message: Some(format!(
                            "{} {}",
                            describe_condition(&condition),
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

    fn args(
        text: Option<&str>,
        selector: Option<&str>,
        url_contains: Option<&str>,
    ) -> BrowserWaitForArgs {
        BrowserWaitForArgs {
            profile: "default".into(),
            text: text.map(str::to_string),
            selector: selector.map(str::to_string),
            url_contains: url_contains.map(str::to_string),
            timeout_ms: 1000,
        }
    }

    #[test]
    fn timeout_is_clamped_to_safe_window() {
        // Below floor → floor; the panic-inducing u64::MAX → ceiling; in-range untouched.
        assert_eq!(clamp_timeout(0), MIN_TIMEOUT_MS);
        assert_eq!(clamp_timeout(u64::MAX), MAX_TIMEOUT_MS);
        assert_eq!(clamp_timeout(5000), 5000);
        assert_eq!(clamp_timeout(MAX_TIMEOUT_MS + 1), MAX_TIMEOUT_MS);
    }

    #[test]
    fn resolve_condition_maps_each_single_condition() {
        assert_eq!(
            resolve_condition(&args(Some("Loading"), None, None)).unwrap(),
            WaitCondition::Text("Loading".into())
        );
        assert_eq!(
            resolve_condition(&args(None, Some("#app .ready"), None)).unwrap(),
            WaitCondition::Selector("#app .ready".into())
        );
        assert_eq!(
            resolve_condition(&args(None, None, Some("/dashboard"))).unwrap(),
            WaitCondition::UrlContains("/dashboard".into())
        );
    }

    #[test]
    fn resolve_condition_rejects_zero_conditions() {
        let err = resolve_condition(&args(None, None, None)).unwrap_err();
        assert!(err.contains("exactly one"), "got: {err}");
    }

    #[test]
    fn resolve_condition_rejects_multiple_conditions() {
        let err = resolve_condition(&args(Some("t"), Some("s"), None)).unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
        let err = resolve_condition(&args(Some("t"), Some("s"), Some("u"))).unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
    }

    #[tokio::test]
    async fn test_wait_for() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserWaitForTool::new(manager);
        let result = tool.call(args(Some("Loading"), None, None)).await.unwrap();
        assert!(!result.success); // No browser running
    }

    #[tokio::test]
    async fn test_wait_for_zero_conditions_is_graceful_failure() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserWaitForTool::new(manager);
        // Validation fires before any backend lookup, so the message explains
        // the contract even with no browser running.
        let result = tool.call(args(None, None, None)).await.unwrap();
        assert!(!result.success);
        assert!(!result.found);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("exactly one")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_wait_for_two_conditions_is_graceful_failure() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserWaitForTool::new(manager);
        let result = tool.call(args(Some("t"), Some("#x"), None)).await.unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("mutually exclusive")),
            "got: {:?}",
            result.message
        );
    }
}
