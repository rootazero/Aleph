//! Agent→client request handling — the *server* half of the bidirectional ACP.
//!
//! ACP is bidirectional JSON-RPC 2.0: while an agent (Claude Code, Codex,
//! Gemini under `--acp`) processes a prompt, it can send **requests back to the
//! client** that REQUIRE a response — `fs/read_text_file`, `fs/write_text_file`,
//! `session/request_permission`, `terminal/*`. If the client never answers,
//! the agent blocks until the prompt times out.
//!
//! This module supplies the handler the transport invokes for those incoming
//! requests. Filesystem operations are confined to the session's working
//! directory subtree (the agent's sandbox); `session/request_permission` is
//! resolved by a [`PermissionPolicy`] that mirrors acpx's
//! `approve-all` / `approve-reads` / `deny-all` modes.
//!
//! Terminal (`terminal/*`) is intentionally NOT advertised in `initialize`
//! (see [`crate::acp::protocol::AcpRequest::initialize`]) — spec-compliant
//! agents therefore won't request it. If one does anyway, the handler returns
//! JSON-RPC `-32601 method not found` so the agent degrades gracefully instead
//! of hanging.

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};
use tracing::{debug, warn};

/// How the client answers `session/request_permission` and gates filesystem
/// writes. Mirrors acpx's three permission modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionPolicy {
    /// Approve everything (reads, writes, tool execution). Filesystem access is
    /// still confined to the workspace subtree. This is the default for a
    /// headless coding agent the user explicitly spawned to work in `cwd`.
    #[default]
    ApproveAll,
    /// Approve read/search operations; deny writes and reject non-read
    /// permission prompts.
    ApproveReads,
    /// Deny everything — useful for a read-only audit harness.
    DenyAll,
}

/// Outcome of handling an agent→client request: either a JSON-RPC `result`
/// payload or a JSON-RPC `error`.
#[derive(Debug)]
pub enum HandlerOutcome {
    Result(Value),
    Error { code: i64, message: String },
}

impl HandlerOutcome {
    fn method_not_found(method: &str) -> Self {
        Self::Error {
            code: -32601,
            message: format!("Method not found: {method}"),
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::Error {
            code: -32602,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::Error {
            code: -32603,
            message: message.into(),
        }
    }

    /// Wrap into a full JSON-RPC 2.0 response envelope addressed to `id`.
    pub fn into_jsonrpc(self, id: u64) -> Value {
        match self {
            Self::Result(result) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }),
            Self::Error { code, message } => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            }),
        }
    }
}

/// Handles agent→client requests for one ACP session.
///
/// Cheap to clone (the only owned state is the workspace root + a `Copy`
/// policy), so it is wrapped in an `Arc` and shared with the transport.
#[derive(Debug, Clone)]
pub struct IncomingHandler {
    /// Workspace root all filesystem access is confined to.
    root: PathBuf,
    policy: PermissionPolicy,
}

