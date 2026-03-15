//! Shared tool name repair logic.
//!
//! Extracted from `AlephToolServer` and `AlephToolServerHandle` to avoid
//! duplicating the repair strategies (case-insensitive, snake_case, invalid fallback).

use serde_json::Value;

use super::ToolMap;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::types::{ToolRepairInfo, ToolRepairType};

/// Convert a string to snake_case.
///
/// Examples:
/// - "WebSearch" -> "web_search"
/// - "searchAPI" -> "search_api"
/// - "already_snake" -> "already_snake"
pub(super) fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    let mut prev_was_lower = false;

    for c in s.chars() {
        if c.is_uppercase() {
            if prev_was_lower {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap_or(c));
            prev_was_lower = false;
        } else {
            result.push(c);
            prev_was_lower = c.is_lowercase();
        }
    }

    result
}

/// Shared implementation for `call_with_repair`.
///
/// Attempts to call a tool with automatic name repair:
/// 1. Exact match
/// 2. Case-insensitive matching
/// 3. snake_case conversion
/// 4. "invalid" tool fallback
/// 5. Error with suggestion
pub(super) async fn call_with_repair_impl(
    tools_map: &ToolMap,
    name: &str,
    args: Value,
) -> (Result<Value>, Option<ToolRepairInfo>) {
    let tools = tools_map.read().await;

    // 1. Try exact match first
    if let Some(tool) = tools.get(name) {
        let tool = Arc::clone(tool);
        drop(tools);
        return (tool.call(args).await, None);
    }

    // 2. Try case-insensitive repair
    let lower_name = name.to_lowercase();
    if lower_name != name {
        if let Some(tool) = tools.get(&lower_name) {
            let tool = Arc::clone(tool);
            drop(tools);
            tracing::info!(
                original = name,
                repaired = lower_name,
                "Repaired tool name (case-insensitive)"
            );
            return (
                tool.call(args).await,
                Some(ToolRepairInfo {
                    original_name: name.to_string(),
                    repaired_name: lower_name,
                    repair_type: ToolRepairType::CaseInsensitive,
                }),
            );
        }
    }

    // 3. Try snake_case conversion (e.g., "WebSearch" -> "web_search")
    let snake_name = to_snake_case(name);
    if snake_name != name && snake_name != lower_name {
        if let Some(tool) = tools.get(&snake_name) {
            let tool = Arc::clone(tool);
            drop(tools);
            tracing::info!(
                original = name,
                repaired = snake_name,
                "Repaired tool name (snake_case)"
            );
            return (
                tool.call(args).await,
                Some(ToolRepairInfo {
                    original_name: name.to_string(),
                    repaired_name: snake_name,
                    repair_type: ToolRepairType::SnakeCase,
                }),
            );
        }
    }

    // 4. Route to "invalid" tool if available
    if let Some(invalid_tool) = tools.get("invalid") {
        let invalid_tool = Arc::clone(invalid_tool);
        drop(tools);

        tracing::info!(
            tool = name,
            "Routing unknown tool to invalid handler"
        );

        let invalid_args = serde_json::json!({
            "tool": name,
            "error": format!("Tool '{}' not found in registry", name)
        });

        return (
            invalid_tool.call(invalid_args).await,
            Some(ToolRepairInfo {
                original_name: name.to_string(),
                repaired_name: "invalid".to_string(),
                repair_type: ToolRepairType::InvalidFallback,
            }),
        );
    }

    // 5. No repair possible
    drop(tools);
    (
        Err(AlephError::tool_not_found_with_suggestion(
            name,
            "Use list_tools to see available tools",
        )),
        None,
    )
}

/// Shared implementation for `try_repair_tool_name`.
///
/// Returns the repaired name if a match is found, None otherwise.
pub(super) async fn try_repair_tool_name_impl(tools_map: &ToolMap, name: &str) -> Option<String> {
    let tools = tools_map.read().await;

    // Exact match
    if tools.contains_key(name) {
        return Some(name.to_string());
    }

    // Case-insensitive
    let lower_name = name.to_lowercase();
    if tools.contains_key(&lower_name) {
        return Some(lower_name);
    }

    // Snake case
    let snake_name = to_snake_case(name);
    if tools.contains_key(&snake_name) {
        return Some(snake_name);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_snake_case() {
        assert_eq!(to_snake_case("WebSearch"), "web_search");
        assert_eq!(to_snake_case("searchAPI"), "search_api");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
        assert_eq!(to_snake_case("HTTPRequest"), "httprequest");
        assert_eq!(to_snake_case("Search"), "search");
        assert_eq!(to_snake_case("search"), "search");
    }
}
