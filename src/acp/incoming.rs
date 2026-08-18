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
    #[must_use]
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
    #[must_use]
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
        if !self.policy_allows_read() {
            return HandlerOutcome::Error {
                code: PERMISSION_DENIED,
                message: "fs/read_text_file denied by permission policy".to_string(),
            };
        }
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

    /// True if the policy permits filesystem reads.
    fn policy_allows_read(&self) -> bool {
        !matches!(self.policy, PermissionPolicy::DenyAll)
    }

    // --- path confinement --------------------------------------------------

    /// Resolve `requested` against the workspace root and verify it stays
    /// Confinement check + canonicalisation. **Async** since the underlying
    /// `confine_blocking` performs blocking `std::fs::*` syscalls
    /// (`canonicalize`, `symlink_metadata`, `read_link`) that must NOT run on
    /// a tokio worker thread — every concurrent ACP request and unrelated
    /// async task sharing the runtime would otherwise stall behind them.
    /// Runs in `spawn_blocking` to keep the runtime responsive. See ACP-02.
    async fn confine(&self, requested: &str) -> Result<PathBuf, HandlerOutcome> {
        let root = self.root.clone();
        let req = requested.to_string();
        match tokio::task::spawn_blocking(move || confine_blocking(&root, &req)).await {
            Ok(Ok(p)) => Ok(p),
            Ok(Err(e)) => Err(e),
            Err(join_err) => {
                warn!(
                    requested,
                    error = %join_err,
                    "ACP incoming: spawn_blocking join failed (canonicalize)"
                );
                Err(HandlerOutcome::internal(format!(
                    "fs canonicalize task panicked: {join_err}"
                )))
            }
        }
    }

    /// Synchronous confinement + canonicalisation. Mirrors `confine` but
    /// callable from `spawn_blocking`. See ACP-02.
    fn confine_blocking(root: &Path, requested: &str) -> Result<PathBuf, HandlerOutcome> {
        let raw = Path::new(requested);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            root.join(raw)
        };
        let normalized = lexical_normalize(&joined);
        if !normalized.starts_with(root) {
            warn!(
                requested,
                root = %root.display(),
                "ACP incoming: filesystem access denied outside workspace"
            );
            return Err(HandlerOutcome::Error {
                code: PERMISSION_DENIED,
                message: format!("path '{requested}' is outside the session workspace"),
            });
        }
        match canonicalize_within_root(root, &normalized) {
            Ok(canonical) => Ok(canonical),
            Err(e) => {
                warn!(
                    requested,
                    root = %root.display(),
                    error = %e,
                    "ACP incoming: filesystem access denied (symlink resolution)"
                );
                Err(HandlerOutcome::Error {
                    code: PERMISSION_DENIED,
                    message: format!("path '{requested}' is outside the session workspace"),
                })
            }
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

/// Resolve `path` against the symlink-canonicalized `root`, walking
/// components one at a time so a symlink pointing outside the workspace
/// (or to a nonexistent target) is rejected *before* the syscall that
/// would otherwise follow it. The input `path` is expected to be lexically
/// normalized and absolute — `confine` guarantees that.
fn canonicalize_within_root(root: &Path, path: &Path) -> std::io::Result<PathBuf> {
    let canonical_root = std::fs::canonicalize(root)?;

    // Walk the requested path *against the canonical root* rather than
    // rebuilding it from its own literal prefix. Windows' `canonicalize` always
    // prepends the `\\?\` verbatim prefix, so a literal `C:\ws\f.txt` never
    // `starts_with` a canonical `\\?\C:\ws`: the containment check below
    // rejected every in-workspace path and ACP filesystem access was dead on
    // that platform. `confine` has already proven `path` is under `root`.
    let relative = path.strip_prefix(root).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "path resolves outside workspace",
        )
    })?;
    let mut current: PathBuf = canonical_root.clone();

    for comp in relative.components() {
        match comp {
            // `strip_prefix` yields a relative remainder, so neither can appear.
            // Refuse rather than silently re-rooting if that ever changes.
            Component::Prefix(_) | Component::RootDir => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "path resolves outside workspace",
                ))
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if current.parent().is_some() {
                    current.pop();
                }
            }
            Component::Normal(name) => {
                let candidate = current.join(name);
                match std::fs::symlink_metadata(&candidate) {
                    Ok(meta) if meta.is_symlink() => {
                        let target = std::fs::read_link(&candidate)?;
                        let base = candidate.parent().unwrap_or_else(|| Path::new(""));
                        let resolved = if target.is_absolute() {
                            lexical_normalize(&target)
                        } else {
                            lexical_normalize(&base.join(&target))
                        };
                        current = std::fs::canonicalize(&resolved)?;
                    }
                    Ok(_) => current.push(name),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => current.push(name),
                    Err(e) => return Err(e),
                }
            }
        }
    }

    if !current.starts_with(&canonical_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "path resolves outside workspace",
        ));
    }
    Ok(current)
}

