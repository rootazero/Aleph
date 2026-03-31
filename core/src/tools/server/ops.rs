//! Shared tool registry operations.
//!
//! Free functions operating on `ToolMap` to eliminate duplication
//! between `AlephToolServer` and `AlephToolServerHandle`.

use serde_json::Value;

use super::ToolMap;
use crate::dispatcher::ToolDefinition;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::traits::AlephToolDyn;
use crate::tools::types::ToolUpdateInfo;

pub(super) async fn add_tool_impl(tools: &ToolMap, tool: impl AlephToolDyn + 'static) {
    let name = tool.name().to_string();
    tools.write().await.insert(name, Arc::new(tool));
}

pub(super) async fn add_tool_arc_impl(tools: &ToolMap, tool: Arc<dyn AlephToolDyn>) {
    let name = tool.name().to_string();
    tools.write().await.insert(name, tool);
}

pub(super) async fn replace_tool_arc_impl(
    tools: &ToolMap,
    tool: Arc<dyn AlephToolDyn>,
) -> ToolUpdateInfo {
    let name = tool.name().to_string();
    let new_description = tool.definition().description;

    let mut guard = tools.write().await;
    let old_tool = guard.insert(name.clone(), tool);

    ToolUpdateInfo {
        tool_name: name,
        was_replaced: old_tool.is_some(),
        old_description: old_tool.map(|t| t.definition().description),
        new_description,
    }
}

pub(super) async fn remove_tool_impl(tools: &ToolMap, name: &str) -> bool {
    tools.write().await.remove(name).is_some()
}

pub(super) async fn has_tool_impl(tools: &ToolMap, name: &str) -> bool {
    tools.read().await.contains_key(name)
}

pub(super) async fn get_definition_impl(tools: &ToolMap, name: &str) -> Option<ToolDefinition> {
    tools.read().await.get(name).map(|t| t.definition())
}

pub(super) async fn list_definitions_impl(tools: &ToolMap) -> Vec<ToolDefinition> {
    let mut defs: Vec<_> = tools
        .read()
        .await
        .values()
        .map(|t| t.definition())
        .collect();
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

pub(super) async fn list_names_impl(tools: &ToolMap) -> Vec<String> {
    let mut names: Vec<_> = tools.read().await.keys().cloned().collect();
    names.sort();
    names
}

pub(super) async fn list_tools_arc_impl(tools: &ToolMap) -> Vec<Arc<dyn AlephToolDyn>> {
    tools.read().await.values().cloned().collect()
}

pub(super) async fn len_impl(tools: &ToolMap) -> usize {
    tools.read().await.len()
}

pub(super) async fn is_empty_impl(tools: &ToolMap) -> bool {
    tools.read().await.is_empty()
}

pub(super) async fn call_impl(tools: &ToolMap, name: &str, args: Value) -> Result<Value> {
    let guard = tools.read().await;
    let tool = guard
        .get(name)
        .ok_or_else(|| AlephError::tool_not_found(name))?;

    // Clone the Arc to release the read lock before calling
    let tool = Arc::clone(tool);
    drop(guard);

    tool.call(args).await
}

pub(super) async fn clear_impl(tools: &ToolMap) {
    tools.write().await.clear();
}
