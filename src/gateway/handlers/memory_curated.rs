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
//! by [`MemoryContextProvider::get_or_load_curated_store`] from the ambient
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
use crate::memory::curated::store::{CuratedError, CuratedMemoryStore, WriteOutcome};
use crate::sync_primitives::Arc;
use crate::thinker::MemoryContextProvider;

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

/// Resolve the per-scope store, or an error response describing why not.
async fn resolve_store(
    request_id: &Option<serde_json::Value>,
    mcp: Option<&Arc<MemoryContextProvider>>,
    agent_id: &str,
) -> Result<Arc<CuratedMemoryStore>, JsonRpcResponse> {
    let Some(mcp) = mcp else {
        // No agent runtime in this process (a bare gateway). Distinct from
        // "empty memory": say so rather than serve a convincing empty list.
        return Err(JsonRpcResponse::error(
            request_id.clone(),
            SERVICE_UNAVAILABLE,
            "curated memory unavailable: no agent runtime in this process".to_string(),
        ));
    };
    mcp.get_or_load_curated_store(agent_id).await.map_err(|e| {
        JsonRpcResponse::error(
            request_id.clone(),
            INTERNAL_ERROR,
            format!("memory.curated: {e}"),
        )
    })
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
    mcp: Option<Arc<MemoryContextProvider>>,
) -> JsonRpcResponse {
    let params: AgentParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();
    let agent = base_agent(params.agent_id);

    if !addressable(&agent) {
        let limit = mcp
            .as_ref()
            .map_or(0, |m| m.curated_config.memory_char_limit);
        return JsonRpcResponse::success(request.id, empty_value(limit));
    }

    let store = match resolve_store(&request.id, mcp.as_ref(), &agent).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let snap = store.snapshot_outcome(String::new());
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
    mcp: Option<Arc<MemoryContextProvider>>,
) -> JsonRpcResponse {
    let params: ReplaceParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let agent = base_agent(params.agent_id);
    if !addressable(&agent) {
        // Same no-oracle contract as the read: a denied write reports the
        // "no such entry" the caller would get for a genuinely absent one.
        return error_response(
            request.id,
            &CuratedError::NoMatch(params.old_text.clone()),
        );
    }

    let store = match resolve_store(&request.id, mcp.as_ref(), &agent).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match store.replace(&params.old_text, &params.content).await {
        Ok(outcome) => {
            invalidate(mcp.as_ref(), &store).await;
            JsonRpcResponse::success(request.id, outcome_value(&outcome))
        }
        Err(e) => error_response(request.id, &e),
    }
}

/// `memory.curated.remove` — drop the single entry matching `old_text`.
pub async fn handle_remove(
    request: JsonRpcRequest,
    mcp: Option<Arc<MemoryContextProvider>>,
) -> JsonRpcResponse {
    let params: RemoveParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let agent = base_agent(params.agent_id);
    if !addressable(&agent) {
        return error_response(
            request.id,
            &CuratedError::NoMatch(params.old_text.clone()),
        );
    }

    let store = match resolve_store(&request.id, mcp.as_ref(), &agent).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match store.remove(&params.old_text).await {
        Ok(outcome) => {
            invalidate(mcp.as_ref(), &store).await;
            JsonRpcResponse::success(request.id, outcome_value(&outcome))
        }
        Err(e) => error_response(request.id, &e),
    }
}

/// Evict the frozen per-session curated envelope after a mutation.
///
/// `build_curated_message` freezes a rendered snapshot per (resolved id,
/// session) and reuses it for that session's whole lifetime (§2.18 prefix
/// stability). Without this eviction, an entry corrected in the Panel keeps
/// being injected in its old wording into every already-open conversation —
/// the file changes and the prompt does not.
///
/// The key is the **resolved** storage id, not the base id the caller sent
/// (`invalidate_curated_for_agent`'s doc says so explicitly). It is read back
/// off the store we just wrote through rather than re-derived from the base
/// id: the store was built by the resolver and carries the answer, and a
/// second derivation here would be a second source that can disagree with it.
async fn invalidate(mcp: Option<&Arc<MemoryContextProvider>>, store: &Arc<CuratedMemoryStore>) {
    if let Some(mcp) = mcp {
        mcp.invalidate_curated_for_agent(&store.agent_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::memory::MemoryInjectionMode;
    use crate::memory::curated::CuratedConfig;
    use serde_json::json;

    const LIMIT: usize = 200;

    fn provider(root: &std::path::Path) -> Arc<MemoryContextProvider> {
        Arc::new(
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
                .with_curated_config(CuratedConfig {
                    memory_char_limit: LIMIT,
                    user_char_limit: LIMIT,
                    ..CuratedConfig::default()
                })
                .with_curated_root_for_test(root.to_path_buf()),
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
        let mcp = provider(dir.path());

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
    }

    #[tokio::test]
    async fn replace_rewrites_the_matched_entry_and_returns_the_new_state() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main", "likes tea\n§\nships on fridays\n§\n").await;
        let mcp = provider(dir.path());

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
    }

    #[tokio::test]
    async fn remove_drops_the_matched_entry() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main", "likes tea\n§\nships on fridays\n§\n").await;
        let mcp = provider(dir.path());

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
        let mcp = provider(dir.path());

        // What the tool's registry arm does, verbatim: base id in, resolver
        // composes the scope.
        let tool_store = mcp.get_or_load_curated_store("main").await.unwrap();

        let resp = handle_list(
            req("memory.curated.list", json!({ "agent_id": "main" })),
            Some(mcp.clone()),
        )
        .await;
        let listed = texts(&resp.result.unwrap());

        assert_eq!(listed, tool_store.current_entries());
        assert_eq!(listed, vec!["written by the tool"]);
    }

    /// Caller-composed ids are refused rather than trusted: the resolver
    /// would compose AGAIN (`main__u-bob__u-bob`), landing on a phantom store
    /// nobody writes. The refusal is the same shape an empty store produces.
    #[tokio::test]
    async fn a_pre_composed_id_reads_as_an_empty_store_not_a_phantom_one() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main__u-bob", "bob's fact\n§\n").await;
        let mcp = provider(dir.path());

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
    }

    /// A foreign partition must not be reachable by naming it, and the
    /// mutation verbs must not leak existence either: both answer with the
    /// same "no entry matched" an absent entry produces.
    #[tokio::test]
    async fn foreign_partition_mutations_answer_like_a_missing_entry() {
        use crate::gateway::caller_identity::CALLER_USER;

        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main__u-alice", "alice's fact\n§\n").await;
        let mcp = provider(dir.path());

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
    }

    /// Everything the caller can fix by sending different arguments is
    /// INVALID_PARAMS, not INTERNAL_ERROR — an operator whose substring
    /// matched two entries should not be sent to read server logs.
    #[tokio::test]
    async fn ambiguous_and_missing_substrings_are_caller_errors() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "main", "ships on fridays\n§\nships on mondays\n§\n").await;
        let mcp = provider(dir.path());

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
        let mcp = provider(dir.path());

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
    }
}