impl IncomingHandler {
    /// Build a handler rooted at `cwd` (falls back to the process working
    /// directory when `cwd` is absent).
    ///
    /// The root is made absolute and lexically normalized — NOT
    /// symlink-canonicalized — so it shares one path namespace with the
    /// (also lexically normalized) request paths in [`confine`](Self::confine).
    /// Canonicalizing only the root would wrongly deny legitimate absolute
    /// paths the agent derives from the cwd string we handed it when that cwd
    /// contains a symlink (e.g. macOS `/tmp` → `/private/tmp`).
    pub fn new(cwd: Option<&str>, policy: PermissionPolicy) -> Self {
        let root = match cwd {
            Some(c) => {
                let p = Path::new(c);
                let abs = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    std::env::current_dir().unwrap_or_default().join(p)
                };
                lexical_normalize(&abs)
            }
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };
        Self { root, policy }
    }

    /// Dispatch one agent→client request. Pure async; never panics.
    pub async fn handle(&self, method: &str, params: &Value) -> HandlerOutcome {
        match method {
            "fs/read_text_file" => self.fs_read(params).await,
            "fs/write_text_file" => self.fs_write(params).await,
            "session/request_permission" => self.request_permission(params),
            other => {
                debug!(method = other, "ACP incoming: unsupported agent request");
                HandlerOutcome::method_not_found(other)
            }
        }
    }

    // --- fs/read_text_file -------------------------------------------------

    async fn fs_read(&self, params: &Value) -> HandlerOutcome {
        let Some(path) = params.get("path").and_then(Value::as_str) else {
            return HandlerOutcome::invalid_params("fs/read_text_file: missing 'path'");
        };
        let abs = match self.confine(path) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let content = match tokio::fs::read_to_string(&abs).await {
            Ok(c) => c,
            Err(e) => {
                return HandlerOutcome::internal(format!("fs/read_text_file: {e}"));
            }
        };
        // Optional 1-based line window: `line` = start line, `limit` = count.
        let line = params.get("line").and_then(Value::as_u64);
        let limit = params.get("limit").and_then(Value::as_u64);
        let windowed = apply_line_window(&content, line, limit);
        HandlerOutcome::Result(json!({ "content": windowed }))
    }

    // --- fs/write_text_file ------------------------------------------------

    async fn fs_write(&self, params: &Value) -> HandlerOutcome {
        if self.policy != PermissionPolicy::ApproveAll {
            return HandlerOutcome::Error {
                code: PERMISSION_DENIED,
                message: "fs/write_text_file denied by permission policy".to_string(),
            };
        }
        let Some(path) = params.get("path").and_then(Value::as_str) else {
            return HandlerOutcome::invalid_params("fs/write_text_file: missing 'path'");
        };
        let Some(content) = params.get("content").and_then(Value::as_str) else {
            return HandlerOutcome::invalid_params("fs/write_text_file: missing 'content'");
        };
        let abs = match self.confine(path) {
            Ok(p) => p,
            Err(e) => return e,
        };
        if let Some(parent) = abs.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return HandlerOutcome::internal(format!("fs/write_text_file mkdir: {e}"));
            }
        }
        if let Err(e) = tokio::fs::write(&abs, content).await {
            return HandlerOutcome::internal(format!("fs/write_text_file: {e}"));
        }
        // ACP write response carries no payload.
        HandlerOutcome::Result(Value::Null)
    }

    // --- session/request_permission ---------------------------------------

    fn request_permission(&self, params: &Value) -> HandlerOutcome {
        let options = params
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let allow = self.permission_allows(params);
        let chosen = if allow {
            pick_option(&options, &["allow_once", "allow_always", "allow"])
        } else {
            pick_option(
                &options,
                &["reject_once", "reject_always", "reject", "deny"],
            )
        };

        match chosen {
            Some(option_id) => HandlerOutcome::Result(json!({
                "outcome": { "outcome": "selected", "optionId": option_id }
            })),
            None => {
                // No matching option to honor the decision — cancel cleanly.
                HandlerOutcome::Result(json!({ "outcome": { "outcome": "cancelled" } }))
            }
        }
    }

    /// Decide approve vs reject for a permission request under the policy.
    fn permission_allows(&self, params: &Value) -> bool {
        match self.policy {
            PermissionPolicy::ApproveAll => true,
            PermissionPolicy::DenyAll => false,
            PermissionPolicy::ApproveReads => {
                let kind = params
                    .get("toolCall")
                    .and_then(|t| t.get("kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                matches!(kind, "read" | "search" | "fetch" | "think")
            }
        }
    }

    // --- path confinement --------------------------------------------------

    /// Resolve `requested` against the workspace root and verify it stays
    /// inside the subtree. Lexical (no symlink resolution) so it works for
    /// not-yet-existing write targets.
    fn confine(&self, requested: &str) -> Result<PathBuf, HandlerOutcome> {
        let raw = Path::new(requested);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.root.join(raw)
        };
        let normalized = lexical_normalize(&joined);
        if normalized.starts_with(&self.root) {
            Ok(normalized)
        } else {
            warn!(
                requested,
                root = %self.root.display(),
                "ACP incoming: filesystem access denied outside workspace"
            );
            Err(HandlerOutcome::Error {
                code: PERMISSION_DENIED,
                message: format!("path '{requested}' is outside the session workspace"),
            })
        }
    }
}

