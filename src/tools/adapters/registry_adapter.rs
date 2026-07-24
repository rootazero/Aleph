//! Adapter bridging `executor::ToolRegistry` to `LoopTool`.
//!
//! Wraps `ToolRegistry::execute_tool()` + `UnifiedTool` metadata into
//! `LoopTool` instances for use in the agent loop.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::executor::ToolRegistry;
use crate::tool_metadata::UnifiedTool;

use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};

/// A `LoopTool` backed by a shared `ToolRegistry`.
///
/// Each instance holds the metadata from a `UnifiedTool` (name, description,
/// schema) and delegates execution to `ToolRegistry::execute_tool()`.
struct RegistryToolAdapter<R: ToolRegistry + 'static> {
    name: String,
    description: String,
    schema: Value,
    registry: Arc<R>,
    /// Default working directory for `bash/code_exec` tools (agent workspace)
    default_working_dir: Arc<Option<String>>,
}

/// Tools that should have `working_dir` injected when not specified by LLM
const WORKING_DIR_TOOLS: &[&str] = &["bash", "code_exec"];

/// Builtin tools that are side-effect-free for scheduling purposes: their
/// execution neither mutates shared state nor depends on another in-flight
/// call's effects, so any number may run concurrently
/// ([`crate::tools::concurrency::ConcurrencyClaim::Shared`]).
///
/// This is the **safe-default inversion** of the former `EXCLUSIVE_TOOLS`
/// mutating-denylist. Under the denylist, a tool was concurrent-safe *unless*
/// explicitly listed — so any mutator a contributor forgot to add (e.g.
/// `team_disband`, which is even confirmation-required, or `team_member_add` /
/// `heartbeat_update` / `skill_install`) silently became `Shared` and could
/// race another call in the same parallel batch. The allowlist flips the
/// failure mode: a tool parallelizes only if it is *explicitly* known read-only;
/// every other tool — including any future mutator added without touching this
/// file — defaults to whole-world [`ConcurrencyClaim::global`], so a forgotten
/// entry costs parallelism (serial, still correct) instead of risking a race.
///
/// Mirrors hermes-agent's `_PARALLEL_SAFE_TOOLS` allowlist, but pairs it with
/// Aleph's sound path-overlap partition ([`crate::tools::concurrency`]): the
/// path-scoped file mutators below ([`bounded_file_writer_path`] + `file_ops`)
/// still parallelize on disjoint paths via [`RegistryToolAdapter::concurrency_claim`],
/// which no reference agent does. Keep this list conservative — only add a tool
/// once its read-only-ness is certain. Shared mutable state (browser session,
/// agent/session lifecycle) is intentionally absent, so those tools serialize.
/// Every name on this list must be a REGISTERED tool: phantom entries are
/// pure noise that rots trust in the list (a 2026-07 sweep removed five
/// never-registered names — `memory_recall`, `web_search`, `knowledge`,
/// `list_tools`, `search_tools` — plus the stale `skill_reader` spelling from
/// the idempotency side). Since 2026-07-17 this list is ALSO the single
/// source of builtin idempotency (`crate::tools::retry::is_idempotent_builtin_name`
/// delegates here), so membership feeds three consumers: the `Shared`
/// concurrency claim, the one-shot auto-retry gate, and the `Ask` exec
/// tier's read exemption. Input-dependent tools (`file_ops`, `doctor`,
/// `note_schema`) must NOT be listed even when their common mode is a read —
/// they resolve per-argument in [`RegistryToolAdapter::concurrency_claim`]
/// and stay non-idempotent (their write arm is what the Ask tier must gate).
pub(crate) const READ_ONLY_TOOLS: &[&str] = &[
    // Introspection / catalog (pure reads). (`a2a_agents` is NOT one: its
    // `add`/`remove` actions mutate the remote-agent trust surface — it
    // resolves per-argument in `a2a_agents_claim`.)
    "agent_info",
    "agent_list",
    "config_audit",
    "get_tool_schema",
    "list_models",
    // Progressive-disclosure meta-tool (reads the catalog, promotes a deferred
    // tool). NB: `list_tools` / `search_tools` were ghost names matching no
    // registered tool; the live one is `tool_search`.
    "tool_search",
    "read_config_guide",
    "node_list",
    // Search / retrieval (no mutation).
    "search",
    "web_fetch",
    "ctx_search",
    "document_extract",
    // File reads (writers are path-scoped, see `bounded_file_writer_path`).
    "file_read",
    // Memory / context reads.
    "memory_search",
    "memory_browse",
    "memory_explore",
    "memory_timeline",
    "recall_context",
    "recall_events",
    // Governance audit reality probe (correction + dreaming counts). Pure read.
    "governance_metrics",
    "user_profile",
    // Session / inbox reads. (`inbox_read` is NOT one: `mark_read` defaults
    // to true, so a read CONSUMES unread state — an auto-retry after a
    // timed-out first attempt would return an empty view of messages the
    // first attempt already consumed. It resolves per-argument in
    // `inbox_read_claim`; its pure `peek`/`count_only` modes still
    // parallelize.)
    "session_list",
    "session_read",
    "session_search",
    // Task reads.
    "task_list",
    "task_read_artifact",
    // Team reads.
    "team_status",
    "team_digest",
    "team_usage",
    // Heartbeat reads. (`heartbeat_report` is NOT one, despite the name: it
    // is the L2 output gate whose `action="notify"` sends the user a message
    // — auto-retrying it after a timeout would double-notify.)
    "heartbeat_list",
    // Skill reads.
    "skill_list",
    "skill_read",
    "skill_status",
    // Note discovery (pure reads; `note_schema` is read/write and resolves
    // per-argument instead — see `note_schema_claim`).
    "note_orient",
    "note_graph_query",
    // MCP capability reads (bridge builtins; the servers' own tools declare
    // safety via `readOnlyHint` through `McpRegistryTool` instead).
    "mcp_read_resource",
    "mcp_list_resources",
    "mcp_list_resource_templates",
    "mcp_get_prompt",
    "mcp_list_prompts",
    // Desktop accessibility queries (read-only inspection of the UI tree).
    "desktop_ax_query_by_role",
    "desktop_ax_query_focused",
    "desktop_ax_query_tree",
    "desktop_ax_snapshot",
    "desktop_som",
    "desktop_gui_locate",
    "desktop_check_permissions",
];

