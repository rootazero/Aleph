//! Identity Management Handlers
//!
//! RPC methods for reading/writing the AI's **identity files** (`SOUL.md`,
//! `IDENTITY.md`, …) — the single source of truth for the assistant's persona.
//! These are the exact files the `self_config` LLM tool edits in-conversation,
//! and that `SoulLayer` / `IdentityFilesLayer` inject into the system prompt
//! each turn, so a change here takes effect on the next turn.
//!
//! Historically these handlers manipulated a session-scoped `SoulManifest`
//! override held in an `IdentityResolver` that was **never wired into prompt
//! assembly** — so `identity.set` silently no-op'd. They now operate directly
//! on the agent identity directory, the same source `self_config` writes, so
//! the RPC/CLI surface is honest and effective.
//!
//! | Method | Description |
//! |--------|-------------|
//! | identity.get   | Live `SOUL.md` (raw + parsed structured preview), parsed `IDENTITY.md` rich fields, + identity-file status |
//! | identity.set   | Write an identity file (`SOUL.md` by default), snapshotting the prior version |
//! | identity.clear | Snapshot and remove `SOUL.md` (revert to the default persona) |
//! | identity.list  | List identity files (exists / size / path) |
//!
//! All operate on `~/.aleph/agents/{agent_id}/`. `agent_id` is an optional
//! request field, defaulting to the daemon's default agent captured at boot.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use crate::thinker::identity_files::{
    backup_identity_file, list_identity_file_status, validate_identity_file_name,
    write_identity_file,
};
use crate::thinker::identity_profile::AgentIdentityProfile;
use crate::thinker::soul::SoulManifest;

/// The default identity file that `get` / `set` / `clear` target when the
/// request does not name one.
const DEFAULT_IDENTITY_FILE: &str = "SOUL.md";

// ============================================================================
// Shared handler context
// ============================================================================

/// Context shared by the identity handlers: the default agent whose identity
/// directory they operate on when a request omits `agent_id`, plus the base
/// under which agent directories live (`~/.aleph/agents/`).
pub struct IdentityHandlerContext {
    default_agent_id: String,
    agents_base: PathBuf,
}

impl IdentityHandlerContext {
    /// Production constructor: agent identity directories live under
    /// `~/.aleph/agents/`. Falls back to a relative path only if the home
    /// directory cannot be resolved (mirrors `SelfConfigTool::new`).
    #[must_use]
    pub fn new(default_agent_id: impl Into<String>) -> Self {
        let agents_base = crate::discovery::aleph_agents_dir()
            .unwrap_or_else(|_| PathBuf::from(".aleph").join("agents"));
        Self {
            default_agent_id: default_agent_id.into(),
            agents_base,
        }
    }

    /// Explicit constructor with a custom agents base directory (used in tests
    /// to point at a temp dir).
    #[must_use]
    pub fn with_base(default_agent_id: impl Into<String>, agents_base: impl Into<PathBuf>) -> Self {
        Self {
            default_agent_id: default_agent_id.into(),
            agents_base: agents_base.into(),
        }
    }

    /// Resolve the `(agent_id, agent_dir)` a request targets, honoring an
    /// optional `agent_id` field and rejecting path-unsafe ids.
    fn resolve(&self, params: Option<&Value>) -> Result<(String, PathBuf), String> {
        let agent_id = params
            .and_then(|p| p.get("agent_id"))
            .and_then(Value::as_str)
            .map_or_else(|| self.default_agent_id.clone(), str::to_string);
        if agent_id.is_empty()
            || agent_id.contains('/')
            || agent_id.contains('\\')
            || agent_id.contains("..")
            || agent_id.contains('\0')
        {
            return Err(format!("Invalid agent_id: {agent_id:?}"));
        }
        let dir = self.agents_base.join(&agent_id);
        Ok((agent_id, dir))
    }
}

/// Shared handle passed into each identity handler.
pub type SharedIdentityCtx = Arc<IdentityHandlerContext>;

// ============================================================================
// identity.get
// ============================================================================

