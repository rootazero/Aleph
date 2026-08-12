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
pub(crate) const MIN_TIMEOUT_MS: u64 = 500;
pub(crate) const MAX_TIMEOUT_MS: u64 = 120_000;

/// Clamp a model-supplied wait timeout to the safe `[MIN, MAX]` window.
/// Hand-rolled rather than `Ord::clamp` so it can stay a `const fn`
/// (`Ord::clamp` is not const).
#[allow(clippy::manual_clamp)]
pub(crate) const fn clamp_timeout(ms: u64) -> u64 {
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
    /// Text to wait for DISAPPEARING from the page (e.g. a spinner or
    /// "Loading…" label). Inverse polarity of `text`.
    #[serde(default)]
    pub text_gone: Option<String>,
    /// CSS selector to wait for (at least one matching element).
    #[serde(default)]
    pub selector: Option<String>,
    /// Substring to wait for in the tab's current URL.
    #[serde(default)]
    pub url_contains: Option<String>,
    /// Fixed delay in milliseconds — for animations and debounced renders
    /// that expose no observable condition. Clamped to 500–120000.
    #[serde(default)]
    pub time_ms: Option<u64>,
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
/// `text` / `text_gone` / `selector` / `url_contains` / `time_ms` must be set —
/// the conditions are mutually exclusive because the backends poll them with
/// different probes and a combined "any-of" semantic would be ambiguous in the
/// result message.
fn resolve_condition(args: &BrowserWaitForArgs) -> std::result::Result<WaitCondition, String> {
    let set = [
        args.text.as_ref().map(|t| ("text", t)),
        args.text_gone.as_ref().map(|t| ("text_gone", t)),
        args.selector.as_ref().map(|s| ("selector", s)),
        args.url_contains.as_ref().map(|u| ("url_contains", u)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    match (set.as_slice(), args.time_ms) {
        ([("text", t)], None) => Ok(WaitCondition::Text((*t).clone())),
        ([("text_gone", t)], None) => Ok(WaitCondition::TextGone((*t).clone())),
        ([("selector", s)], None) => Ok(WaitCondition::Selector((*s).clone())),
        ([("url_contains", u)], None) => Ok(WaitCondition::UrlContains((*u).clone())),
        // A bare delay is itself the condition; clamp it into the same safe
        // window as a polling timeout so `u64::MAX` cannot pin a tab.
        ([], Some(ms)) => Ok(WaitCondition::Time(clamp_timeout(ms))),
        ([], None) => Err(
            "exactly one wait condition is required: set one of 'text', 'text_gone', \
             'selector', 'url_contains' or 'time_ms'"
                .into(),
        ),
        _ => Err(
            "wait conditions are mutually exclusive: set exactly one of 'text', 'text_gone', \
             'selector', 'url_contains' or 'time_ms', not several"
                .into(),
        ),
    }
}

/// Human-readable "what was waited on" for the output message.
fn describe_condition(condition: &WaitCondition) -> String {
    match condition {
        WaitCondition::Text(t) => format!("Text '{t}'"),
        WaitCondition::TextGone(t) => format!("Text gone '{t}'"),
        WaitCondition::Selector(s) => format!("Selector '{s}'"),
        WaitCondition::UrlContains(u) => format!("URL containing '{u}'"),
        WaitCondition::Time(ms) => format!("Delay {ms}ms"),
    }
}

#[async_trait]
impl AlephTool for BrowserWaitForTool {
    const NAME: &'static str = "browser_wait_for";
    const DESCRIPTION: &'static str =
        "Wait for a condition on the page (useful after navigation or actions): text appearing \
         or disappearing, a CSS selector matching an element, the URL containing a substring, \
         or a fixed delay in milliseconds. \
         Set exactly one of text / text_gone / selector / url_contains / time_ms.";
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
        // The GUARDED resolver: every wait except a bare delay polls the page
        // through `backend.evaluate` JS probes (see `browser::wait_probe`), so
        // this is a content read and owes the same read-time SSRF re-check
        // every other content read performs. Navigation-time guards only vet
        // the URL navigated *to*; a redirect or a JS `location` change can put
        // a forbidden internal origin under the probe afterwards.
        match super::make_backend_and_tab_guarded(&self.manager, &args.profile).await {
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
                        message: Some(format!(
                            "Wait failed: {}",
                            super::backend_error_text(&self.manager, &e)
                        )),
                    }),
                }
            }
            Err(e) => Ok(BrowserWaitForOutput {
                success: false,
                found: false,
                message: Some(super::backend_error_text(&self.manager, &e)),
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
        text_gone: Option<&str>,
        selector: Option<&str>,
        url_contains: Option<&str>,
        time_ms: Option<u64>,
    ) -> BrowserWaitForArgs {
        BrowserWaitForArgs {
            profile: "default".into(),
            text: text.map(str::to_string),
            text_gone: text_gone.map(str::to_string),
            selector: selector.map(str::to_string),
            url_contains: url_contains.map(str::to_string),
            time_ms,
            timeout_ms: 1000,
        }
    }

    /// `browser_wait_for` reads the page (its probes run through
    /// `backend.evaluate`), so it must resolve its tab through the GUARDED
    /// helper — the one that re-checks the tab's CURRENT url against the SSRF
    /// policy. It used the unguarded helper, which vets only the URL that was
    /// navigated *to*, so a redirect or a JS `location` change could park a
    /// forbidden internal origin under the probe.
    ///
    /// Source-level because there is no seam to inject a fake backend into a
    /// `ProfileManager`; what is being asserted is "which helper does this call
    /// site name", which no runtime observation can answer.
    ///
    /// CRLF-safe: `\r` is stripped before splitting and the split token is not
    /// anchored to a line boundary — on a Windows checkout an anchored `"\n…"`
    /// token matches nothing and the guard silently scans its own test module
    /// instead of the production code (CLAUDE.md §10).
    #[test]
    fn wait_for_resolves_its_tab_through_the_guarded_helper() {
        let src = include_str!("wait_for.rs").replace('\r', "");
        let prod = src.split("#[cfg(test)]").next().unwrap_or(&src).to_string();
        assert!(
            prod.contains("make_backend_and_tab_guarded(&self.manager"),
            "the production half of wait_for.rs no longer resolves through the guarded helper"
        );
        assert!(
            !prod.contains("make_backend_and_tab(&self.manager"),
            "wait_for.rs resolves through the UNGUARDED helper: page reads skip the \
             read-time SSRF re-check"
        );
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
            resolve_condition(&args(Some("Loading"), None, None, None, None)).unwrap(),
            WaitCondition::Text("Loading".into())
        );
        assert_eq!(
            resolve_condition(&args(None, Some("Loading"), None, None, None)).unwrap(),
            WaitCondition::TextGone("Loading".into())
        );
        assert_eq!(
            resolve_condition(&args(None, None, Some("#app .ready"), None, None)).unwrap(),
            WaitCondition::Selector("#app .ready".into())
        );
        assert_eq!(
            resolve_condition(&args(None, None, None, Some("/dashboard"), None)).unwrap(),
            WaitCondition::UrlContains("/dashboard".into())
        );
    }

    #[test]
    fn resolve_condition_maps_time_ms_to_a_clamped_delay() {
        assert_eq!(
            resolve_condition(&args(None, None, None, None, Some(2000))).unwrap(),
            WaitCondition::Time(2000)
        );
        // The delay shares the polling timeout's safe window.
        assert_eq!(
            resolve_condition(&args(None, None, None, None, Some(0))).unwrap(),
            WaitCondition::Time(MIN_TIMEOUT_MS)
        );
        assert_eq!(
            resolve_condition(&args(None, None, None, None, Some(u64::MAX))).unwrap(),
            WaitCondition::Time(MAX_TIMEOUT_MS)
        );
    }

    #[test]
    fn resolve_condition_rejects_zero_conditions() {
        let err = resolve_condition(&args(None, None, None, None, None)).unwrap_err();
        assert!(err.contains("exactly one"), "got: {err}");
        assert!(err.contains("time_ms"), "got: {err}");
    }

    #[test]
    fn resolve_condition_rejects_multiple_conditions() {
        let err = resolve_condition(&args(Some("t"), None, Some("s"), None, None)).unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
        let err =
            resolve_condition(&args(Some("t"), None, Some("s"), Some("u"), None)).unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
        // A delay combined with any polled condition is also several conditions.
        let mut mixed = args(None, None, None, None, Some(1000));
        mixed.text = Some("t".into());
        let err = resolve_condition(&mixed).unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
        let err = resolve_condition(&args(Some("t"), Some("g"), None, None, None)).unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
    }

    #[tokio::test]
    async fn test_wait_for() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserWaitForTool::new(manager);
        let result = tool
            .call(args(Some("Loading"), None, None, None, None))
            .await
            .unwrap();
        assert!(!result.success); // No browser running
    }

    #[tokio::test]
    async fn test_wait_for_zero_conditions_is_graceful_failure() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserWaitForTool::new(manager);
        // Validation fires before any backend lookup, so the message explains
        // the contract even with no browser running.
        let result = tool.call(args(None, None, None, None, None)).await.unwrap();
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
        let result = tool
            .call(args(Some("t"), None, Some("#x"), None, None))
            .await
            .unwrap();
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
