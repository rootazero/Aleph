// Browser session tool — persist and restore login/authentication state.
//
// Wraps `playwright-cli state-save` / `state-load`. A saved state file captures
// the managed browser context's cookies + localStorage (the authentication
// state), letting an agent log in once and reuse the session later without
// re-authenticating. State files live in a managed directory keyed by a safe
// name slug, so a caller can never write/read outside it.

use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Save or restore the browser's authentication/storage state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionAction {
    /// Capture the current cookies + localStorage to a named state file.
    Save,
    /// Restore cookies + localStorage from a previously-saved state file.
    Load,
}

/// Arguments for the `browser_session` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSessionArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Whether to save the current state or load a saved one.
    pub action: SessionAction,
    /// Name of the saved session (e.g. "github"). Stored under the managed
    /// browser state directory; must contain only letters, digits, '-', '_', '.'
    /// and may not start with '.' (no path separators or traversal).
    pub name: String,
}

/// Output from the `browser_session` tool.
#[derive(Debug, Serialize)]
pub struct BrowserSessionOutput {
    pub success: bool,
    /// Absolute path of the state file that was written or read.
    pub path: Option<String>,
    pub message: Option<String>,
}

/// Persists / restores browser login sessions via storage-state files.
#[derive(Clone)]
pub struct BrowserSessionTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserSessionTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate save/load behind the approval policy, classified as
    /// [`ActionType::BrowserSessionState`].
    ///
    /// `browser_cookies set` is gated on the stated ground that "a cookie value
    /// is a credential by design" — and this tool moves EVERY cookie plus
    /// localStorage in one call: `save` writes the whole authenticated identity
    /// to a file on disk, `load` installs someone's whole authenticated
    /// identity into the live browser. Leaving the bulk operation ungated while
    /// the single-cookie one asks made the gate trivially avoidable, so both
    /// route through the same policy key. A dedicated `BrowserSession` variant
    /// would read better in a policy file but lives in `src/approval/types.rs`.
    ///
    /// With no policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

/// Reject empty names, leading dots, and any character outside `[A-Za-z0-9._-]`
/// so a caller can never escape the managed directory.
///
/// Split out from [`resolve_session_path`] because the two run at different
/// points: the name is a pure check that must precede the approval gate (a
/// malformed name is a model mistake and must not consume a user approval),
/// while resolving the path CREATES the sessions directory — a side effect a
/// denied call must not leave behind.
fn validate_session_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("session name must not be empty".into());
    }
    if name.starts_with('.')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!(
            "invalid session name '{name}': use only letters, digits, '-', '_', '.' \
             and do not start with '.' (no path separators)"
        ));
    }
    Ok(())
}

/// Resolve a validated session name to an absolute path under the managed
/// browser state directory (`~/.aleph/data/browser/sessions/<name>.json`),
/// creating the directory.
async fn resolve_session_path(name: &str) -> std::result::Result<PathBuf, String> {
    validate_session_name(name)?;
    let dir = crate::discovery::aleph_home_dir()
        .map_err(|e| format!("cannot resolve aleph home: {e}"))?
        .join("data")
        .join("browser")
        .join("sessions");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("cannot create session dir: {e}"))?;
    Ok(dir.join(format!("{name}.json")))
}

#[async_trait]
impl AlephTool for BrowserSessionTool {
    const NAME: &'static str = "browser_session";
    // One-sided capability — `BrowserBackend::{save_state,load_state}` are
    // served only by the managed Playwright backend; see `pdf.rs`.
    const DESCRIPTION: &'static str =
        "Save or restore a browser login session (cookies + localStorage) by name, \
         so a logged-in state can be reused without re-authenticating \
         — managed profiles only (e.g. profile='default')";
    type Args = BrowserSessionArgs;
    type Output = BrowserSessionOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if let Err(e) = validate_session_name(&args.name) {
            return Ok(BrowserSessionOutput {
                success: false,
                path: None,
                message: Some(e),
            });
        }