/// Handle `identity.get` — return the live `SOUL.md` (raw + a best-effort
/// structured preview parsed via [`SoulManifest::from_markdown`]), the parsed
/// rich-identity fields from `IDENTITY.md`, plus the status of every identity
/// file.
pub async fn handle_get(request: JsonRpcRequest, ctx: SharedIdentityCtx) -> JsonRpcResponse {
    let (agent_id, agent_dir) = match ctx.resolve(request.params.as_ref()) {
        Ok(v) => v,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };

    let soul_path = agent_dir.join(DEFAULT_IDENTITY_FILE);
    let soul_md = tokio::fs::read_to_string(&soul_path).await.ok();
    // Structured preview parsed from the text already read above. Parsing the
    // in-memory string rather than re-opening the path keeps this handler off
    // blocking `std::fs` inside the async runtime and removes a second read
    // that could observe a different file than the one returned as `soul_md`.
    let parsed = soul_md
        .as_deref()
        .and_then(|c| SoulManifest::from_markdown(c).ok());

    // Rich identity fields (name / role / vibe / emoji / language) parsed from
    // IDENTITY.md. These are archetype-seeded at creation and were previously
    // write-only — no reader existed, so no caller could render the agent's
    // chosen emoji or name without re-implementing the parse.
    let identity_dir = agent_dir.clone();
    let profile =
        tokio::task::spawn_blocking(move || AgentIdentityProfile::from_agent_dir(&identity_dir))
            .await
            .unwrap_or_default();

    let files = list_identity_file_status(&agent_dir);
    let has_custom_identity = files.iter().any(|f| f.exists);

    let value = json!({
        "agent_id": agent_id,
        "soul_md": soul_md.map_or(Value::Null, Value::String),
        "parsed": serde_json::to_value(&parsed).unwrap_or(Value::Null),
        "identity": serde_json::to_value(&profile).unwrap_or(Value::Null),
        "files": serde_json::to_value(&files).unwrap_or_default(),
        "has_custom_identity": has_custom_identity,
    });
    JsonRpcResponse::success(request.id, value)
}

// ============================================================================
// identity.set
// ============================================================================

/// Request for `identity.set`.
#[derive(Debug, Deserialize)]
struct IdentitySetRequest {
    /// Full markdown content to write.
    content: String,
    /// Which identity file to write (defaults to `SOUL.md`). Must be a
    /// canonical identity file name.
    #[serde(default)]
    file_name: Option<String>,
}

/// Handle `identity.set` — write markdown content to an identity file
/// (`SOUL.md` by default), snapshotting any prior version. Takes effect on the
/// next turn (the file is re-read into the prompt each turn).
pub async fn handle_set(request: JsonRpcRequest, ctx: SharedIdentityCtx) -> JsonRpcResponse {
    let params = match request.params.as_ref() {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: content".to_string(),
            );
        }
    };

    let set: IdentitySetRequest = match serde_json::from_value(params.clone()) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid parameters: {e}"),
            );
        }
    };

    let (agent_id, agent_dir) = match ctx.resolve(request.params.as_ref()) {
        Ok(v) => v,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };

    let file_name = set.file_name.as_deref().unwrap_or(DEFAULT_IDENTITY_FILE);
    match write_identity_file(&agent_dir, file_name, &set.content) {
        Ok(outcome) => JsonRpcResponse::success(
            request.id,
            json!({
                "success": true,
                "agent_id": agent_id,
                "file_name": file_name,
                "bytes_written": outcome.bytes_written,
                "backup_path": outcome.backup_path.map(|p| p.display().to_string()),
                "note": "Identity updated. Takes effect on the next turn.",
            }),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    }
}

// ============================================================================
// identity.clear
// ============================================================================

