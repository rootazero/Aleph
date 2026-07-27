//! Filesystem-browse RPCs for the cross-platform directory picker.
//!
//! These exist so every channel (Panel webview — local or remote-Tailnet,
//! future CLI prompt, future Bot) can pick a project directory the same
//! way: by browsing the **server's** filesystem through JSON-RPC, instead
//! of each channel reaching for its own native dialog. That makes:
//!
//! - Local web (`http://127.0.0.1:18790/`) — same flow as Tauri.
//! - Tauri shell webview — same flow as Local web (no more
//!   `alephShell.pickDirectory` cross-origin hole).
//! - Remote Tailnet web — actually correct: the user picks the directory
//!   the agent will run in, not a stray path from their laptop.
//!
//! Safety surface: every traversal/mutation is gated by
//! [`ProjectsConfig::allowed_roots`]. A path that doesn't canonicalise
//! into one of those roots' subtrees returns `-32600` and never touches
//! `fs::read_dir` / `fs::create_dir`.
//!
//! RPCs:
//! - `fs.allowed_roots`            → `{ roots: [{ label, path }] }`
//! - `fs.home_dir`                 → `{ path }`
//! - `fs.list_dir { path, show_hidden? }` → `{ path, parent?, entries: [{ name, path, is_dir }] }`
//! - `fs.create_dir { parent, name }`     → `{ path }`
//! - `fs.read_file { path }`               → `{ path, content, truncated }`
//!
//! All read paths are absolute, canonicalised, and stripped of symlinks
//! before being handed back to the client.

use crate::config::Config;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::sync_primitives::Arc;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

/// The handler-facing config handle. Mirrors what other hot-reload-aware
/// handlers (e.g. `config::handle_get`) consume, so a `config.reload`
/// that widens `allowed_roots` reflects immediately without a restart.
type SharedConfig = Arc<RwLock<Config>>;

const INVALID_PARAMS: i32 = -32602;
const OUT_OF_SCOPE: i32 = -32600;
const NOT_FOUND: i32 = -32004;
const INTERNAL_ERROR: i32 = -32603;

/// Maximum entries returned by `fs.list_dir`. Large directories (e.g.
/// `node_modules`) would otherwise drag the panel into a slow JSON parse
/// — at 5000 dirs the UX is already "you should narrow down first."
const LIST_DIR_CAP: usize = 5000;

/// Maximum bytes returned by `fs.read_file`. Beyond this the content is
/// truncated and `truncated: true` is set — the preview pane is for
/// reading, not for hauling multi-MB blobs over JSON-RPC.
const READ_FILE_CAP: usize = 512 * 1024;

/// Expand a single user-supplied root string into a real, canonical
/// directory. `~` and `~/foo` are resolved against `dirs::home_dir()`;
/// `$VAR` is intentionally NOT expanded (no shell-style mystery surface).
/// Returns `None` if the path can't be resolved or isn't a directory —
/// callers should warn-log and drop, not surface to clients.
fn resolve_root(spec: &str) -> Option<PathBuf> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded: PathBuf = if trimmed == "~" {
        dirs::home_dir()?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        dirs::home_dir()?.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    let canon = std::fs::canonicalize(&expanded).ok()?;
    if canon.is_dir() {
        Some(canon)
    } else {
        None
    }
}

/// Resolve every configured root once at handler-time. Bad entries are
/// dropped silently (caller has access to the same TOML so misconfig is
/// self-evident); an empty result is treated as "no scope, lock down."
fn resolve_roots(cfg: &Config) -> Vec<PathBuf> {
    cfg.projects
        .allowed_roots
        .iter()
        .filter_map(|s| resolve_root(s))
        .collect()
}

/// Check that `candidate` (already canonical) is inside at least one
/// allowed root. Returns the canonical path on success.
fn validate_in_scope(candidate: &Path, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let canon = std::fs::canonicalize(candidate)
        .map_err(|e| format!("cannot canonicalise '{}': {e}", candidate.display()))?;
    if roots.iter().any(|r| canon.starts_with(r)) {
        Ok(canon)
    } else {
        Err(format!(
            "path '{}' is outside the configured projects.allowed_roots",
            canon.display()
        ))
    }
}