/// Apply an optional 1-based `line`/`limit` window to file content. Absent
/// bounds return the whole file.
fn apply_line_window(content: &str, line: Option<u64>, limit: Option<u64>) -> String {
    if line.is_none() && limit.is_none() {
        return content.to_string();
    }
    // Subtract in u64 space (line is clamped to >=1, so this never underflows),
    // then convert. Width is bounded by the file's line count below.
    let start = usize::try_from(line.unwrap_or(1).max(1) - 1).unwrap_or(usize::MAX);
    let lines: Vec<&str> = content.lines().collect();
    if start >= lines.len() {
        return String::new();
    }
    // `saturating_add` so an attacker-supplied huge `limit` can't overflow
    // `usize` (a debug-build panic in this "never panics" handler).
    let end = match limit {
        Some(n) => start
            .saturating_add(usize::try_from(n).unwrap_or(usize::MAX))
            .min(lines.len()),
        None => lines.len(),
    };
    lines
        .get(start..end)
        // Safe: `start` is guarded by the check above, and `end` is clamped to `lines.len()`.
        .expect("invariant: start/end are within line bounds")
        .join("\n")
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
            // Match the whole kind or a `_`/`-`-delimited token — NOT a raw
            // substring. A substring `contains` would let a reject-ish option
            // like "disallow" satisfy a wanted "allow", inverting the
            // permission decision. "allow_once" still matches generic "allow"
            // via its leading token.
            let matches = kind == *want || kind.split(['_', '-']).any(|tok| tok == *want);
            if matches {
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

    fn unique_suffix() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{}_{}", std::process::id(), nanos)
    }

    fn handler_at(dir: &Path, policy: PermissionPolicy) -> IncomingHandler {
        IncomingHandler::new(Some(dir.to_str().unwrap()), policy)
    }

    #[tokio::test]
    async fn fs_read_returns_file_content() {
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
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
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
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
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
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
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
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
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
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
    fn pick_option_does_not_invert_disallow_as_allow() {
        // A reject-ish "disallow" option must NEVER satisfy a wanted "allow"
        // (a raw substring `contains` would, since "disallow".contains("allow")).
        let opts = json!([
            { "optionId": "no", "kind": "disallow" },
            { "optionId": "yes", "kind": "allow_once" },
        ]);
        assert_eq!(
            pick_option(
                opts.as_array().unwrap(),
                &["allow_once", "allow_always", "allow"]
            ),
            Some("yes".to_string())
        );

        // With only a "disallow" option, wanting allow must find nothing.
        let only_disallow = json!([{ "optionId": "no", "kind": "disallow" }]);
        assert_eq!(
            pick_option(
                only_disallow.as_array().unwrap(),
                &["allow_once", "allow_always", "allow"]
            ),
            None
        );

        // Generic "allow" still matches the leading token of "allow_always".
        let only_always = json!([{ "optionId": "aa", "kind": "allow_always" }]);
        assert_eq!(
            pick_option(only_always.as_array().unwrap(), &["allow"]),
            Some("aa".to_string())
        );
    }

    #[test]
    fn line_window_whole_file_when_unbounded() {
        assert_eq!(apply_line_window("a\nb", None, None), "a\nb");
        assert_eq!(apply_line_window("a\nb\nc", Some(2), None), "b\nc");
        assert_eq!(apply_line_window("a\nb\nc", Some(5), None), "");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_inside_root_round_trips() {
        use std::os::unix::fs::symlink;
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("real.txt"), "inner-content\n")
            .await
            .unwrap();
        tokio::fs::create_dir_all(dir.join("real_dir"))
            .await
            .unwrap();
        tokio::fs::write(dir.join("real_dir/nested.txt"), "nested\n")
            .await
            .unwrap();
        symlink(dir.join("real.txt"), dir.join("link_file")).unwrap();
        symlink(dir.join("real_dir"), dir.join("link_dir")).unwrap();

        let h = handler_at(&dir, PermissionPolicy::ApproveAll);

        let read_link = h
            .handle("fs/read_text_file", &json!({ "path": "link_file" }))
            .await;
        match read_link {
            HandlerOutcome::Result(v) => {
                assert!(v["content"].as_str().unwrap().contains("inner-content"));
            }
            other => panic!("inside-root symlink read should succeed, got {other:?}"),
        }

        let read_nested = h
            .handle(
                "fs/read_text_file",
                &json!({ "path": "link_dir/nested.txt" }),
            )
            .await;
        match read_nested {
            HandlerOutcome::Result(v) => {
                assert!(v["content"].as_str().unwrap().contains("nested"));
            }
            other => panic!("inside-root symlinked dir read should succeed, got {other:?}"),
        }

        let write_through = h
            .handle(
                "fs/write_text_file",
                &json!({ "path": "link_dir/written.txt", "content": "ok" }),
            )
            .await;
        assert!(matches!(write_through, HandlerOutcome::Result(_)));
        let written = tokio::fs::read_to_string(dir.join("real_dir/written.txt"))
            .await
            .unwrap();
        assert_eq!(written, "ok");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_outside_root_rejected() {
        use std::os::unix::fs::symlink;
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        // A SEPARATE scratch root, not a sibling under the same one: the point
        // of the test is that `outside` is not reachable from `dir`'s root.
        let (_scratch_outside, outside) = crate::utils::scratch::scratch_root();
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        tokio::fs::write(outside.join("secret.txt"), "outside-data\n")
            .await
            .unwrap();
        tokio::fs::write(outside.join("nested.txt"), "nested-out\n")
            .await
            .unwrap();
        symlink(&outside, dir.join("escape")).unwrap();
        symlink(outside.join("nested.txt"), dir.join("escape_file")).unwrap();

        let h = handler_at(&dir, PermissionPolicy::ApproveAll);

        let read_through_dir = h
            .handle("fs/read_text_file", &json!({ "path": "escape/secret.txt" }))
            .await;
        match read_through_dir {
            HandlerOutcome::Error { code, .. } => assert_eq!(code, PERMISSION_DENIED),
            other => panic!("outside-root symlink dir read must be denied, got {other:?}"),
        }

        let read_direct = h
            .handle("fs/read_text_file", &json!({ "path": "escape_file" }))
            .await;
        match read_direct {
            HandlerOutcome::Error { code, .. } => assert_eq!(code, PERMISSION_DENIED),
            other => panic!("outside-root symlink file read must be denied, got {other:?}"),
        }

        let write_through_dir = h
            .handle(
                "fs/write_text_file",
                &json!({ "path": "escape/new.txt", "content": "leak" }),
            )
            .await;
        match write_through_dir {
            HandlerOutcome::Error { code, .. } => assert_eq!(code, PERMISSION_DENIED),
            other => panic!("outside-root symlink dir write must be denied, got {other:?}"),
        }
        assert!(
            !outside.join("new.txt").exists(),
            "write through escape symlink must not have created a file outside the workspace"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
        let _ = tokio::fs::remove_dir_all(&outside).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_symlink_rejected() {
        use std::os::unix::fs::symlink;
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let bogus = format!("/definitely/does/not/exist/aleph_acp_{}", unique_suffix());
        symlink(&bogus, dir.join("dangle")).unwrap();
        let file_link = format!(
            "/definitely/does/not/exist/aleph_acp_file_{}",
            unique_suffix()
        );
        symlink(&file_link, dir.join("dangle_file")).unwrap();

        let h = handler_at(&dir, PermissionPolicy::ApproveAll);

        let read = h
            .handle(
                "fs/read_text_file",
                &json!({ "path": "dangle/anything.txt" }),
            )
            .await;
        match read {
            HandlerOutcome::Error { code, .. } => assert_eq!(code, PERMISSION_DENIED),
            other => panic!("dangling symlink read must be denied, got {other:?}"),
        }

        let read_direct = h
            .handle("fs/read_text_file", &json!({ "path": "dangle_file" }))
            .await;
        match read_direct {
            HandlerOutcome::Error { code, .. } => assert_eq!(code, PERMISSION_DENIED),
            other => panic!("dangling symlink final read must be denied, got {other:?}"),
        }

        let write = h
            .handle(
                "fs/write_text_file",
                &json!({ "path": "dangle/new.txt", "content": "x" }),
            )
            .await;
        match write {
            HandlerOutcome::Error { code, .. } => assert_eq!(code, PERMISSION_DENIED),
            other => panic!("dangling symlink write must be denied, got {other:?}"),
        }

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn new_file_write_normal_succeeds() {
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let h = handler_at(&dir, PermissionPolicy::ApproveAll);
        let out = h
            .handle(
                "fs/write_text_file",
                &json!({ "path": "fresh/new.txt", "content": "data" }),
            )
            .await;
        assert!(matches!(out, HandlerOutcome::Result(_)));
        let written = tokio::fs::read_to_string(dir.join("fresh/new.txt"))
            .await
            .unwrap();
        assert_eq!(written, "data");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tmp_style_root_resolves_through_symlink() {
        // Build the symlinked root instead of borrowing the platform's: only
        // macOS hands out a `/tmp` that is itself a symlink (to `/private/tmp`).
        // On Linux `/tmp` is a real directory, so canonicalising it returns the
        // path unchanged and the sanity assertion below failed the whole test on
        // every Linux runner — the case this test exists to cover was never
        // actually exercised there.
        let (_scratch, base) = crate::utils::scratch::scratch_root();
        let real = base.join("real");
        let ws = base.join("link");
        tokio::fs::create_dir_all(&real).await.unwrap();
        tokio::fs::symlink(&real, &ws).await.unwrap();
        tokio::fs::write(ws.join("hello.txt"), "via-tmp\n")
            .await
            .unwrap();

        let real_canon = std::fs::canonicalize(&ws).unwrap();
        assert_ne!(
            ws, real_canon,
            "sanity: the workspace root must resolve through a symlink for this test"
        );

        let h = handler_at(&ws, PermissionPolicy::ApproveAll);
        let abs = ws.join("hello.txt").to_str().unwrap().to_string();
        let read = h.handle("fs/read_text_file", &json!({ "path": abs })).await;
        match read {
            HandlerOutcome::Result(v) => {
                assert!(v["content"].as_str().unwrap().contains("via-tmp"));
            }
            other => panic!("read via /tmp symlink must succeed, got {other:?}"),
        }

        let write = h
            .handle(
                "fs/write_text_file",
                &json!({ "path": "written.txt", "content": "ok" }),
            )
            .await;
        assert!(matches!(write, HandlerOutcome::Result(_)));
        let written = tokio::fs::read_to_string(ws.join("written.txt"))
            .await
            .unwrap();
        assert_eq!(written, "ok");

        let _ = tokio::fs::remove_dir_all(&base).await;
    }
}