/// Builtin tools that require explicit user confirmation before they run.
///
/// Destructive or security-sensitive operations (secret-vault mutation,
/// agent deletion, team disband). The adapter reports this through
/// [`LoopTool::requires_confirmation`], so the live `ScopedToolService`
/// confirmation gate routes a user prompt before dispatch — no gateway-side
/// allowlist needed. Co-located with [`READ_ONLY_TOOLS`]: both are per-tool
/// runtime dispatch properties this adapter self-declares. Mirrors the
/// `AlephTool::requires_confirmation()` overrides on these tools (which feed
/// the metadata/describe path); keep the two in sync.
pub(crate) const CONFIRMATION_REQUIRED_TOOLS: &[&str] =
    &["vault_store", "agent_delete", "team_disband"];

/// Extract the bounded target path for a single-path file mutator
/// (`file_write` / `file_edit`). Returns `None` for any other tool (including
/// read-only `file_read`, which is in [`READ_ONLY_TOOLS`] and keeps the
/// `Shared` claim, and multi-file `apply_patch`, whose footprint is parsed
/// from its patch body by [`apply_patch_claim`]). Used by
/// [`RegistryToolAdapter::concurrency_claim`] to give these mutators a
/// same-path-serializing scope so writes to different files parallelize while
/// writes to the same file serialize.
fn bounded_file_writer_path(name: &str, input: &Value) -> Option<String> {
    let candidates: &[&str] = match name {
        "file_write" | "file_edit" => &["file_path", "path"],
        _ => return None,
    };
    candidates
        .iter()
        .find_map(|field| input.get(*field).and_then(Value::as_str))
        .map(str::to_string)
}

/// Resolve the [`crate::tools::concurrency::ConcurrencyClaim`] for a `file_ops`
/// call from its `operation` discriminant. Read-only operations are `Shared`;
/// mutating operations bind to their concrete `path` (+ `destination`) so
/// disjoint-path operations parallelize; an unknown/unparseable operation
/// falls back to whole-world exclusive.
fn file_ops_claim(input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
    use crate::tools::concurrency::ConcurrencyClaim;
    let op = input.get("operation").and_then(Value::as_str).unwrap_or("");
    match op {
        "list" | "search" | "stats" => ConcurrencyClaim::Shared,
        "move" | "copy" | "delete" | "mkdir" | "batch_move" | "organize" => {
            let mut paths = Vec::with_capacity(2);
            if let Some(p) = input.get("path").and_then(Value::as_str) {
                paths.push(p.to_string());
            }
            if let Some(d) = input.get("destination").and_then(Value::as_str) {
                paths.push(d.to_string());
            }
            // `ConcurrencyClaim::paths` degrades to whole-world exclusive when
            // no concrete path could be extracted.
            ConcurrencyClaim::paths(paths)
        }
        _ => ConcurrencyClaim::global(),
    }
}

