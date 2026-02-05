//! Tools context for BDD tests

use alephcore::tools::{AlephToolServer, AlephToolServerHandle, ToolUpdateInfo};
use alephcore::dispatcher::ToolDefinition;

/// Context for tool server tests
pub struct ToolsContext {
    /// Tool server instance
    pub server: Option<AlephToolServer>,
    /// Tool server handle for handle-based operations
    pub handle: Option<AlephToolServerHandle>,
    /// Tool definition captured from a tool
    pub tool_definition: Option<ToolDefinition>,
    /// LLM context string from a tool definition
    pub llm_context: Option<String>,
    /// Update info from replace_tool operations
    pub update_info: Option<ToolUpdateInfo>,
    /// Result from calling a tool
    pub call_result: Option<serde_json::Value>,
    /// Replacement counter for tracking multiple replacements
    pub replacement_count: usize,
}

impl Default for ToolsContext {
    fn default() -> Self {
        Self {
            server: None,
            handle: None,
            tool_definition: None,
            llm_context: None,
            update_info: None,
            call_result: None,
            replacement_count: 0,
        }
    }
}

impl std::fmt::Debug for ToolsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolsContext")
            .field("server", &self.server.as_ref().map(|_| "AlephToolServer"))
            .field("handle", &self.handle.as_ref().map(|_| "AlephToolServerHandle"))
            .field("tool_definition", &self.tool_definition)
            .field("llm_context", &self.llm_context)
            .field("update_info", &self.update_info)
            .field("call_result", &self.call_result)
            .field("replacement_count", &self.replacement_count)
            .finish()
    }
}
