//! Runtime tool hot-refresh support.
//!
//! Allows external systems (e.g. MCP server reconnect, skill install) to signal
//! that the tool set has changed. The main loop polls cheaply each iteration and
//! rebuilds the registry only when necessary.

use super::tool::{LoopTool, LoopToolRegistry};

// =============================================================================
// ToolRefreshSource trait
// =============================================================================

/// Source of runtime tool changes.
///
/// Implementations must be cheap to poll (flag check only) and only do real
/// work in `fetch_tools` when `poll_changes` returns `true`.
pub trait ToolRefreshSource: Send + Sync {
    /// Check if tools changed. Must be cheap (flag check only).
    fn poll_changes(&self) -> bool;

    /// Fetch refreshed tool list. Only called when `poll_changes()` is true.
    fn fetch_tools(&self) -> Vec<Box<dyn LoopTool>>;
}

// =============================================================================
// Helper
// =============================================================================

/// Build a fresh `LoopToolRegistry` from a list of tools.
pub fn build_refreshed_registry(tools: Vec<Box<dyn LoopTool>>) -> LoopToolRegistry {
    let mut registry = LoopToolRegistry::new();
    for tool in tools {
        registry.register(tool);
    }
    registry
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::tool::ToolResult;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // -- Mock tool --

    struct FakeTool {
        tool_name: String,
    }

    #[async_trait]
    impl LoopTool for FakeTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            "fake tool"
        }

        fn schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn execute(&self, _args: Value) -> ToolResult {
            ToolResult::Success {
                output: Value::Null,
            }
        }
    }

    // -- Mock refresh source --

    struct MockRefreshSource {
        flag: AtomicBool,
        tools: Vec<String>,
    }

    impl MockRefreshSource {
        fn new(tools: Vec<&str>) -> Self {
            Self {
                flag: AtomicBool::new(false),
                tools: tools.into_iter().map(String::from).collect(),
            }
        }

        fn signal(&self) {
            self.flag.store(true, Ordering::Release);
        }
    }

    impl ToolRefreshSource for MockRefreshSource {
        fn poll_changes(&self) -> bool {
            self.flag.swap(false, Ordering::AcqRel)
        }

        fn fetch_tools(&self) -> Vec<Box<dyn LoopTool>> {
            self.tools
                .iter()
                .map(|name| -> Box<dyn LoopTool> {
                    Box::new(FakeTool {
                        tool_name: name.clone(),
                    })
                })
                .collect()
        }
    }

    #[test]
    fn poll_returns_false_when_no_changes() {
        let src = MockRefreshSource::new(vec!["a"]);
        assert!(!src.poll_changes());
        assert!(!src.poll_changes());
    }

    #[test]
    fn poll_returns_true_after_signal() {
        let src = MockRefreshSource::new(vec!["a"]);
        src.signal();
        assert!(src.poll_changes(), "first poll after signal should be true");
        assert!(
            !src.poll_changes(),
            "second poll should reset back to false"
        );
    }

    #[test]
    fn fetch_tools_returns_configured_tools() {
        let src = MockRefreshSource::new(vec!["alpha", "beta", "gamma"]);
        let tools = src.fetch_tools();
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn apply_refresh_builds_new_registry() {
        let src = Arc::new(MockRefreshSource::new(vec!["x", "y"]));
        src.signal();
        assert!(src.poll_changes());
        let tools = src.fetch_tools();
        let registry = build_refreshed_registry(tools);
        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 2);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"x"));
        assert!(names.contains(&"y"));
    }
}
