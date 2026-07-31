// Browser cookies tool — list, get, set, delete, or clear cookies in the
// managed browser session. Maps a flat, LLM-friendly argument set onto the
// typed `CookieOp` backend contract.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::browser::types::{CookieOp, SameSite};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Which cookie operation to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CookieAction {
    /// List all cookies (optionally filtered by `domain` / `path`).
    List,
    /// Get a single cookie by `name`.
    Get,
    /// Set a cookie (`name` + `value` required; other fields optional).
    Set,
    /// Delete a single cookie by `name`.
    Delete,
    /// Clear all cookies.
    Clear,
}

/// Arguments for the `browser_cookies` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserCookiesArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Cookie operation to perform.
    pub action: CookieAction,
    /// Cookie name — required for `get` / `delete`, and the key for `set`.
    #[serde(default)]
    pub name: Option<String>,
    /// Cookie value — required for `set`.
    #[serde(default)]
    pub value: Option<String>,
    /// Cookie domain — a filter for `list`, an attribute for `set`.
    #[serde(default)]
    pub domain: Option<String>,
    /// Cookie path — a filter for `list`, an attribute for `set`.
    #[serde(default)]
    pub path: Option<String>,
    /// Expiration as a unix timestamp (seconds) — `set` only.
    #[serde(default)]
    pub expires: Option<i64>,
    /// Mark the cookie `HttpOnly` — `set` only.
    #[serde(default)]
    pub http_only: Option<bool>,
    /// Mark the cookie Secure — `set` only.
    #[serde(default)]
    pub secure: Option<bool>,
    /// `SameSite` attribute — `set` only.
    #[serde(default)]
    pub same_site: Option<SameSite>,
}

impl BrowserCookiesArgs {
    /// Build the backend [`CookieOp`] from these arguments, validating that the
    /// fields required by the chosen action are present.
    fn to_op(&self) -> std::result::Result<CookieOp, String> {
        match self.action {
            CookieAction::List => Ok(CookieOp::List {
                domain: self.domain.clone(),
                path: self.path.clone(),
            }),
            CookieAction::Get => Ok(CookieOp::Get {
                name: self.require_name()?,
            }),
            CookieAction::Set => {
                let name = self.require_name()?;
                // Deliberately NOT scanned by `check_input_secret_block`: a cookie
                // value legitimately IS a credential (session id, auth token), so
                // the form-input secret gate would false-positive on its core use.
                let value = self
                    .value
                    .clone()
                    .filter(|v| !v.is_empty())
                    .ok_or("cookie 'set' requires a value")?;
                Ok(CookieOp::Set {
                    name,
                    value,
                    domain: self.domain.clone(),
                    path: self.path.clone(),
                    expires: self.expires,
                    http_only: self.http_only,
                    secure: self.secure,
                    same_site: self.same_site,
                })
            }
            CookieAction::Delete => Ok(CookieOp::Delete {
                name: self.require_name()?,
            }),
            CookieAction::Clear => Ok(CookieOp::Clear),
        }
    }

    fn require_name(&self) -> std::result::Result<String, String> {
        self.name
            .clone()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| format!("cookie '{:?}' requires a name", self.action))
    }
}

/// Output from the `browser_cookies` tool.
#[derive(Debug, Serialize)]
pub struct BrowserCookiesOutput {
    pub success: bool,
    /// Raw textual cookie listing (for `list` / `get`).
    pub cookies: Option<String>,
    pub message: Option<String>,
}

/// Lists, gets, sets, deletes, or clears cookies in the managed browser session.
#[derive(Clone)]
pub struct BrowserCookiesTool {
    manager: Arc<ProfileManager>,
}

impl BrowserCookiesTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserCookiesTool {
    const NAME: &'static str = "browser_cookies";
    const DESCRIPTION: &'static str =
        "List, get, set, delete, or clear cookies in the managed browser session";
    type Args = BrowserCookiesArgs;
    type Output = BrowserCookiesOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let op = match args.to_op() {
            Ok(op) => op,
            Err(e) => {
                return Ok(BrowserCookiesOutput {
                    success: false,
                    cookies: None,
                    message: Some(e),
                });
            }
        };
        let backend = match super::make_backend(&self.manager, &args.profile) {
            Ok(b) => b,
            Err(e) => {
                return Ok(BrowserCookiesOutput {
                    success: false,
                    cookies: None,
                    message: Some(e.to_string()),
                });
            }
        };
        match backend.cookies(&op).await {
            Ok(text) => {
                let is_read = matches!(args.action, CookieAction::List | CookieAction::Get);
                // Cookie listings are untrusted external content (site-controlled
                // names/values): route them through the same egress chokepoint as
                // page text / console / network so they inherit injection-boundary
                // fencing, credential redaction, and size bounding. Every other
                // browser content-read tool already does this via `redact_and_wrap`
                // (mod.rs); cookies were the lone bypass.
                Ok(BrowserCookiesOutput {
                    success: true,
                    cookies: is_read.then(|| super::redact_and_wrap(&self.manager, &text)),
                    message: Some(format!(
                        "Cookie {:?} succeeded in profile '{}'",
                        args.action, args.profile
                    )),
                })
            }
            Err(e) => Ok(BrowserCookiesOutput {
                success: false,
                cookies: None,
                message: Some(format!("Cookie {:?} failed: {e}", args.action)),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    fn args(action: CookieAction) -> BrowserCookiesArgs {
        BrowserCookiesArgs {
            profile: "default".into(),
            action,
            name: None,
            value: None,
            domain: None,
            path: None,
            expires: None,
            http_only: None,
            secure: None,
            same_site: None,
        }
    }

    #[test]
    fn test_to_op_set_requires_name_and_value() {
        // Missing name.
        assert!(args(CookieAction::Set).to_op().is_err());
        // Name but no value.
        let mut a = args(CookieAction::Set);
        a.name = Some("sid".into());
        let err = a.to_op().unwrap_err();
        assert!(err.contains("value"), "got: {err}");
        // Both present → ok.
        a.value = Some("abc".into());
        assert!(a.to_op().is_ok());
    }

    #[test]
    fn test_to_op_get_and_delete_require_name() {
        assert!(args(CookieAction::Get).to_op().is_err());
        assert!(args(CookieAction::Delete).to_op().is_err());
        // List and Clear need nothing.
        assert!(args(CookieAction::List).to_op().is_ok());
        assert!(args(CookieAction::Clear).to_op().is_ok());
    }

    #[tokio::test]
    async fn test_cookies_set_without_value_fails_before_backend() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserCookiesTool::new(manager);
        let mut a = args(CookieAction::Set);
        a.name = Some("sid".into());
        let result = tool.call(a).await.unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("value"));
    }

    #[tokio::test]
    async fn test_cookies_list_degrades_without_browser() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserCookiesTool::new(manager);
        let result = tool.call(args(CookieAction::List)).await.unwrap();
        // Reaches the backend, which fails gracefully without a running browser.
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}
