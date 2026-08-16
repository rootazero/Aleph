//! Shared tool registry operations.
//!
//! Free functions operating on `ToolMap`. Only the live production paths
//! are kept: `replace_tool_arc_impl` (markdown-skill hot reload) and
//! `list_tools_arc_impl` (agent-loop factory).

use super::ToolMap;
use crate::sync_primitives::Arc;
use crate::tools::traits::AlephToolDyn;
use crate::tools::types::ToolUpdateInfo;

pub(super) async fn replace_tool_arc_impl(
    tools: &ToolMap,
    tool: Arc<dyn AlephToolDyn>,
) -> ToolUpdateInfo {
    let name = tool.name().to_string();
    let new_description = tool.definition().description;

    let mut guard = tools.lock().unwrap_or_else(|e| e.into_inner());
    let old_tool = guard.insert(name.clone(), tool);

    ToolUpdateInfo {
        tool_name: name,
        was_replaced: old_tool.is_some(),
        old_description: old_tool.map(|t| t.definition().description),
        new_description,
    }
}

pub(super) async fn list_tools_arc_impl(tools: &ToolMap) -> Vec<Arc<dyn AlephToolDyn>> {
    let guard = tools.lock().unwrap_or_else(|e| e.into_inner());
    guard.values().cloned().collect()
}
