//! Tools Invoke Handler
//!
//! Provides `tools.invoke` JSON-RPC method for direct execution of a builtin
//! tool by name, bypassing the LLM agent loop. Intended for E2E tests and
//! deterministic tool exercising — production callers should still go through
//! the agent loop (R8 LLM Sovereignty).
//!
//! ## Request
//! ```json
//! {"tool_name": "memory_search", "arguments": {"query": "foo"}, "agent_id": "main"}
//! ```
//!
//! ## Response (success)
//! ```json
//! {"ok": true, "tool_name": "memory_search", "result": {...}}
//! ```
//!
//! ## Response (tool error)
//! Returns RPC error with `INTERNAL_ERROR` code and the tool's error message.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use crate::executor::ToolRegistry;

/// Parameters for `tools.invoke`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InvokeParams {
    /// Tool name as registered in the BuiltinToolRegistry (e.g. "memory_search", "note_manage").
    pub tool_name: String,
    /// Arguments forwarded to the tool. Schema depends on the tool.
    #[serde(default)]
    pub arguments: Value,
    /// Optional agent_id; merged into `arguments.agent_id` when present and the
    /// arguments object doesn't already carry one. Tools that read `agent_id`
    /// (e.g. `note_manage`) pick it up automatically.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Real handler — executes the tool directly via the registry trait.
pub async fn handle_invoke<R>(request: JsonRpcRequest, registry: Arc<R>) -> JsonRpcResponse
where
    R: ToolRegistry + ?Sized,
{
    let params: InvokeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if params.tool_name.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "tool_name must not be empty");
    }

    let arguments = merge_agent_id(params.arguments, params.agent_id.as_deref());

    match registry.execute_tool(&params.tool_name, arguments).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "tool_name": params.tool_name,
                "result": result,
            }),
        ),
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("tool '{}' failed: {}", params.tool_name, err),
        ),
    }
}

/// Stub registered at startup; replaced with the real handler once the
/// `BuiltinToolRegistry` Arc is available (see `agent_init.rs`).
pub async fn handle_invoke_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "tools.invoke requires ToolRegistry — wire in Gateway startup".to_string(),
    )
}

/// If the caller supplied a top-level `agent_id`, fold it into the JSON
/// arguments object under the `agent_id` key. Existing values win so the
/// caller can still override per-call. Non-object arguments pass through
/// unchanged (the tool will reject them on its own schema check).
fn merge_agent_id(mut arguments: Value, agent_id: Option<&str>) -> Value {
    let Some(agent_id) = agent_id else {
        return arguments;
    };
    if let Value::Object(ref mut map) = arguments {
        map.entry("agent_id".to_string())
            .or_insert(Value::String(agent_id.to_string()));
    }
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::UnifiedTool;
    use crate::error::Result as AlephResult;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Minimal in-test ToolRegistry returning canned values keyed by tool name.
    /// Returns an error for unknown tools so we can exercise the error path.
    struct StubRegistry {
        results: Mutex<HashMap<String, AlephResult<Value>>>,
        last_args: Mutex<Option<(String, Value)>>,
    }

    impl StubRegistry {
        fn new() -> Self {
            Self {
                results: Mutex::new(HashMap::new()),
                last_args: Mutex::new(None),
            }
        }
        fn with_ok(self, name: &str, value: Value) -> Self {
            self.results
                .lock()
                .unwrap()
                .insert(name.to_string(), Ok(value));
            self
        }
        fn last_call(&self) -> Option<(String, Value)> {
            self.last_args.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl ToolRegistry for StubRegistry {
        fn get_tool(&self, _name: &str) -> Option<&UnifiedTool> {
            None
        }
        fn execute_tool(
            &self,
            tool_name: &str,
            arguments: Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AlephResult<Value>> + Send + '_>>
        {
            *self.last_args.lock().unwrap_or_else(|e| e.into_inner()) = Some((tool_name.to_string(), arguments.clone()));
            let canned = self
                .results
                .lock()
                .unwrap()
                .get(tool_name)
                .map(|r| match r {
                    Ok(v) => Ok(v.clone()),
                    Err(e) => Err(crate::error::AlephError::tool(e.to_string())),
                })
                .unwrap_or_else(|| {
                    Err(crate::error::AlephError::tool(format!(
                        "unknown tool: {tool_name}"
                    )))
                });
            Box::pin(async move { canned })
        }
    }

    #[tokio::test]
    async fn rejects_missing_params() {
        let reg = Arc::new(StubRegistry::new());
        let req = JsonRpcRequest::with_id("tools.invoke", None, json!(1));
        let resp = handle_invoke(req, reg).await;
        assert!(!resp.is_success(), "expected error response");
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn rejects_empty_tool_name() {
        let reg = Arc::new(StubRegistry::new());
        let params = json!({"tool_name": "  ", "arguments": {}});
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg).await;
        assert!(!resp.is_success());
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn returns_internal_error_when_tool_fails() {
        let reg = Arc::new(StubRegistry::new());
        let params = json!({"tool_name": "missing_tool", "arguments": {}});
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg).await;
        assert!(!resp.is_success());
        let err = resp.error.unwrap();
        assert_eq!(err.code, INTERNAL_ERROR);
        assert!(
            err.message.contains("missing_tool"),
            "error should mention tool name: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn forwards_arguments_and_returns_tool_result() {
        let reg = Arc::new(StubRegistry::new().with_ok(
            "memory_search",
            json!({"hits": [{"id": "n1", "snippet": "hello"}]}),
        ));
        let params = json!({
            "tool_name": "memory_search",
            "arguments": {"query": "hello"},
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg.clone()).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["tool_name"], "memory_search");
        assert_eq!(result["result"]["hits"][0]["id"], "n1");
        let (called_name, called_args) = reg.last_call().expect("execute_tool was called");
        assert_eq!(called_name, "memory_search");
        assert_eq!(called_args["query"], "hello");
    }

    #[tokio::test]
    async fn folds_top_level_agent_id_into_arguments() {
        let reg = Arc::new(StubRegistry::new().with_ok("note_manage", json!({"status": "ok"})));
        let params = json!({
            "tool_name": "note_manage",
            "arguments": {"action": "list"},
            "agent_id": "research",
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg.clone()).await;
        assert!(resp.is_success());
        let (_, called_args) = reg.last_call().unwrap();
        assert_eq!(called_args["agent_id"], "research");
        assert_eq!(called_args["action"], "list");
    }

    #[tokio::test]
    async fn merges_agent_id_into_arguments_when_absent() {
        let merged = merge_agent_id(json!({"query": "x"}), Some("research"));
        assert_eq!(merged["agent_id"], "research");
    }

    #[tokio::test]
    async fn does_not_overwrite_existing_agent_id() {
        let merged = merge_agent_id(json!({"agent_id": "explicit"}), Some("ignored"));
        assert_eq!(merged["agent_id"], "explicit");
    }

    #[tokio::test]
    async fn passes_through_non_object_arguments() {
        let merged = merge_agent_id(json!("plain string"), Some("any"));
        assert_eq!(merged, json!("plain string"));
    }
}