// ─── fs.allowed_roots ────────────────────────────────────────────────────────

pub async fn handle_allowed_roots(
    request: JsonRpcRequest,
    config: SharedConfig,
) -> JsonRpcResponse {
    let cfg = config.read().await;
    let roots: Vec<serde_json::Value> = cfg
        .projects
        .allowed_roots
        .iter()
        .filter_map(|spec| {
            let canon = resolve_root(spec)?;
            Some(json!({
                "label": spec,                 // original user-facing literal (`~`, `/Volumes/...`)
                "path": canon.to_string_lossy(),
            }))
        })
        .collect();
    JsonRpcResponse::success(request.id, json!({ "roots": roots }))
}

// ─── fs.home_dir ─────────────────────────────────────────────────────────────

pub async fn handle_home_dir(request: JsonRpcRequest) -> JsonRpcResponse {
    match dirs::home_dir() {
        Some(home) => {
            JsonRpcResponse::success(request.id, json!({ "path": home.to_string_lossy() }))
        }
        None => JsonRpcResponse::error(request.id, INTERNAL_ERROR, "home directory unavailable"),
    }
}

// ─── fs.list_dir ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListDirParams {
    path: String,
    #[serde(default)]
    show_hidden: bool,
}

pub async fn handle_list_dir(request: JsonRpcRequest, config: SharedConfig) -> JsonRpcResponse {
    let params: ListDirParams = match request
        .params
        .clone()
        .map(serde_json::from_value)
        .transpose()
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "params required");
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            );
        }
    };

    let roots = resolve_roots(&*config.read().await);
    if roots.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            OUT_OF_SCOPE,
            "no allowed_roots configured — directory browsing is disabled",
        );
    }

    let candidate = PathBuf::from(&params.path);
    let canon = match validate_in_scope(&candidate, &roots) {
        Ok(p) => p,
        Err(msg) => return JsonRpcResponse::error(request.id, OUT_OF_SCOPE, &msg),
    };

    let mut read = match tokio::fs::read_dir(&canon).await {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return JsonRpcResponse::error(
                request.id,
                NOT_FOUND,
                format!("not found: {}", canon.display()),
            );
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("read_dir failed: {e}"),
            );
        }
    };

    let mut entries: Vec<serde_json::Value> = Vec::with_capacity(64);
    loop {
        let entry = match read.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("read_dir failed: {e}"),
                );
            }
        };
        let name_os = entry.file_name();
        let name = name_os.to_string_lossy().to_string();
        if !params.show_hidden && name.starts_with('.') {
            continue;
        }
        // `file_type()` follows symlinks; we deliberately resolve so the
        // displayed `is_dir` matches what the next `list_dir` would do.
        let is_dir = entry.file_type().await.is_ok_and(|t| t.is_dir());
        let full = entry.path();
        entries.push(json!({
            "name": name,
            "path": full.to_string_lossy(),
            "is_dir": is_dir,
        }));
        if entries.len() >= LIST_DIR_CAP {
            break;
        }
    }
    entries.sort_by(|a, b| {
        let an = a["name"].as_str().unwrap_or("");
        let bn = b["name"].as_str().unwrap_or("");
        let ad = a["is_dir"].as_bool().unwrap_or(false);
        let bd = b["is_dir"].as_bool().unwrap_or(false);
        bd.cmp(&ad)
            .then_with(|| an.to_lowercase().cmp(&bn.to_lowercase()))
    });

    // Parent is only exposed when it is *also* in scope. Crossing out of
    // an allowed_root would let the user "escape" by going up.
    let parent = canon
        .parent()
        .and_then(|p| {
            std::fs::canonicalize(p)
                .ok()
                .filter(|c| roots.iter().any(|r| c.starts_with(r)))
        })
        .map(|p| p.to_string_lossy().into_owned());

    JsonRpcResponse::success(
        request.id,
        json!({
            "path": canon.to_string_lossy(),
            "parent": parent,
            "entries": entries,
        }),
    )
}

// ─── fs.create_dir ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CreateDirParams {
    parent: String,
    name: String,
}