/// Resolve the [`crate::tools::concurrency::ConcurrencyClaim`] for an
/// `apply_patch` call from the files its V4A envelope will touch.
///
/// `apply_patch` is the only multi-file mutator: it carries a whole patch body
/// and *no* single path argument, so its blast radius is parsed from the
/// envelope (every Add/Delete/Update target + any `*** Move to:` destination)
/// via [`patch_target_paths`](crate::builtin_tools::file_ops::patch_target_paths)
/// — the same parser the executor runs, so the scheduled footprint cannot drift
/// from the files actually mutated. Binding to that set lets disjoint patches
/// parallelize while patches sharing a file serialize.
///
/// `ConcurrencyClaim::paths` degrades to whole-world exclusive when the patch
/// names no extractable path (missing `patch` field, empty, or unparseable), so
/// a malformed patch serializes against everything rather than racing.
fn apply_patch_claim(input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
    use crate::tools::concurrency::ConcurrencyClaim;
    let Some(patch) = input.get("patch").and_then(Value::as_str) else {
        return ConcurrencyClaim::global();
    };
    ConcurrencyClaim::paths(crate::builtin_tools::file_ops::patch_target_paths(patch))
}

/// Resolve the claim for a `doctor` call from its `fix` flag. `fix=true`
/// applies repairs (recreates the data dir, clears a stale instance lock —
/// see `builtin_tools::doctor`), so it is whole-world exclusive; the default
/// inspect-only posture is a pure read and parallelizes. `doctor` must NOT
/// sit on [`READ_ONLY_TOOLS`] — a fixing doctor inside a parallel read batch
/// was exactly the misclassification that rule exists to prevent.
fn doctor_claim(input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
    use crate::tools::concurrency::ConcurrencyClaim;
    if input.get("fix").and_then(Value::as_bool).unwrap_or(false) {
        ConcurrencyClaim::global()
    } else {
        ConcurrencyClaim::Shared
    }
}

/// Resolve the claim for a `note_schema` call from its `action` tag.
/// `read` is a pure fetch of SCHEMA.md; `write` (and anything unrecognized)
/// replaces it and serializes against the world. Mirrors [`file_ops_claim`]:
/// one tool name multiplexing read and write modes resolves per-argument.
fn note_schema_claim(input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
    use crate::tools::concurrency::ConcurrencyClaim;
    match input.get("action").and_then(Value::as_str) {
        Some("read") => ConcurrencyClaim::Shared,
        _ => ConcurrencyClaim::global(),
    }
}

/// Resolve the claim for an `a2a_agents` call from its `action` tag. `list`
/// is a pure registry read; `add` / `remove` (and anything unrecognized)
/// mutate the remote-agent trust surface and serialize against the world.
fn a2a_agents_claim(input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
    use crate::tools::concurrency::ConcurrencyClaim;
    match input.get("action").and_then(Value::as_str) {
        Some("list") => ConcurrencyClaim::Shared,
        _ => ConcurrencyClaim::global(),
    }
}

/// Resolve the claim for an `inbox_read` call. `peek` / `count_only` modes
/// are pure reads; the default mode CONSUMES unread state (`mark_read`
/// defaults to true), so it serializes.
fn inbox_read_claim(input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
    use crate::tools::concurrency::ConcurrencyClaim;
    let flag = |name: &str| input.get(name).and_then(Value::as_bool).unwrap_or(false);
    if flag("peek")
        || flag("count_only")
        || matches!(input.get("mark_read"), Some(Value::Bool(false)))
    {
        ConcurrencyClaim::Shared
    } else {
        ConcurrencyClaim::global()
    }
}

/// Resolve the claim for a `session_send` call from its target session key.
/// `session_send` blocks up to `timeout_seconds` waiting on the child run, so
/// a fan-out of delegations to *distinct* sessions paid N × wait serially
/// under the old blanket `Global` claim. Binding to the target lets disjoint
/// delegations run concurrently ([`ExclusiveScope::Sessions`] conservatively
/// conflicts with every non-session scope, so this only ever parallelizes
/// sibling delegations).
///
/// The claim keys on the **resolved gateway session key**
/// ([`crate::builtin_tools::sessions::send_tool::claim_session_key`] — the
/// same parse + gateway collapse the send executes under), NOT the raw
/// spelling: aliased spellings that execute in one session must yield one
/// claim key or they would wrongly parallelize. A missing / empty / invalid
/// key (server-side default target, unresolvable here) → `Global`.
fn session_send_claim(input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
    use crate::tools::concurrency::ConcurrencyClaim;
    match input
        .get("session_key")
        .and_then(Value::as_str)
        .and_then(crate::builtin_tools::sessions::send_tool::claim_session_key)
    {
        Some(key) => ConcurrencyClaim::sessions(std::iter::once(key)),
        None => ConcurrencyClaim::global(),
    }
}

