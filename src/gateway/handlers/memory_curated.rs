//! Curated hot-memory RPC handlers (`memory.curated.*`).
//!
//! The third memory pillar's read/manage face. `MEMORY.md` — the always-on
//! block the `remember` tool writes and [`crate::thinker::layers`] renders
//! into every system prompt — had no RPC and no Panel surface at all: the
//! Vault tab showed notes and raw conversation rows, and the one tier the
//! user most directly curates was invisible from every client.
//!
//! ## One resolution source, and it is not this file
//!
//! The `agent_id` on the wire is the **BASE** (registered) agent id. The
//! effective per-scope storage id (`main__u-bob`, `main__proj-…`) is composed
//! by the bundle-owned `MemoryContextProvider` from the ambient
//! scope. This handler must NOT pre-compose it: `session_write_id` is not
//! idempotent under re-application, so feeding it an already-composed id
//! under an active personal scope double-composes (the same warning the
//! builtin tool registry carries at its `remember` arm). A caller that sends
//! an already-composed id is answered with the invisible-partition shape
//! below, so there is no path where a composed id reaches the resolver.
//!
//! ## Why there is no `add`
//!
//! Creating hot-tier memory is the model's job through `remember` (R7/R8) —
//! the Panel offers management (correct a wrong entry, drop a stale one), the
//! same split the notes layer already ships (no "new note" button; the drawer
//! edits and deletes what the daemon and tools produced).
//!
//! Panel-side mutations deliberately do **not** write
//! `memory_write_decisions` rows. That ledger answers "why didn't the model's
//! write land" — a human editing the file directly is not that question, and
//! filling it with operator edits would dilute the one signal `memory_trace`
//! promises.

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, SERVICE_UNAVAILABLE,
};
use crate::memory::curated::store::{CuratedError, WriteOutcome};
use crate::sync_primitives::Arc;
use crate::thinker::memory_context_provider::{CuratedRuntimeError, RuntimeMemoryContext};

/// Shared params: every curated verb is scoped by base agent id.
#[derive(Debug, Default, Deserialize)]
struct AgentParams {
    #[serde(default)]
    agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReplaceParams {
    #[serde(default)]
    agent_id: Option<String>,
    /// Substring addressing the entry to rewrite — same `match_unique`
    /// semantics the `remember` tool uses, so an operator and the model
    /// address the same entry the same way.
    old_text: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct RemoveParams {
    #[serde(default)]
    agent_id: Option<String>,
    old_text: String,
}

/// The one response shape for all three verbs: a full snapshot of the store
/// after the call. A mutation returning the new state saves the Panel a
/// second round-trip and removes the window where its list disagrees with
/// the file it just wrote.
fn snapshot_value(
    entries: &[String],
    usage_chars: usize,
    usage_pct: u8,
    limit: usize,
    legacy: bool,
    message: Option<&str>,
) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| json!({ "text": e, "chars": e.chars().count() }))
        .collect();
    let mut v = json!({
        "entries": rows,
        "usage_chars": usage_chars,
        "usage_pct": usage_pct,
        "limit": limit,
        "legacy": legacy,
    });
    if let Some(m) = message {
        v["message"] = json!(m);
    }
    v
}

fn outcome_value(outcome: &WriteOutcome) -> serde_json::Value {
    snapshot_value(
        &outcome.entries,
        outcome.usage_chars,
        outcome.usage_pct,
        outcome.limit,
        outcome.legacy,
        Some(&outcome.message),
    )
}

/// The shape an invisible partition — or a store that genuinely holds nothing
/// — produces. Deliberately identical for both, so a denied read is not an
/// existence oracle (same contract as `memory.search` / `memory.listFacts`).
///
/// `limit` comes from the process-wide `CuratedConfig`, not from the store: a
/// denied read must be byte-identical to a real empty one, and a real empty
/// one advertises the configured budget. Hard-coding `0` here would have made
/// "denied" tellable from "empty" by one field.
fn empty_value(limit: usize) -> serde_json::Value {
    snapshot_value(&[], 0, 0, limit, false, None)
}