pub async fn handle_create_dir(request: JsonRpcRequest, config: SharedConfig) -> JsonRpcResponse {
    let params: CreateDirParams = match request
        .params
        .clone()
        .map(serde_json::from_value)
        .transpose()
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "params required");
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            );
        }
    };

    let trimmed = params.name.trim();
    if trimmed.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "name cannot be empty");
    }
    if trimmed.contains(std::path::MAIN_SEPARATOR)
        || trimmed.contains('/')
        || trimmed == "."
        || trimmed == ".."
    {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "name must not contain path separators or be '.'/'..'",
        );
    }

    let roots = resolve_roots(&*config.read().await);
    if roots.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            OUT_OF_SCOPE,
            "no allowed_roots configured — directory creation is disabled",
        );
    }

    let parent_buf = PathBuf::from(&params.parent);
    let parent_canon = match validate_in_scope(&parent_buf, &roots) {
        Ok(p) => p,
        Err(msg) => return JsonRpcResponse::error(request.id, OUT_OF_SCOPE, &msg),
    };

    let target = parent_canon.join(trimmed);
    if target.exists() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("already exists: {}", target.display()),
        );
    }
    if let Err(e) = tokio::fs::create_dir_all(&target).await {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("mkdir failed: {e}"));
    }
    // canonicalise so the returned path matches list_dir's output style
    // (the new dir definitely exists now, so canonicalize won't fail).
    let canon = std::fs::canonicalize(&target).unwrap_or(target);
    JsonRpcResponse::success(request.id, json!({ "path": canon.to_string_lossy() }))
}

// ─── fs.read_file ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ReadFileParams {
    path: String,
}