/// Handle `identity.clear` — snapshot and remove an identity file (`SOUL.md`
/// by default), reverting to the default persona. Idempotent: clearing an
/// absent file succeeds with `had_content: false`.
pub async fn handle_clear(request: JsonRpcRequest, ctx: SharedIdentityCtx) -> JsonRpcResponse {
    let (agent_id, agent_dir) = match ctx.resolve(request.params.as_ref()) {
        Ok(v) => v,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };

    let file_name = request
        .params
        .as_ref()
        .and_then(|p| p.get("file_name"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_IDENTITY_FILE)
        .to_string();

    if let Err(e) = validate_identity_file_name(&file_name) {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, e);
    }

    let path = agent_dir.join(&file_name);
    if !path.exists() {
        return JsonRpcResponse::success(
            request.id,
            json!({
                "success": true,
                "agent_id": agent_id,
                "file_name": file_name,
                "had_content": false,
                "backup_path": Value::Null,
                "note": "No custom identity file to clear.",
            }),
        );
    }

    let backup = backup_identity_file(&agent_dir, &file_name, &path);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "success": true,
                "agent_id": agent_id,
                "file_name": file_name,
                "had_content": true,
                "backup_path": backup.map(|p| p.display().to_string()),
                "note": "Identity reverted to default. Takes effect on the next turn.",
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to remove {file_name}: {e}"),
        ),
    }
}

// ============================================================================
// identity.list
// ============================================================================