#[async_trait]
impl<R: ToolRegistry + 'static> LoopTool for RegistryToolAdapter<R> {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        // Safe default: only explicitly-known read-only tools are freely
        // concurrent. Path-scoped file writers are not "freely" concurrent
        // (they conflict on overlapping paths), so they report `false` here —
        // their bounded parallelism is expressed through `concurrency_claim`,
        // not this coarse boolean. Everything unlisted is exclusive.
        READ_ONLY_TOOLS.contains(&self.name.as_str())
    }

    fn concurrency_claim(&self, input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
        use crate::tools::concurrency::ConcurrencyClaim;
        let name = self.name.as_str();
        // `file_ops` multiplexes read-only (list/search/stats → `Shared`) and
        // mutating (move/copy/delete/… → bounded `Paths`) operations off its
        // `operation` discriminant, so it is resolved before the allowlist.
        if name == "file_ops" {
            return file_ops_claim(input);
        }
        // `apply_patch` carries a multi-file V4A envelope and no single path
        // field; its blast radius is every file the patch body touches, parsed
        // from the envelope. Bind to that set so disjoint patches parallelize
        // while patches sharing a file serialize.
        if name == "apply_patch" {
            return apply_patch_claim(input);
        }
        // Input-dependent read/write multiplexers resolve per-argument, like
        // `file_ops` above: `doctor` flips on `fix`, `note_schema` and
        // `a2a_agents` on their `action` tag, `inbox_read` on whether the
        // call consumes unread state.
        if name == "doctor" {
            return doctor_claim(input);
        }
        if name == "note_schema" {
            return note_schema_claim(input);
        }
        if name == "a2a_agents" {
            return a2a_agents_claim(input);
        }
        if name == "inbox_read" {
            return inbox_read_claim(input);
        }
        // `session_send` blocks on the named target session; disjoint fan-out
        // delegations parallelize under a bounded `Sessions` scope.
        if name == "session_send" {
            return session_send_claim(input);
        }
        // `file_write` / `file_edit` mutate exactly their target path. Bind them
        // to that concrete path so two writes to the same file serialize while
        // writes to different files parallelize.
        if let Some(path) = bounded_file_writer_path(name, input) {
            return ConcurrencyClaim::paths(std::iter::once(path));
        }
        // `node_invoke` drives exactly one cluster node, named right there in the
        // args. Bind it to that node so a batch touching different machines runs
        // concurrently (the whole point of owning a fleet) while two calls into
        // the SAME node serialize — they share its one bash session workspace.
        //
        // Deliberately NOT extended to the other two cluster tools:
        //   * `node_invoke_many` selects by TAG, so its footprint is whatever the
        //     registry currently matches — not derivable from `input` alone.
        //   * `node_file` straddles a center-local path AND a node.
        // Both keep the conservative `Global` claim below.
        if name == "node_invoke" {
            if let Some(node) = input.get("node").and_then(Value::as_str) {
                return ConcurrencyClaim::nodes(std::iter::once(node));
            }
        }
        // Safe default: read-only allowlist → `Shared`; everything else
        // (known mutators AND any unlisted/unknown tool) → whole-world
        // exclusive. A forgotten mutator serializes (correct) rather than
        // racing, which is the whole point of the allowlist inversion.
        if READ_ONLY_TOOLS.contains(&name) {
            ConcurrencyClaim::Shared
        } else {
            ConcurrencyClaim::global()
        }
    }

    fn requires_confirmation(&self) -> bool {
        CONFIRMATION_REQUIRED_TOOLS.contains(&self.name.as_str())
    }

    fn is_idempotent(&self) -> bool {
        // The maintained builtin pure-read allowlist is the single source of
        // truth for builtin idempotency; unlisted builtins stay `false`.
        crate::tools::retry::is_idempotent_builtin_name(&self.name)
    }

    async fn execute(&self, input: Value, cancel: CancellationToken) -> ToolResult {
        tracing::debug!(tool = %self.name, args = %input, "Tool call raw arguments from LLM");
        // Inject default working_dir for bash/code_exec if not provided by LLM
        let input = if WORKING_DIR_TOOLS.contains(&self.name.as_str()) {
            if let Some(dir) = self.default_working_dir.as_ref() {
                let mut obj = match input {
                    Value::Object(m) => m,
                    _ => serde_json::Map::new(),
                };
                // Inject workspace if working_dir is missing, null, or relative
                let should_inject = match obj.get("working_dir") {
                    None => true,
                    Some(v) if v.is_null() => true,
                    Some(Value::String(s)) => {
                        let s = s.trim();
                        s.is_empty() || s == "." || s == "./" || !s.starts_with('/')
                    }
                    _ => false,
                };
                if should_inject {
                    obj.insert("working_dir".to_string(), Value::String(dir.clone()));
                }
                Value::Object(obj)
            } else {
                input
            }
        } else {
            input
        };
        // opencode-parity AbortSignal: when the harness cancels this call,
        // we drop the registry-execute future. Drop semantics propagate down
        // to whatever the registry's per-tool impl is doing (kill_on_drop
        // for spawned subprocesses, reqwest abort, etc.).
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return ToolResult::Error {
                    error: format!("tool {} cancelled", self.name),
                    retryable: false,
                };
            }
            r = self.registry.execute_tool(&self.name, input) => r,
        };
        match outcome {
            Ok(output) => ToolResult::Success { output },
            Err(e) => {
                tracing::warn!(tool = %self.name, error = %e, "Tool execution failed");
                let retryable = matches!(
                    e,
                    crate::error::AlephError::NetworkError { .. }
                        | crate::error::AlephError::IoError(..)
                        | crate::error::AlephError::Timeout { .. }
                );
                ToolResult::Error {
                    error: e.to_string(),
                    retryable,
                }
            }
        }
    }
}

