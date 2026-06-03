//! Adapter bridging executor::ToolRegistry to LoopTool.
//!
//! Wraps `ToolRegistry::execute_tool()` + `UnifiedTool` metadata into
//! LoopTool instances for use in the agent loop.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::executor::ToolRegistry;
use crate::tool_metadata::UnifiedTool;

use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};

/// A LoopTool backed by a shared ToolRegistry.
///
/// Each instance holds the metadata from a `UnifiedTool` (name, description,
/// schema) and delegates execution to `ToolRegistry::execute_tool()`.
struct RegistryToolAdapter<R: ToolRegistry + 'static> {
    name: String,
    description: String,
    schema: Value,
    registry: Arc<R>,
    /// Default working directory for bash/code_exec tools (agent workspace)
    default_working_dir: Option<String>,
}

/// Tools that should have working_dir injected when not specified by LLM
const WORKING_DIR_TOOLS: &[&str] = &["bash", "code_exec"];

/// Tools that mutate state and must NOT run concurrently.
pub(crate) const EXCLUSIVE_TOOLS: &[&str] = &[
    "bash",
    "code_exec",
    "file_ops",
    "self_manage",
    "vault_store",
    "cron_manage",
    "agent_create",
    "agent_delete",
    "team_create",
    "team_delegate",
    "team_set_protocol",
    "heartbeat_create",
    "heartbeat_delete",
    "task_create",
    "task_update",
    "channel_pairing",
];

/// Builtin tools that require explicit user confirmation before they run.
///
/// Destructive or security-sensitive operations (secret-vault mutation,
/// agent deletion, team disband). The adapter reports this through
/// [`LoopTool::requires_confirmation`], so the live `ScopedToolService`
/// confirmation gate routes a user prompt before dispatch — no gateway-side
/// allowlist needed. Co-located with [`EXCLUSIVE_TOOLS`]: both are per-tool
/// runtime dispatch properties this adapter self-declares. Mirrors the
/// `AlephTool::requires_confirmation()` overrides on these tools (which feed
/// the metadata/describe path); keep the two in sync.
pub(crate) const CONFIRMATION_REQUIRED_TOOLS: &[&str] =
    &["vault_store", "agent_delete", "team_disband"];

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
        !EXCLUSIVE_TOOLS.contains(&self.name.as_str())
    }

    fn requires_confirmation(&self) -> bool {
        CONFIRMATION_REQUIRED_TOOLS.contains(&self.name.as_str())
    }

    async fn execute(&self, input: Value, cancel: CancellationToken) -> ToolResult {
        tracing::debug!(tool = %self.name, args = %input, "Tool call raw arguments from LLM");
        // Inject default working_dir for bash/code_exec if not provided by LLM
        let input = if WORKING_DIR_TOOLS.contains(&self.name.as_str()) {
            if let Some(ref dir) = self.default_working_dir {
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
/// `default_working_dir` is injected into bash/code_exec tools when the LLM
/// doesn't specify a working_dir (defaults to agent workspace).
pub fn build_tool_adapters_from_tools<R: ToolRegistry + 'static>(
    tool_registry: Arc<R>,
    unified_tools: &[UnifiedTool],
    default_working_dir: Option<String>,
) -> Vec<Box<dyn LoopTool>> {
    let mut adapters: Vec<Box<dyn LoopTool>> = Vec::new();

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
            default_working_dir: default_working_dir.clone(),
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
        // workspace_handle, smart_recall_config_handle, session_context_handle,
        // tool_policy_handle all have default implementations returning None
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
    fn test_exclusive_tools_list() {
        // Known write/mutating tools must be in the exclusive list
        let write_tools = &[
            "bash",
            "code_exec",
            "file_ops",
            "self_manage",
            "vault_store",
            "cron_manage",
            "agent_create",
            "agent_delete",
            "team_create",
            "team_delegate",
            "channel_pairing",
        ];
        for tool in write_tools {
            assert!(
                EXCLUSIVE_TOOLS.contains(tool),
                "{} should be in EXCLUSIVE_TOOLS",
                tool
            );
        }
    }

    #[test]
    fn test_readonly_not_in_exclusive() {
        // Read-only tools must NOT be in the exclusive list
        let read_tools = &["search", "memory_recall", "web_fetch", "knowledge"];
        for tool in read_tools {
            assert!(
                !EXCLUSIVE_TOOLS.contains(tool),
                "{} should NOT be in EXCLUSIVE_TOOLS",
                tool
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
