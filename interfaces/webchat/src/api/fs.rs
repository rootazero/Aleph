//! `fs.*` Gateway RPC client.
//!
//! Backs the cross-platform `<DirectoryBrowser />` component (Panel) so
//! the directory picker browses **the server's** filesystem regardless of
//! where the client runs — local Tauri webview, localhost web tab, or
//! remote Tailnet web. See `gateway::handlers::fs` on the server for the
//! safety surface (`projects.allowed_roots`).

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

/// A configured browseable root surfaced to the picker UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedRoot {
    /// The original user-facing literal from `config.toml` (`~`,
    /// `/Volumes/Workspace`, …). Use this for breadcrumbs/labels — never
    /// for filesystem ops; pass `path` to `list_dir`/`create_dir` instead.
    pub label: String,
    /// The canonical, absolute, symlink-resolved path the server returned.
    pub path: String,
}

/// One row in `fs.list_dir`'s response. Files come back too (so the
/// browser can show them as disabled context); only `is_dir == true`
/// entries are clickable as targets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// `fs.read_file` response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadFileResult {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}

/// `fs.list_dir` response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListDirResult {
    /// Canonical path the server actually read (echo of input after
    /// canonicalisation — symlinks resolved, etc.).
    pub path: String,
    /// Parent directory, **only** when the parent is itself inside an
    /// allowed root. `None` means "you are at a root, can't go up."
    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,
}

pub struct FsApi;

impl FsApi {
    /// Return the configured browseable roots. UI uses this as the
    /// starting list when no path has been picked yet.
    pub async fn allowed_roots(state: &DashboardState) -> Result<Vec<AllowedRoot>, String> {
        let result = state
            .rpc_call("fs.allowed_roots", serde_json::json!({}))
            .await?;
        let arr = result
            .get("roots")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(arr).map_err(|e| e.to_string())
    }

    /// Server's $HOME — used as the default initial path when the user
    /// has no other context (e.g. first-time picker open).
    pub async fn home_dir(state: &DashboardState) -> Result<String, String> {
        let result = state.rpc_call("fs.home_dir", serde_json::json!({})).await?;
        result
            .get("path")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .ok_or_else(|| "missing path in response".to_string())
    }

    /// List entries at `path`. `show_hidden` includes dotfiles (off by
    /// default — matches macOS Finder + most directory pickers).
    pub async fn list_dir(
        state: &DashboardState,
        path: &str,
        show_hidden: bool,
    ) -> Result<ListDirResult, String> {
        let result = state
            .rpc_call(
                "fs.list_dir",
                serde_json::json!({ "path": path, "show_hidden": show_hidden }),
            )
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Read a file's text content (server-side, scoped to `allowed_roots`).
    pub async fn read_file(state: &DashboardState, path: &str) -> Result<ReadFileResult, String> {
        let result = state
            .rpc_call("fs.read_file", serde_json::json!({ "path": path }))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Create a fresh subdirectory `<parent>/<name>` and return its
    /// canonical absolute path. Server enforces: `parent` in scope, name
    /// has no separators / not '.'/'..', target doesn't already exist.
    pub async fn create_dir(
        state: &DashboardState,
        parent: &str,
        name: &str,
    ) -> Result<String, String> {
        let result = state
            .rpc_call(
                "fs.create_dir",
                serde_json::json!({ "parent": parent, "name": name }),
            )
            .await?;
        result
            .get("path")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .ok_or_else(|| "missing path in response".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_result_round_trips() {
        let v = serde_json::json!({ "path": "/a", "content": "x", "truncated": false });
        let r: ReadFileResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.path, "/a");
        assert_eq!(r.content, "x");
        assert!(!r.truncated);
    }
}