/// `true` when this base id is addressable by the caller.
///
/// Two gates, in order:
/// 1. The id must be a BASE id. A caller-composed scope suffix would be
///    double-composed by the resolver, so it is refused here rather than
///    trusted — see the module doc.
/// 2. Standard P1 partition visibility.
fn addressable(agent_id: &str) -> bool {
    !crate::memory::project_scope::is_composed_id(agent_id)
        && crate::gateway::visibility::partition_visible(agent_id)
}

fn runtime_error(
    request_id: Option<serde_json::Value>,
    error: CuratedRuntimeError,
) -> JsonRpcResponse {
    match error {
        CuratedRuntimeError::Unavailable => JsonRpcResponse::error(
            request_id,
            SERVICE_UNAVAILABLE,
            "curated memory unavailable: Memory is not running".to_owned(),
        ),
        CuratedRuntimeError::Service(error) => JsonRpcResponse::error(
            request_id,
            INTERNAL_ERROR,
            format!("memory.curated: {error}"),
        ),
    }
}

/// Map a store error onto the caller/us split (§4.13c three-way): everything
/// the caller can fix by sending different arguments is `INVALID_PARAMS`;
/// only genuine I/O failure is ours. Folding all of these into
/// `INTERNAL_ERROR` would tell an operator to go read server logs when the
/// actual fix is "your substring matched two entries".
fn error_response(request_id: Option<serde_json::Value>, e: &CuratedError) -> JsonRpcResponse {
    let code = match e {
        CuratedError::Io(_) => INTERNAL_ERROR,
        _ => INVALID_PARAMS,
    };
    JsonRpcResponse::error(request_id, code, e.to_string())
}

fn base_agent(agent_id: Option<String>) -> String {
    agent_id
        .filter(|a| !a.trim().is_empty())
        .unwrap_or_else(|| crate::routing::DEFAULT_AGENT_ID.to_string())
}

/// `memory.curated.list` — read the hot-tier entries plus budget usage.
pub async fn handle_list(
    request: JsonRpcRequest,
    mcp: Option<Arc<RuntimeMemoryContext>>,
) -> JsonRpcResponse {
    let params: AgentParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();
    let agent = base_agent(params.agent_id);

    if !addressable(&agent) {
        let limit = mcp.as_ref().map_or(0, |m| m.curated_limit());
        return JsonRpcResponse::success(request.id, empty_value(limit));
    }

    let Some(mcp) = mcp else {
        return runtime_error(request.id, CuratedRuntimeError::Unavailable);
    };
    let snap = match mcp.curated_list(&agent).await {
        Ok(snapshot) => snapshot,
        Err(error) => return runtime_error(request.id, error),
    };
    JsonRpcResponse::success(
        request.id,
        snapshot_value(
            &snap.entries,
            snap.usage_chars,
            snap.usage_pct,
            snap.limit,
            snap.legacy,
            None,
        ),
    )
}

/// `memory.curated.replace` — rewrite the single entry matching `old_text`.
pub async fn handle_replace(
    request: JsonRpcRequest,
    mcp: Option<Arc<RuntimeMemoryContext>>,
) -> JsonRpcResponse {
    let params: ReplaceParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let agent = base_agent(params.agent_id);
    if !addressable(&agent) {
        // Same no-oracle contract as the read: a denied write reports the
        // "no such entry" the caller would get for a genuinely absent one.
        return error_response(request.id, &CuratedError::NoMatch(params.old_text.clone()));
    }

    let Some(mcp) = mcp else {
        return runtime_error(request.id, CuratedRuntimeError::Unavailable);
    };
    match mcp
        .curated_replace(&agent, params.old_text, params.content)
        .await
    {
        Ok(Ok(outcome)) => JsonRpcResponse::success(request.id, outcome_value(&outcome)),
        Ok(Err(error)) => error_response(request.id, &error),
        Err(error) => runtime_error(request.id, error),
    }
}