/// Build a `LoopToolRegistry` from an executor `ToolRegistry` + `UnifiedTool` list.
///
/// Each `UnifiedTool` becomes a `LoopTool` that delegates execution to the
/// shared `ToolRegistry`. Only active tools are included.
///
/// `default_working_dir` is injected into `bash/code_exec` tools when the LLM
/// doesn't specify a `working_dir` (defaults to agent workspace).
pub fn build_tool_adapters_from_tools<R: ToolRegistry + 'static>(
    tool_registry: Arc<R>,
    unified_tools: &[UnifiedTool],
    default_working_dir: Option<String>,
) -> Vec<Box<dyn LoopTool>> {
    let mut adapters: Vec<Box<dyn LoopTool>> = Vec::new();
    let default_working_dir = Arc::new(default_working_dir.clone());

    for tool in unified_tools {
        if !tool.is_active {
            continue;
        }

        let schema = tool
            .parameters_schema
            .clone()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}}));

        adapters.push(Box::new(RegistryToolAdapter {
            name: tool.name.clone(),
            description: tool.description.clone(),
            schema,
            registry: Arc::clone(&tool_registry),
            default_working_dir: Arc::clone(&default_working_dir),
        }));
    }

    adapters
}

pub fn build_registry_from_tools<R: ToolRegistry + 'static>(
    tool_registry: Arc<R>,
    unified_tools: &[UnifiedTool],
    default_working_dir: Option<String>,
) -> LoopToolRegistry {
    let mut registry = LoopToolRegistry::new();
    for adapter in build_tool_adapters_from_tools(tool_registry, unified_tools, default_working_dir)
    {
        registry.register(adapter);
    }
    registry
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ToolRegistry;
    use crate::tool_metadata::ToolSource;
    use serde_json::json;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;

    /// Mock ToolRegistry for testing.
    struct MockRegistry {
        results: HashMap<String, Value>,
    }

    impl ToolRegistry for MockRegistry {
        fn get_tool(&self, _name: &str) -> Option<&UnifiedTool> {
            None // Not needed for execution
        }

        fn execute_tool(
            &self,
            tool_name: &str,
            _arguments: Value,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<Value>> + Send + '_>> {
            let result = self.results.get(tool_name).cloned();
            let name = tool_name.to_string();
            Box::pin(async move {
                result.ok_or_else(|| crate::error::AlephError::tool_not_found(&name))
            })
        }
        // workspace_handle, smart_recall_config_handle, session_context_handle
        // all have default implementations returning None
    }

    fn make_unified_tool(name: &str, desc: &str) -> UnifiedTool {
        let mut tool = UnifiedTool::new(format!("native:{}", name), name, desc, ToolSource::Native);
        tool.parameters_schema = Some(json!({"type": "object", "properties": {}}));
        tool
    }

    #[tokio::test]
    async fn test_build_registry_from_tools() {
        let mut results = HashMap::new();
        results.insert("search".to_string(), json!({"found": 42}));

        let tool_registry = Arc::new(MockRegistry { results });
        let tools = vec![
            make_unified_tool("search", "Search for things"),
            make_unified_tool("memory", "Query memory"),
        ];

        let registry = build_registry_from_tools(tool_registry, &tools, None);
        assert_eq!(registry.len(), 2);
        assert!(registry.get("search").is_some());
        assert!(registry.get("memory").is_some());
    }

    #[tokio::test]
    async fn test_registry_adapter_execute() {
        let mut results = HashMap::new();
        results.insert("search".to_string(), json!({"found": 42}));

        let tool_registry = Arc::new(MockRegistry { results });
        let tools = vec![make_unified_tool("search", "Search")];

        let registry = build_registry_from_tools(tool_registry, &tools, None);
        let result = registry
            .execute("search", json!({"q": "rust"}), CancellationToken::new())
            .await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["found"], 42)
            }
            ToolResult::Error { error, .. } => panic!("expected success: {}", error),
        }
    }

    #[test]
    fn test_mutating_tools_excluded_from_readonly_allowlist() {
        // Known write/mutating tools must NOT be on the read-only allowlist, so
        // they default to whole-world exclusive. Includes `team_disband` and
        // `team_member_add`, which the old `EXCLUSIVE_TOOLS` denylist omitted —
        // the bug the allowlist inversion fixes (a forgotten mutator now
        // serializes instead of silently parallelizing).
        let write_tools = &[
            "bash",
            "code_exec",
            "file_ops",
            "apply_patch",
            "self_manage",
            "vault_store",
            "cron_manage",
            "agent_create",
            "agent_delete",
            "team_create",
            "team_delegate",
            "team_disband",
            "team_member_add",
            "team_member_remove",
            "heartbeat_create",
            "heartbeat_update",
            "skill_install",
            "channel_pairing",
            // Input-dependent read/write multiplexers resolve per-argument in
            // `concurrency_claim` and must never sit on the static read-only
            // list (their write arm would ride the `Shared` claim AND the
            // idempotency consolidation would exempt them from the Ask tier).
            // `doctor` with `fix=true` rebuilds the data dir / clears stale
            // locks — a filesystem mutation, never a freely-parallel read.
            "doctor",
            "note_schema",
            "session_send",
            "a2a_agents",
            "inbox_read",
            // Output gate masquerading as a read: `notify` messages the user.
            "heartbeat_report",
        ];
        for tool in write_tools {
            assert!(
                !READ_ONLY_TOOLS.contains(tool),
                "{tool} mutates state and must NOT be on READ_ONLY_TOOLS",
            );
        }
    }

    #[test]
    fn test_readonly_tools_on_allowlist() {
        // Read-only tools must be on the allowlist so they keep parallelizing
        // (and, since the idempotency consolidation, keep auto-retry + the
        // Ask-tier read exemption). All names here are REGISTERED tools —
        // the old list carried phantoms (`memory_recall`, `knowledge`).
        let read_tools = &[
            "search",
            "memory_search",
            "web_fetch",
            "file_read",
            "user_profile",
            "note_orient",
            "note_graph_query",
            "mcp_read_resource",
            "mcp_list_resources",
            "mcp_list_resource_templates",
            "mcp_get_prompt",
            "mcp_list_prompts",
            // The live progressive-disclosure meta-tool (formerly the ghost
            // names `list_tools` / `search_tools`).
            "tool_search",
        ];
        for tool in read_tools {
            assert!(
                READ_ONLY_TOOLS.contains(tool),
                "{tool} is read-only and should be on READ_ONLY_TOOLS",
            );
        }
    }

    #[tokio::test]
    async fn test_adapter_requires_confirmation() {
        let tool_registry = Arc::new(MockRegistry {
            results: HashMap::new(),
        });

        // A destructive builtin self-declares confirmation through the adapter.
        let confirm_tools = vec![make_unified_tool("agent_delete", "Delete an agent")];
        let registry = build_registry_from_tools(tool_registry.clone(), &confirm_tools, None);
        let agent_delete = registry.get("agent_delete").unwrap();
        assert!(agent_delete.requires_confirmation());

        // A plain read-only tool does not.
        let plain_tools = vec![make_unified_tool("search", "Search")];
        let registry = build_registry_from_tools(tool_registry, &plain_tools, None);
        let search = registry.get("search").unwrap();
        assert!(!search.requires_confirmation());
    }

    #[test]
    fn test_confirmation_required_tools_list() {
        // The three destructive builtins must self-declare confirmation,
        // replacing the deleted gateway `CONFIRMATION_REQUIRED_TOOLS` constant.
        for tool in &["vault_store", "agent_delete", "team_disband"] {
            assert!(
                CONFIRMATION_REQUIRED_TOOLS.contains(tool),
                "{} should require confirmation",
                tool
            );
        }
        // Read-only tools must NOT.
        for tool in &["search", "memory_recall", "web_fetch"] {
            assert!(
                !CONFIRMATION_REQUIRED_TOOLS.contains(tool),
                "{} should not require confirmation",
                tool
            );
        }
    }

    #[tokio::test]
    async fn test_adapter_concurrent_safe() {
        let tool_registry = Arc::new(MockRegistry {
            results: HashMap::new(),
        });

        // A read-only tool should be concurrent-safe
        let read_tools = vec![make_unified_tool("search", "Search")];
        let registry = build_registry_from_tools(tool_registry.clone(), &read_tools, None);
        let search = registry.get("search").unwrap();
        assert!(search.is_concurrent_safe(&json!({})));

        // An exclusive tool should NOT be concurrent-safe
        let write_tools = vec![make_unified_tool("bash", "Run commands")];
        let registry = build_registry_from_tools(tool_registry, &write_tools, None);
        let bash = registry.get("bash").unwrap();
        assert!(!bash.is_concurrent_safe(&json!({})));
    }

    #[tokio::test]
    async fn test_concurrency_claim_scopes() {
        use crate::tools::concurrency::{ConcurrencyClaim, ExclusiveScope};

        let tool_registry = Arc::new(MockRegistry {
            results: HashMap::new(),
        });
        let tools = vec![
            make_unified_tool("file_ops", "File operations"),
            make_unified_tool("file_write", "Write a file"),
            make_unified_tool("file_read", "Read a file"),
            make_unified_tool("apply_patch", "Apply a V4A patch"),
            make_unified_tool("bash", "Run commands"),
            make_unified_tool("search", "Search"),
        ];
        let registry = build_registry_from_tools(tool_registry, &tools, None);

        // file_ops read-only operation -> Shared (parallelizable).
        let claim = registry
            .get("file_ops")
            .unwrap()
            .concurrency_claim(&json!({"operation": "list", "path": "/x"}));
        assert_eq!(claim, ConcurrencyClaim::Shared);

        // file_ops mutating operation -> bounded Paths scope.
        let claim = registry
            .get("file_ops")
            .unwrap()
            .concurrency_claim(&json!({"operation": "move", "path": "/a", "destination": "/b"}));
        match claim {
            ConcurrencyClaim::Exclusive {
                scope: ExclusiveScope::Paths(p),
            } => {
                assert!(p.contains("/a") && p.contains("/b"));
            }
            other => panic!("expected bounded Paths, got {other:?}"),
        }

        // file_write -> bounded to its file_path (was racy Shared before).
        let claim = registry
            .get("file_write")
            .unwrap()
            .concurrency_claim(&json!({"file_path": "/src/a.rs", "content": "x"}));
        assert_eq!(
            claim,
            ConcurrencyClaim::paths(["/src/a.rs"]),
            "file_write must serialize on its target path"
        );

        // file_read stays Shared.
        assert_eq!(
            registry
                .get("file_read")
                .unwrap()
                .concurrency_claim(&json!({"path": "/src/a.rs"})),
            ConcurrencyClaim::Shared
        );

        // apply_patch -> bounded to the files its envelope touches, NOT the
        // whole-world `Global` it always degraded to before (its args carry no
        // `path` field, so the old `bounded_file_writer_path` lookup missed).
        let patch = "*** Begin Patch\n\
*** Update File: src/a.rs\n\
@@\n\
-old\n\
+new\n\
*** Delete File: src/b.rs\n\
*** End Patch\n";
        let claim = registry
            .get("apply_patch")
            .unwrap()
            .concurrency_claim(&json!({ "patch": patch }));
        assert_eq!(
            claim,
            ConcurrencyClaim::paths(["src/a.rs", "src/b.rs"]),
            "apply_patch must scope to its patched files so disjoint patches parallelize"
        );

        // A malformed / unparseable patch degrades to whole-world exclusive
        // (serializes) rather than racing on a wrong narrow footprint.
        assert_eq!(
            registry
                .get("apply_patch")
                .unwrap()
                .concurrency_claim(&json!({ "patch": "not a valid patch" })),
            ConcurrencyClaim::global()
        );

        // bash stays whole-world exclusive.
        assert_eq!(
            registry
                .get("bash")
                .unwrap()
                .concurrency_claim(&json!({"command": "ls"})),
            ConcurrencyClaim::global()
        );

        // plain read-only tool stays Shared.
        assert_eq!(
            registry
                .get("search")
                .unwrap()
                .concurrency_claim(&json!({"query": "x"})),
            ConcurrencyClaim::Shared
        );
    }

    #[tokio::test]
    async fn test_input_dependent_claims_doctor_note_schema_session_send() {
        use crate::tools::concurrency::ConcurrencyClaim;

        let tool_registry = Arc::new(MockRegistry {
            results: HashMap::new(),
        });
        let tools = vec![
            make_unified_tool("doctor", "Diagnose"),
            make_unified_tool("note_schema", "SCHEMA.md read/write"),
            make_unified_tool("session_send", "Delegate to a session"),
            make_unified_tool("a2a_agents", "Manage remote A2A agents"),
            make_unified_tool("inbox_read", "Read the team inbox"),
        ];
        let registry = build_registry_from_tools(tool_registry, &tools, None);

        // doctor: inspect (default) parallelizes; fix=true serializes.
        let doctor = registry.get("doctor").unwrap();
        assert_eq!(
            doctor.concurrency_claim(&json!({})),
            ConcurrencyClaim::Shared
        );
        assert_eq!(
            doctor.concurrency_claim(&json!({"fix": false})),
            ConcurrencyClaim::Shared
        );
        assert_eq!(
            doctor.concurrency_claim(&json!({"fix": true})),
            ConcurrencyClaim::global(),
            "a fixing doctor mutates (data dir, instance lock) and must not \
             ride a parallel read batch"
        );

        // note_schema: read parallelizes; write (and anything unrecognized)
        // serializes.
        let schema = registry.get("note_schema").unwrap();
        assert_eq!(
            schema.concurrency_claim(&json!({"action": "read"})),
            ConcurrencyClaim::Shared
        );
        assert_eq!(
            schema.concurrency_claim(&json!({"action": "write", "content": "x"})),
            ConcurrencyClaim::global()
        );
        assert_eq!(
            schema.concurrency_claim(&json!({})),
            ConcurrencyClaim::global()
        );

        // session_send: named target -> bounded Sessions scope keyed on the
        // RESOLVED gateway key (disjoint fan-out delegations parallelize);
        // missing/empty/unparseable key -> Global.
        let send = registry.get("session_send").unwrap();
        let expected_key =
            crate::builtin_tools::sessions::send_tool::claim_session_key("agent:a:main")
                .expect("valid key resolves");
        assert_eq!(
            send.concurrency_claim(&json!({"session_key": "agent:a:main", "message": "hi"})),
            ConcurrencyClaim::sessions([expected_key])
        );
        assert_eq!(
            send.concurrency_claim(&json!({"message": "hi"})),
            ConcurrencyClaim::global(),
            "an unnamed target resolves server-side; the adapter cannot pin it"
        );
        assert_eq!(
            send.concurrency_claim(&json!({"session_key": "  ", "message": "hi"})),
            ConcurrencyClaim::global()
        );
        assert_eq!(
            send.concurrency_claim(&json!({"session_key": "not a session key", "message": "hi"})),
            ConcurrencyClaim::global(),
            "an unparseable key cannot be pinned to one session"
        );
        // Aliased spellings that the gateway collapses onto ONE execution
        // session must yield CONFLICTING claims (same claim key), or two
        // fan-out arms would race into one session. `dm` and `group` keys
        // with the same agent + peer both collapse to `peer`.
        let dm = send
            .concurrency_claim(&json!({"session_key": "agent:a:telegram:dm:u1", "message": "x"}));
        let group = send.concurrency_claim(
            &json!({"session_key": "agent:a:telegram:group:u1", "message": "y"}),
        );
        assert_ne!(
            dm,
            ConcurrencyClaim::global(),
            "test key must parse to a bounded Sessions scope for the alias \
             assertion to be meaningful"
        );
        assert!(
            crate::tools::concurrency::claims_conflict(&dm, &group),
            "gateway-collapsed aliases must conflict: dm={dm:?} group={group:?}"
        );

        // a2a_agents: list is a pure read; add/remove mutate the remote-agent
        // trust surface.
        let a2a = registry.get("a2a_agents").unwrap();
        assert_eq!(
            a2a.concurrency_claim(&json!({"action": "list"})),
            ConcurrencyClaim::Shared
        );
        assert_eq!(
            a2a.concurrency_claim(&json!({"action": "add", "url": "https://x"})),
            ConcurrencyClaim::global()
        );
        assert_eq!(
            a2a.concurrency_claim(&json!({})),
            ConcurrencyClaim::global()
        );

        // inbox_read: pure modes parallelize; the default consumes unread
        // state and serializes.
        let inbox = registry.get("inbox_read").unwrap();
        assert_eq!(
            inbox.concurrency_claim(&json!({"peek": true})),
            ConcurrencyClaim::Shared
        );
        assert_eq!(
            inbox.concurrency_claim(&json!({"count_only": true})),
            ConcurrencyClaim::Shared
        );
        assert_eq!(
            inbox.concurrency_claim(&json!({"mark_read": false})),
            ConcurrencyClaim::Shared
        );
        assert_eq!(
            inbox.concurrency_claim(&json!({})),
            ConcurrencyClaim::global(),
            "default mark_read=true consumes unread state"
        );
    }

    #[tokio::test]
    async fn test_inactive_tools_excluded() {
        let tool_registry = Arc::new(MockRegistry {
            results: HashMap::new(),
        });

        let mut inactive = make_unified_tool("disabled", "Should not appear");
        inactive.is_active = false;

        let tools = vec![make_unified_tool("active", "Active tool"), inactive];

        let registry = build_registry_from_tools(tool_registry, &tools, None);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("active").is_some());
        assert!(registry.get("disabled").is_none());
    }
}