/// Handle `identity.list` — report the status (exists / size / path) of every
/// canonical identity file for the target agent.
pub async fn handle_list(request: JsonRpcRequest, ctx: SharedIdentityCtx) -> JsonRpcResponse {
    let (agent_id, agent_dir) = match ctx.resolve(request.params.as_ref()) {
        Ok(v) => v,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };

    let files = list_identity_file_status(&agent_dir);
    JsonRpcResponse::success(
        request.id,
        json!({
            "agent_id": agent_id,
            "files": serde_json::to_value(&files).unwrap_or_default(),
        }),
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a context whose agents base is a temp dir, with a pre-created
    /// agent directory for `agent_id`.
    fn ctx_with_agent(base: &std::path::Path, agent_id: &str) -> SharedIdentityCtx {
        std::fs::create_dir_all(base.join(agent_id)).unwrap();
        Arc::new(IdentityHandlerContext::with_base(agent_id, base))
    }

    #[tokio::test]
    async fn get_reports_absent_identity() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_with_agent(tmp.path(), "main");
        let req = JsonRpcRequest::with_id("identity.get", None, json!(1));

        let resp = handle_get(req, ctx).await;
        assert!(resp.is_success());
        let r = resp.result.unwrap();
        assert_eq!(r["agent_id"], "main");
        assert!(r["soul_md"].is_null());
        assert_eq!(r["has_custom_identity"], false);
        // All canonical files listed, none existing.
        assert_eq!(r["files"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn get_exposes_parsed_identity_md_fields() {
        // Regression: IDENTITY.md's rich fields are archetype-seeded at agent
        // creation but had no reader — `identity.get` could return the raw
        // SOUL.md while no caller could learn the agent's name or emoji
        // without re-implementing the parse. The `identity` block closes that.
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("main");
        tokio::fs::create_dir_all(&agent_dir).await.unwrap();
        tokio::fs::write(
            agent_dir.join("IDENTITY.md"),
            "- **Name:** Ada\n\
             - **Role:** librarian _(edit to taste)_\n\
             - **Emoji:** 📚\n\
             - **Language:** _(preferred language for conversation)_\n",
        )
        .await
        .unwrap();

        let ctx = ctx_with_agent(tmp.path(), "main");
        let resp = handle_get(JsonRpcRequest::with_id("identity.get", None, json!(1)), ctx).await;
        assert!(resp.is_success());
        let identity = &resp.result.unwrap()["identity"];
        assert_eq!(identity["name"], "Ada");
        // The editorial aside is stripped; the seeded value survives.
        assert_eq!(identity["role"], "librarian");
        assert_eq!(identity["emoji"], "📚");
        // A bare placeholder must not be reported as a real value.
        assert!(identity["language"].is_null());
    }

    #[tokio::test]
    async fn set_then_get_roundtrips_and_takes_effect_on_disk() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_with_agent(tmp.path(), "main");

        let set_req = JsonRpcRequest::new(
            "identity.set",
            Some(json!({ "content": "You are Aleph, calm and precise." })),
            Some(json!(1)),
        );
        let set_resp = handle_set(set_req, ctx.clone()).await;
        assert!(set_resp.is_success(), "{:?}", set_resp.error);
        assert_eq!(set_resp.result.unwrap()["success"], true);

        // The write landed in the live SOUL.md the prompt reads.
        let live = tokio::fs::read_to_string(tmp.path().join("main").join("SOUL.md"))
            .await
            .unwrap();
        assert_eq!(live, "You are Aleph, calm and precise.");

        // identity.get now returns it.
        let get_resp =
            handle_get(JsonRpcRequest::with_id("identity.get", None, json!(2)), ctx).await;
        let r = get_resp.result.unwrap();
        assert_eq!(r["soul_md"], "You are Aleph, calm and precise.");
        assert_eq!(r["has_custom_identity"], true);
    }

    #[tokio::test]
    async fn set_parses_structured_preview_from_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_with_agent(tmp.path(), "main");

        let content = "---\nidentity: I am a test soul\nrelationship: peer\n---\n\n## Directives\n\n- Be helpful\n";
        handle_set(
            JsonRpcRequest::new(
                "identity.set",
                Some(json!({ "content": content })),
                Some(json!(1)),
            ),
            ctx.clone(),
        )
        .await;

        let r = handle_get(JsonRpcRequest::with_id("identity.get", None, json!(2)), ctx)
            .await
            .result
            .unwrap();
        assert_eq!(r["parsed"]["identity"], "I am a test soul");
        assert_eq!(r["parsed"]["relationship"], "peer");
    }

    #[tokio::test]
    async fn set_missing_content_is_invalid() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_with_agent(tmp.path(), "main");
        let resp = handle_set(JsonRpcRequest::with_id("identity.set", None, json!(1)), ctx).await;
        assert!(resp.is_error());
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn set_rejects_non_identity_file() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_with_agent(tmp.path(), "main");
        let resp = handle_set(
            JsonRpcRequest::new(
                "identity.set",
                Some(json!({ "content": "x", "file_name": "MEMORY.md" })),
                Some(json!(1)),
            ),
            ctx,
        )
        .await;
        assert!(resp.is_error());
        assert!(resp.error.unwrap().message.contains("remember"));
    }

    #[tokio::test]
    async fn clear_backs_up_and_removes_then_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_with_agent(tmp.path(), "main");
        tokio::fs::write(tmp.path().join("main").join("SOUL.md"), "custom")
            .await
            .unwrap();

        let resp = handle_clear(
            JsonRpcRequest::with_id("identity.clear", None, json!(1)),
            ctx.clone(),
        )
        .await;
        let r = resp.result.unwrap();
        assert_eq!(r["had_content"], true);
        assert!(!tmp.path().join("main").join("SOUL.md").exists());
        // A backup was taken.
        assert!(r["backup_path"].is_string());

        // Second clear: nothing to remove.
        let resp2 = handle_clear(
            JsonRpcRequest::with_id("identity.clear", None, json!(2)),
            ctx,
        )
        .await;
        assert_eq!(resp2.result.unwrap()["had_content"], false);
    }

    #[tokio::test]
    async fn list_reports_all_identity_files() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_with_agent(tmp.path(), "main");
        tokio::fs::write(tmp.path().join("main").join("SOUL.md"), "hi")
            .await
            .unwrap();

        let resp = handle_list(
            JsonRpcRequest::with_id("identity.list", None, json!(1)),
            ctx,
        )
        .await;
        let files = resp.result.unwrap()["files"].as_array().unwrap().clone();
        assert_eq!(files.len(), 5);
        let soul = files.iter().find(|f| f["name"] == "SOUL.md").unwrap();
        assert_eq!(soul["exists"], true);
    }

    #[tokio::test]
    async fn rejects_path_unsafe_agent_id() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_with_agent(tmp.path(), "main");
        let resp = handle_get(
            JsonRpcRequest::new(
                "identity.get",
                Some(json!({ "agent_id": "../escape" })),
                Some(json!(1)),
            ),
            ctx,
        )
        .await;
        assert!(resp.is_error());
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }
}