/// `memory.curated.remove` — drop the single entry matching `old_text`.
pub async fn handle_remove(
    request: JsonRpcRequest,
    mcp: Option<Arc<RuntimeMemoryContext>>,
) -> JsonRpcResponse {
    let params: RemoveParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let agent = base_agent(params.agent_id);
    if !addressable(&agent) {
        return error_response(request.id, &CuratedError::NoMatch(params.old_text.clone()));
    }

    let Some(mcp) = mcp else {
        return runtime_error(request.id, CuratedRuntimeError::Unavailable);
    };
    match mcp.curated_remove(&agent, params.old_text).await {
        Ok(Ok(outcome)) => JsonRpcResponse::success(request.id, outcome_value(&outcome)),
        Ok(Err(error)) => error_response(request.id, &error),
        Err(error) => runtime_error(request.id, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::memory::MemoryInjectionMode;
    use serde_json::json;

    const LIMIT: usize = 200;

    async fn provider(
        root: &std::path::Path,
    ) -> (
        Arc<RuntimeMemoryContext>,
        Arc<crate::memory::runtime::MemoryRuntime>,
    ) {
        use crate::capability::ownership::OwnershipTree;
        use crate::config::types::memory::MemoryConfig;
        use crate::memory::extensions::MemoryExtensionRegistry;
        use crate::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig, MemoryServicesConfig};
        use crate::memory::store::markdown::MarkdownSessionStore;

        let raw = Arc::new(
            MarkdownSessionStore::new(&root.join("runtime"))
                .await
                .unwrap(),
        );
        let mut memory = MemoryConfig::default();
        memory.injection_mode = MemoryInjectionMode::Context;
        memory.project_scoped = false;
        memory.curated.memory_char_limit = LIMIT;
        memory.curated.user_char_limit = LIMIT;
        memory.embedding.providers.clear();
        let runtime = MemoryRuntime::new_with_services(
            MemoryRuntimeConfig {
                metadata_path: root.join("runtime/metadata.db"),
                note_memory_dir: root.join("runtime/note"),
            },
            Arc::new(OwnershipTree::new()),
            MemoryServicesConfig {
                app_config: Arc::new(tokio::sync::RwLock::new(crate::Config {
                    memory: memory.clone(),
                    ..Default::default()
                })),
                raw_store: raw,
                session_store: None,
                provider: None,
                extensions: Arc::new(MemoryExtensionRegistry::new()),
                state_db: None,
                default_agent_id: "main".into(),
                curated_root: root.to_owned(),
                snapshot_dir: root.join("runtime/snapshots"),
            },
        );
        runtime.start().await.unwrap();
        (
            Arc::new(RuntimeMemoryContext::new(runtime.clone())),
            runtime,
        )
    }

    async fn seed(root: &std::path::Path, agent: &str, body: &str) {
        let dir = root.join(agent);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("MEMORY.md"), body).await.unwrap();
    }

    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), json!(1))
    }

    fn texts(value: &serde_json::Value) -> Vec<String> {
        value["entries"]
            .as_array()
            .expect("entries array")
            .iter()
            .map(|e| e["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[tokio::test]
    async fn list_reports_entries_and_budget_usage() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main", "likes tea\n§\nships on fridays\n§\n").await;
        let (mcp, runtime) = provider(dir.path()).await;

        let resp = handle_list(
            req("memory.curated.list", json!({ "agent_id": "main" })),
            Some(mcp),
        )
        .await;

        assert!(resp.is_success(), "{:?}", resp.error);
        let v = resp.result.unwrap();
        assert_eq!(texts(&v), vec!["likes tea", "ships on fridays"]);
        assert_eq!(v["limit"], json!(LIMIT));
        assert!(v["usage_chars"].as_u64().unwrap() > 0);
        assert_eq!(v["legacy"], json!(false));
        // Per-entry char counts let the Panel show which entry costs what
        // without re-deriving the store's own char (not byte) accounting.
        assert_eq!(v["entries"][0]["chars"], json!("likes tea".chars().count()));
        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn replace_rewrites_the_matched_entry_and_returns_the_new_state() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main", "likes tea\n§\nships on fridays\n§\n").await;
        let (mcp, runtime) = provider(dir.path()).await;

        let resp = handle_replace(
            req(
                "memory.curated.replace",
                json!({ "agent_id": "main", "old_text": "tea", "content": "likes coffee" }),
            ),
            Some(mcp.clone()),
        )
        .await;

        assert!(resp.is_success(), "{:?}", resp.error);
        // The mutation answers with the full new snapshot: no second
        // round-trip, and no window where the Panel's list disagrees with
        // the file it just wrote.
        assert_eq!(
            texts(&resp.result.unwrap()),
            vec!["likes coffee", "ships on fridays"]
        );

        // And it really hit the disk, not just the in-memory copy.
        let on_disk = tokio::fs::read_to_string(dir.path().join("main").join("MEMORY.md"))
            .await
            .unwrap();
        assert!(on_disk.contains("likes coffee"), "{on_disk}");
        assert!(!on_disk.contains("likes tea"), "{on_disk}");
        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn remove_drops_the_matched_entry() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main", "likes tea\n§\nships on fridays\n§\n").await;
        let (mcp, runtime) = provider(dir.path()).await;

        let resp = handle_remove(
            req(
                "memory.curated.remove",
                json!({ "agent_id": "main", "old_text": "fridays" }),
            ),
            Some(mcp),
        )
        .await;

        assert!(resp.is_success(), "{:?}", resp.error);
        assert_eq!(texts(&resp.result.unwrap()), vec!["likes tea"]);
        runtime.stop().await.unwrap();
    }

    /// The single most load-bearing property of this handler family: the
    /// Panel must read and write the SAME `MEMORY.md` the `remember` tool
    /// does. Both go through `get_or_load_curated_store`, so they resolve
    /// scope with one function — this pins that the handler did not grow its
    /// own composition (which would be a second, drifting resolver).
    #[tokio::test]
    async fn the_rpc_and_the_remember_tool_resolve_the_same_store() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main", "written by the tool\n§\n").await;
        let (mcp, runtime) = provider(dir.path()).await;

        let tool =
            crate::builtin_tools::runtime::RuntimeTool::<crate::builtin_tools::RememberTool>::new(
                runtime.clone(),
            );
        let written = tool
            .call_for_agent(
                "main",
                crate::builtin_tools::remember::RememberArgs::Add {
                    content: "I prefer tea in the evening.".into(),
                },
            )
            .await
            .unwrap();

        let resp = handle_list(
            req("memory.curated.list", json!({ "agent_id": "main" })),
            Some(mcp.clone()),
        )
        .await;
        let listed = texts(&resp.result.unwrap());

        assert_eq!(listed, written.entries);
        assert!(listed.contains(&"I prefer tea in the evening.".to_owned()));
        runtime.stop().await.unwrap();
    }

    /// Caller-composed ids are refused rather than trusted: the resolver
    /// would compose AGAIN (`main__u-bob__u-bob`), landing on a phantom store
    /// nobody writes. The refusal is the same shape an empty store produces.
    #[tokio::test]
    async fn a_pre_composed_id_reads_as_an_empty_store_not_a_phantom_one() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main__u-bob", "bob's fact\n§\n").await;
        let (mcp, runtime) = provider(dir.path()).await;

        let resp = handle_list(
            req("memory.curated.list", json!({ "agent_id": "main__u-bob" })),
            Some(mcp),
        )
        .await;

        assert!(resp.is_success(), "denial is a shape, not an error");
        let v = resp.result.unwrap();
        assert!(texts(&v).is_empty());
        // Byte-identical to a real empty store, budget included — otherwise
        // "denied" would be tellable from "empty" by one field.
        assert_eq!(v["limit"], json!(LIMIT));
        assert_eq!(v["usage_chars"], json!(0));
        assert_eq!(v["legacy"], json!(false));
        runtime.stop().await.unwrap();
    }

    /// A foreign partition must not be reachable by naming it, and the
    /// mutation verbs must not leak existence either: both answer with the
    /// same "no entry matched" an absent entry produces.
    #[tokio::test]
    async fn foreign_partition_mutations_answer_like_a_missing_entry() {
        use crate::gateway::caller_identity::CALLER_USER;

        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main__u-alice", "alice's fact\n§\n").await;
        let (mcp, runtime) = provider(dir.path()).await;

        let resp = CALLER_USER
            .scope(
                Some("bob".to_string()),
                handle_remove(
                    req(
                        "memory.curated.remove",
                        json!({ "agent_id": "main__u-alice", "old_text": "alice" }),
                    ),
                    Some(mcp),
                ),
            )
            .await;

        assert!(!resp.is_success());
        let err = resp.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(err.message.contains("no entry matched"), "{}", err.message);

        // Untouched.
        let on_disk = tokio::fs::read_to_string(dir.path().join("main__u-alice").join("MEMORY.md"))
            .await
            .unwrap();
        assert!(on_disk.contains("alice's fact"));
        runtime.stop().await.unwrap();
    }

    /// Everything the caller can fix by sending different arguments is
    /// INVALID_PARAMS, not INTERNAL_ERROR — an operator whose substring
    /// matched two entries should not be sent to read server logs.
    #[tokio::test]
    async fn ambiguous_and_missing_substrings_are_caller_errors() {
        let dir = tempfile::tempdir().unwrap();
        seed(
            dir.path(),
            "main",
            "ships on fridays\n§\nships on mondays\n§\n",
        )
        .await;
        let (mcp, runtime) = provider(dir.path()).await;

        let ambiguous = handle_remove(
            req(
                "memory.curated.remove",
                json!({ "agent_id": "main", "old_text": "ships on" }),
            ),
            Some(mcp.clone()),
        )
        .await;
        assert_eq!(ambiguous.error.unwrap().code, INVALID_PARAMS);

        let missing = handle_remove(
            req(
                "memory.curated.remove",
                json!({ "agent_id": "main", "old_text": "nothing like this" }),
            ),
            Some(mcp),
        )
        .await;
        assert_eq!(missing.error.unwrap().code, INVALID_PARAMS);
        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn off_rejects_curated_reads_and_writes_and_restart_uses_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main", "likes tea\n§\n").await;
        let (mcp, runtime) = provider(dir.path()).await;
        let file = dir.path().join("main/MEMORY.md");
        let before = tokio::fs::read(&file).await.unwrap();
        runtime.stop().await.unwrap();
        let listed = handle_list(
            req("memory.curated.list", json!({"agent_id": "main"})),
            Some(mcp.clone()),
        )
        .await;
        assert_eq!(listed.error.unwrap().code, SERVICE_UNAVAILABLE);
        let replaced = handle_replace(
            req(
                "memory.curated.replace",
                json!({
                    "agent_id": "main", "old_text": "tea", "content": "likes coffee"
                }),
            ),
            Some(mcp.clone()),
        )
        .await;
        assert_eq!(replaced.error.unwrap().code, SERVICE_UNAVAILABLE);
        assert_eq!(tokio::fs::read(&file).await.unwrap(), before);
        runtime.start().await.unwrap();
        let listed = handle_list(
            req("memory.curated.list", json!({"agent_id": "main"})),
            Some(mcp),
        )
        .await;
        assert_eq!(texts(&listed.result.unwrap()), vec!["likes tea"]);
        runtime.stop().await.unwrap();
    }

    /// No agent runtime is not "empty memory". Answering with an empty list
    /// would invite the user to conclude their hot tier had been wiped.
    #[tokio::test]
    async fn a_gateway_with_no_agent_runtime_says_so() {
        let resp = handle_list(
            req("memory.curated.list", json!({ "agent_id": "main" })),
            None,
        )
        .await;

        assert!(!resp.is_success());
        assert_eq!(resp.error.unwrap().code, SERVICE_UNAVAILABLE);
    }

    /// A mutation must evict the frozen per-session envelope, or an entry
    /// corrected in the Panel keeps being injected in its old wording for the
    /// rest of every already-open conversation.
    #[tokio::test]
    async fn a_mutation_evicts_the_frozen_prompt_envelope() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main", "likes tea\n§\n").await;
        let (mcp, runtime) = provider(dir.path()).await;

        let before = mcp.build_curated_message("main", "ses-1").await.unwrap();
        assert!(format!("{before:?}").contains("likes tea"));

        let resp = handle_replace(
            req(
                "memory.curated.replace",
                json!({ "agent_id": "main", "old_text": "tea", "content": "likes coffee" }),
            ),
            Some(mcp.clone()),
        )
        .await;
        assert!(resp.is_success(), "{:?}", resp.error);

        let after = mcp.build_curated_message("main", "ses-1").await.unwrap();
        let rendered = format!("{after:?}");
        assert!(rendered.contains("likes coffee"), "{rendered}");
        assert!(!rendered.contains("likes tea"), "{rendered}");
        runtime.stop().await.unwrap();
    }
}
