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
}

impl BrowserSessionTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

/// Validate a session name and resolve it to an absolute path under the managed
/// browser state directory (`~/.aleph/data/browser/sessions/<name>.json`),
/// creating the directory. Rejects empty names, leading dots, and any character
/// outside `[A-Za-z0-9._-]` so a caller can never escape the managed directory.
async fn resolve_session_path(name: &str) -> std::result::Result<PathBuf, String> {
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
    const DESCRIPTION: &'static str =
        "Save or restore a browser login session (cookies + localStorage) by name, \
         so a logged-in state can be reused without re-authenticating";
    type Args = BrowserSessionArgs;
    type Output = BrowserSessionOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
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
                    message: Some(e.to_string()),
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
                message: Some(format!("Session {:?} failed: {e}", args.action)),
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