/// JSON-RPC-adjacent permission error code. Negative app-defined range; mirrors
/// the spirit of acpx's `PERMISSION_DENIED` classification.
const PERMISSION_DENIED: i64 = -32000;

/// Lexically normalize a path (resolve `.`/`..` without touching the
/// filesystem). Mirrors `manager::session_key::normalize_path` but kept local
/// to avoid widening that module's visibility.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => result.push(p.as_os_str()),
            Component::RootDir => result.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if result.parent().is_some() {
                    result.pop();
                } else {
                    result.push("..");
                }
            }
            Component::Normal(name) => result.push(name),
        }
    }
    result
}

/// Apply an optional 1-based `line`/`limit` window to file content. Absent
/// bounds return the whole file.
fn apply_line_window(content: &str, line: Option<u64>, limit: Option<u64>) -> String {
    if line.is_none() && limit.is_none() {
        return content.to_string();
    }
    let start = line.unwrap_or(1).max(1) as usize - 1;
    let lines: Vec<&str> = content.lines().collect();
    let end = match limit {
        Some(n) => (start + n as usize).min(lines.len()),
        None => lines.len(),
    };
    if start >= lines.len() {
        return String::new();
    }
    lines[start..end].join("\n")
}

/// Pick the first option whose `kind` (or `name`) matches one of `wanted`,
/// returning its `optionId`.
fn pick_option(options: &[Value], wanted: &[&str]) -> Option<String> {
    for want in wanted {
        for opt in options {
            let kind = opt
                .get("kind")
                .or_else(|| opt.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if kind == *want || kind.contains(want) {
                if let Some(id) = opt.get("optionId").and_then(Value::as_str) {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler_at(dir: &Path, policy: PermissionPolicy) -> IncomingHandler {
        IncomingHandler::new(Some(dir.to_str().unwrap()), policy)
    }

    #[tokio::test]
    async fn fs_read_returns_file_content() {
        let dir = std::env::temp_dir().join(format!("acp_inc_read_{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let file = dir.join("hello.txt");
        tokio::fs::write(&file, "line1\nline2\nline3\n")
            .await
            .unwrap();

        let h = handler_at(&dir, PermissionPolicy::ApproveAll);
        let out = h
            .handle("fs/read_text_file", &json!({ "path": "hello.txt" }))
            .await;
        match out {
            HandlerOutcome::Result(v) => {
                assert!(v["content"].as_str().unwrap().contains("line2"));
            }
            other => panic!("expected result, got {other:?}"),
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn fs_read_line_window() {
        let dir = std::env::temp_dir().join(format!("acp_inc_win_{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("f.txt"), "a\nb\nc\nd\ne\n")
            .await
            .unwrap();
        let h = handler_at(&dir, PermissionPolicy::ApproveAll);
        let out = h
            .handle(
                "fs/read_text_file",
                &json!({ "path": "f.txt", "line": 2, "limit": 2 }),
            )
            .await;
        match out {
            HandlerOutcome::Result(v) => assert_eq!(v["content"].as_str().unwrap(), "b\nc"),
            other => panic!("expected result, got {other:?}"),
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn fs_read_escape_denied() {
        let dir = std::env::temp_dir().join(format!("acp_inc_esc_{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let h = handler_at(&dir, PermissionPolicy::ApproveAll);
        let out = h
            .handle(
                "fs/read_text_file",
                &json!({ "path": "../../../etc/passwd" }),
            )
            .await;
        match out {
            HandlerOutcome::Error { code, .. } => assert_eq!(code, PERMISSION_DENIED),
            other => panic!("expected permission error, got {other:?}"),
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn fs_write_round_trips_under_approve_all() {
        let dir = std::env::temp_dir().join(format!("acp_inc_wr_{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let h = handler_at(&dir, PermissionPolicy::ApproveAll);
        let out = h
            .handle(
                "fs/write_text_file",
                &json!({ "path": "sub/new.txt", "content": "hi" }),
            )
            .await;
        assert!(matches!(out, HandlerOutcome::Result(_)));
        let written = tokio::fs::read_to_string(dir.join("sub/new.txt"))
            .await
            .unwrap();
        assert_eq!(written, "hi");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn fs_write_denied_under_approve_reads() {
        let dir = std::env::temp_dir().join(format!("acp_inc_ro_{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let h = handler_at(&dir, PermissionPolicy::ApproveReads);
        let out = h
            .handle(
                "fs/write_text_file",
                &json!({ "path": "x.txt", "content": "nope" }),
            )
            .await;
        match out {
            HandlerOutcome::Error { code, .. } => assert_eq!(code, PERMISSION_DENIED),
            other => panic!("expected denial, got {other:?}"),
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn permission_approve_all_selects_allow_option() {
        let h = IncomingHandler::new(None, PermissionPolicy::ApproveAll);
        let out = h
            .handle(
                "session/request_permission",
                &json!({
                    "options": [
                        { "optionId": "a", "kind": "allow_once" },
                        { "optionId": "r", "kind": "reject_once" }
                    ],
                    "toolCall": { "kind": "edit" }
                }),
            )
            .await;
        match out {
            HandlerOutcome::Result(v) => {
                assert_eq!(v["outcome"]["outcome"], "selected");
                assert_eq!(v["outcome"]["optionId"], "a");
            }
            other => panic!("expected selected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn permission_deny_all_selects_reject_option() {
        let h = IncomingHandler::new(None, PermissionPolicy::DenyAll);
        let out = h
            .handle(
                "session/request_permission",
                &json!({
                    "options": [
                        { "optionId": "a", "kind": "allow_once" },
                        { "optionId": "r", "kind": "reject_once" }
                    ]
                }),
            )
            .await;
        match out {
            HandlerOutcome::Result(v) => assert_eq!(v["outcome"]["optionId"], "r"),
            other => panic!("expected reject selection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn permission_approve_reads_allows_read_rejects_edit() {
        let h = IncomingHandler::new(None, PermissionPolicy::ApproveReads);
        let opts = json!([
            { "optionId": "a", "kind": "allow_once" },
            { "optionId": "r", "kind": "reject_once" }
        ]);
        let read = h
            .handle(
                "session/request_permission",
                &json!({ "options": opts, "toolCall": { "kind": "read" } }),
            )
            .await;
        match read {
            HandlerOutcome::Result(v) => assert_eq!(v["outcome"]["optionId"], "a"),
            other => panic!("expected allow, got {other:?}"),
        }
        let edit = h
            .handle(
                "session/request_permission",
                &json!({ "options": opts, "toolCall": { "kind": "edit" } }),
            )
            .await;
        match edit {
            HandlerOutcome::Result(v) => assert_eq!(v["outcome"]["optionId"], "r"),
            other => panic!("expected reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let h = IncomingHandler::new(None, PermissionPolicy::ApproveAll);
        let out = h.handle("terminal/create", &json!({})).await;
        match out {
            HandlerOutcome::Error { code, .. } => assert_eq!(code, -32601),
            other => panic!("expected -32601, got {other:?}"),
        }
    }

    #[test]
    fn into_jsonrpc_shapes() {
        let r = HandlerOutcome::Result(json!({ "content": "x" })).into_jsonrpc(7);
        assert_eq!(r["id"], 7);
        assert_eq!(r["result"]["content"], "x");
        let e = HandlerOutcome::method_not_found("foo").into_jsonrpc(8);
        assert_eq!(e["error"]["code"], -32601);
    }

    #[test]
    fn line_window_whole_file_when_unbounded() {
        assert_eq!(apply_line_window("a\nb", None, None), "a\nb");
        assert_eq!(apply_line_window("a\nb\nc", Some(2), None), "b\nc");
        assert_eq!(apply_line_window("a\nb\nc", Some(5), None), "");
    }
}