        // Gate AFTER name validation (a malformed name must not consume an
        // approval) and BEFORE both the sessions directory is created and the
        // backend is constructed, so a denied call leaves nothing behind. The
        // audit target names the session and direction, never the state file's
        // contents.
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserSessionState,
            "session",
            &format!("{:?} auth state '{}'", args.action, args.name),
        )
        .await
        {
            return Ok(BrowserSessionOutput {
                success: false,
                path: None,
                message: Some(message),
            });
        }

        let path = match resolve_session_path(&args.name).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(BrowserSessionOutput {
                    success: false,
                    path: None,
                    message: Some(e),
                });
            }
        };
        let path_str = path.to_string_lossy().to_string();

        let backend = match super::make_backend(&self.manager, &args.profile) {
            Ok(b) => b,
            Err(e) => {
                return Ok(BrowserSessionOutput {
                    success: false,
                    path: None,
                    message: Some(super::backend_error_text(&self.manager, &e)),
                });
            }
        };
        let result = match args.action {
            SessionAction::Save => backend.save_state(&path).await,
            SessionAction::Load => backend.load_state(&path).await,
        };
        match result {
            Ok(()) => Ok(BrowserSessionOutput {
                success: true,
                path: Some(path_str.clone()),
                message: Some(match args.action {
                    SessionAction::Save => {
                        format!("Saved session '{}' to {}", args.name, path_str)
                    }
                    SessionAction::Load => {
                        format!("Loaded session '{}' from {}", args.name, path_str)
                    }
                }),
            }),
            Err(e) => Ok(BrowserSessionOutput {
                success: false,
                path: None,
                message: Some(format!(
                    "Session {:?} failed: {}",
                    args.action,
                    super::backend_error_text(&self.manager, &e)
                )),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_resolve_session_path_rejects_traversal() {
        assert!(resolve_session_path("").await.is_err());
        assert!(resolve_session_path("../etc/passwd").await.is_err());
        assert!(resolve_session_path("a/b").await.is_err());
        assert!(resolve_session_path("a\\b").await.is_err());
        assert!(resolve_session_path(".hidden").await.is_err());
        assert!(resolve_session_path("..").await.is_err());
        // Valid slugs resolve under the managed sessions directory.
        let p = resolve_session_path("github").await.unwrap();
        assert!(p.ends_with("browser/sessions/github.json"));
        assert!(resolve_session_path("my_site-1").await.is_ok());
    }

    #[tokio::test]
    async fn test_session_save_degrades_without_browser() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager);
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Save,
                name: "unit-test".into(),
            })
            .await
            .unwrap();
        // Without a running browser the save fails gracefully.
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    fn deny_policy() -> Arc<crate::approval::ConfigApprovalPolicy> {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        let mut defaults = std::collections::HashMap::new();
        defaults.insert(ActionType::BrowserSessionState, DefaultDecision::Deny);
        Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }))
    }

    #[tokio::test]
    async fn test_session_save_is_gated_before_the_backend() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager).with_approval_policy(deny_policy());
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Save,
                name: "github".into(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        // The denial — not a "no browser running" error — proves the gate ran
        // before the backend was constructed. And nothing was written.
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("denied by approval policy")),
            "got: {:?}",
            result.message
        );
        assert!(result.path.is_none());
    }

    #[tokio::test]
    async fn test_session_load_is_gated_before_the_backend() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager).with_approval_policy(deny_policy());
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Load,
                name: "github".into(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("denied by approval policy")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_session_bad_name_does_not_consume_approval() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager).with_approval_policy(deny_policy());
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Load,
                name: "../evil".into(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(message.contains("invalid session name"), "got: {message}");
        assert!(!message.contains("denied"), "got: {message}");
    }

    #[tokio::test]
    async fn test_session_rejects_bad_name_before_backend() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager);
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Load,
                name: "../evil".into(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("invalid session name"));
    }
}