pub async fn handle_read_file(request: JsonRpcRequest, config: SharedConfig) -> JsonRpcResponse {
    let params: ReadFileParams = match request
        .params
        .clone()
        .map(serde_json::from_value)
        .transpose()
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "params required");
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            );
        }
    };

    let roots = resolve_roots(&*config.read().await);
    if roots.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            OUT_OF_SCOPE,
            "no allowed_roots configured — file reading is disabled",
        );
    }

    let candidate = PathBuf::from(&params.path);
    let canon = match validate_in_scope(&candidate, &roots) {
        Ok(p) => p,
        Err(msg) => return JsonRpcResponse::error(request.id, OUT_OF_SCOPE, &msg),
    };

    let bytes = match tokio::fs::read(&canon).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return JsonRpcResponse::error(
                request.id,
                NOT_FOUND,
                format!("not found: {}", canon.display()),
            );
        }
        Err(e) => {
            return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("read failed: {e}"));
        }
    };

    let truncated = bytes.len() > READ_FILE_CAP;
    let slice = if truncated {
        // Truncate on a UTF-8 character boundary so multi-byte characters are
        // not split and replaced with U+FFFD in the returned preview.
        let mut cap = READ_FILE_CAP;
        // `bytes` is raw `Vec<u8>`; a UTF-8 continuation byte matches 0b10xxxxxx.
        // Walk back until `cap` lands on a leading byte (a char boundary).
        while cap > 0 && (bytes[cap] & 0xC0) == 0x80 {
            cap -= 1;
        }
        &bytes[..cap]
    } else {
        &bytes[..]
    };
    let content = String::from_utf8_lossy(slice).into_owned();

    JsonRpcResponse::success(
        request.id,
        json!({
            "path": canon.to_string_lossy(),
            "content": content,
            "truncated": truncated,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cfg_with_roots(roots: Vec<String>) -> SharedConfig {
        let mut cfg = Config::default();
        cfg.projects.allowed_roots = roots;
        Arc::new(RwLock::new(cfg))
    }

    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    #[tokio::test]
    async fn list_dir_returns_only_in_scope_entries() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        tokio::fs::create_dir(root.join("alpha")).await.unwrap();
        tokio::fs::create_dir(root.join(".hidden")).await.unwrap();
        tokio::fs::File::create(root.join("readme.txt"))
            .await
            .unwrap();

        let cfg = cfg_with_roots(vec![root.to_string_lossy().to_string()]);
        let r = req("fs.list_dir", json!({ "path": root.to_string_lossy() }));
        let resp = handle_list_dir(r, cfg).await;
        let result = resp.result.expect("expected success");
        let entries = result["entries"].as_array().unwrap();
        // 1 dir + 1 file = 2 (hidden dropped by default)
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|e| e["name"] == "alpha" && e["is_dir"] == true));
        assert!(entries
            .iter()
            .any(|e| e["name"] == "readme.txt" && e["is_dir"] == false));
    }

    #[tokio::test]
    async fn list_dir_show_hidden_includes_dotfiles() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        tokio::fs::create_dir(root.join(".hidden")).await.unwrap();

        let cfg = cfg_with_roots(vec![root.to_string_lossy().to_string()]);
        let r = req(
            "fs.list_dir",
            json!({ "path": root.to_string_lossy(), "show_hidden": true }),
        );
        let resp = handle_list_dir(r, cfg).await;
        let entries = resp.result.unwrap()["entries"].as_array().unwrap().clone();
        assert!(entries.iter().any(|e| e["name"] == ".hidden"));
    }

    #[tokio::test]
    async fn list_dir_rejects_out_of_scope_path() {
        let tmp = TempDir::new().unwrap();
        let scope = tmp.path().join("scope");
        let outside = tmp.path().join("outside");
        tokio::fs::create_dir(&scope).await.unwrap();
        tokio::fs::create_dir(&outside).await.unwrap();

        let cfg = cfg_with_roots(vec![scope.to_string_lossy().to_string()]);
        let r = req("fs.list_dir", json!({ "path": outside.to_string_lossy() }));
        let resp = handle_list_dir(r, cfg).await;
        let err = resp.error.expect("expected error");
        assert_eq!(err.code, OUT_OF_SCOPE);
    }

    #[tokio::test]
    async fn list_dir_rejects_dotdot_escape() {
        let tmp = TempDir::new().unwrap();
        let scope = tmp.path().join("scope");
        let outside = tmp.path().join("outside");
        tokio::fs::create_dir(&scope).await.unwrap();
        tokio::fs::create_dir(&outside).await.unwrap();

        let cfg = cfg_with_roots(vec![scope.to_string_lossy().to_string()]);
        // Use `..` to try to climb out of `scope`. canonicalize will collapse
        // it back to `outside`, which must then be flagged as out of scope.
        let escape = scope.join("..").join("outside");
        let r = req("fs.list_dir", json!({ "path": escape.to_string_lossy() }));
        let resp = handle_list_dir(r, cfg).await;
        let err = resp.error.expect("expected error");
        assert_eq!(err.code, OUT_OF_SCOPE);
    }

    #[tokio::test]
    async fn list_dir_parent_only_when_in_scope() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let child = root.join("child");
        tokio::fs::create_dir(&child).await.unwrap();

        let cfg = cfg_with_roots(vec![root.to_string_lossy().to_string()]);

        // Inside a non-root subdir, parent IS in scope.
        let r1 = req("fs.list_dir", json!({ "path": child.to_string_lossy() }));
        let resp1 = handle_list_dir(r1, cfg.clone()).await;
        let parent1 = resp1.result.unwrap()["parent"].clone();
        assert!(
            parent1.is_string(),
            "expected parent in scope, got {parent1}"
        );

        // At the root itself, parent is OUT of scope → returns null.
        let r2 = req("fs.list_dir", json!({ "path": root.to_string_lossy() }));
        let resp2 = handle_list_dir(r2, cfg).await;
        let parent2 = resp2.result.unwrap()["parent"].clone();
        assert!(
            parent2.is_null(),
            "expected parent OUT of scope, got {parent2}"
        );
    }

    #[tokio::test]
    async fn create_dir_makes_a_subdirectory_inside_scope() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let cfg = cfg_with_roots(vec![root.to_string_lossy().to_string()]);

        let r = req(
            "fs.create_dir",
            json!({ "parent": root.to_string_lossy(), "name": "new-project" }),
        );
        let resp = handle_create_dir(r, cfg).await;
        let result = resp.result.expect("expected success");
        let created = result["path"].as_str().unwrap();
        assert!(PathBuf::from(created).is_dir());
        assert!(created.ends_with("new-project"));
    }

    #[tokio::test]
    async fn create_dir_rejects_path_separators_in_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let cfg = cfg_with_roots(vec![root.to_string_lossy().to_string()]);

        for evil in ["foo/bar", "..", ".", " "] {
            let r = req(
                "fs.create_dir",
                json!({ "parent": root.to_string_lossy(), "name": evil }),
            );
            let resp = handle_create_dir(r, cfg.clone()).await;
            assert_eq!(
                resp.error.expect("expected error").code,
                INVALID_PARAMS,
                "name '{evil}' should have been rejected"
            );
        }
    }

    #[tokio::test]
    async fn create_dir_rejects_existing_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        tokio::fs::create_dir(root.join("already")).await.unwrap();
        let cfg = cfg_with_roots(vec![root.to_string_lossy().to_string()]);

        let r = req(
            "fs.create_dir",
            json!({ "parent": root.to_string_lossy(), "name": "already" }),
        );
        let resp = handle_create_dir(r, cfg).await;
        assert_eq!(resp.error.expect("expected error").code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn allowed_roots_drops_bad_entries() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().to_path_buf();
        let cfg = cfg_with_roots(vec![
            real.to_string_lossy().to_string(),
            "/this/does/not/exist".to_string(),
            "".to_string(),
        ]);
        let r = req("fs.allowed_roots", json!({}));
        let resp = handle_allowed_roots(r, cfg).await;
        let roots = resp.result.unwrap()["roots"].as_array().unwrap().clone();
        assert_eq!(roots.len(), 1, "only the existing dir should survive");
    }

    #[tokio::test]
    async fn home_dir_returns_existing_path() {
        let r = req("fs.home_dir", json!({}));
        let resp = handle_home_dir(r).await;
        let path = resp.result.unwrap()["path"].as_str().unwrap().to_string();
        assert!(
            PathBuf::from(&path).is_dir(),
            "home dir should exist: {path}"
        );
    }

    #[tokio::test]
    async fn read_file_returns_content_in_scope() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file_path = root.join("hello.txt");
        tokio::fs::write(&file_path, "hi there").await.unwrap();

        let cfg = cfg_with_roots(vec![root.to_string_lossy().to_string()]);
        let r = req(
            "fs.read_file",
            json!({ "path": file_path.to_string_lossy() }),
        );
        let resp = handle_read_file(r, cfg).await;
        let result = resp.result.expect("expected success");
        assert_eq!(result["content"], "hi there");
        assert_eq!(result["truncated"], false);
    }

    #[tokio::test]
    async fn read_file_rejects_out_of_scope() {
        let tmp = TempDir::new().unwrap();
        let scope = tmp.path().join("scope");
        let outside = tmp.path().join("outside");
        tokio::fs::create_dir(&scope).await.unwrap();
        tokio::fs::create_dir(&outside).await.unwrap();
        let outside_file = outside.join("not-in-root.txt");
        tokio::fs::write(&outside_file, "secret").await.unwrap();

        let cfg = cfg_with_roots(vec![scope.to_string_lossy().to_string()]);
        let r = req(
            "fs.read_file",
            json!({ "path": outside_file.to_string_lossy() }),
        );
        let resp = handle_read_file(r, cfg).await;
        let err = resp.error.expect("expected error");
        assert_eq!(err.code, OUT_OF_SCOPE);
    }

    #[tokio::test]
    async fn read_file_truncates_oversized_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file_path = root.join("big.txt");
        tokio::fs::write(&file_path, vec![b'a'; READ_FILE_CAP + 1])
            .await
            .unwrap();

        let cfg = cfg_with_roots(vec![root.to_string_lossy().to_string()]);
        let r = req(
            "fs.read_file",
            json!({ "path": file_path.to_string_lossy() }),
        );
        let resp = handle_read_file(r, cfg).await;
        let result = resp.result.expect("expected success");
        assert_eq!(result["truncated"], true);
        // All-ASCII content decodes 1:1, so the capped byte slice maps to
        // exactly READ_FILE_CAP chars.
        assert_eq!(result["content"].as_str().unwrap().len(), READ_FILE_CAP);
    }
}
