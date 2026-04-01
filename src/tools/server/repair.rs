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
/// Handles consecutive uppercase runs correctly:
/// - "WebSearch" -> "web_search"
/// - "searchAPI" -> "search_api"
/// - "HTTPRequest" -> "http_request"
/// - "already_snake" -> "already_snake"
pub(super) fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    let chars: Vec<char> = s.chars().collect();

    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_was_upper = i > 0 && chars[i - 1].is_uppercase();
            let prev_was_lower_or_digit =
                i > 0 && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit());
            let next_is_lower = i + 1 < chars.len() && chars[i + 1].is_lowercase();

            // Insert underscore before:
            // 1. An uppercase letter preceded by a lowercase letter or digit: "searchA" → "search_a", "get3D" → "get3_d"
            // 2. The last letter of an uppercase run followed by lowercase: "HTTPRequest" → "http_request"
            //    (digits don't trigger rule 2 — "MP3" stays as "mp3", not "m_p3")
            if prev_was_lower_or_digit || (prev_was_upper && next_is_lower) {
                result.push('_');
            }
            result.extend(c.to_lowercase());
        } else {
            result.push(c);
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
    let lower_name = name.to_lowercase();
    let snake_name = to_snake_case(name);

    // Single read lock for all lookup attempts — consistent snapshot, no TOCTOU gap
    let (tool, repair_info) = {
        let tools = tools_map.read().await;

        if let Some(t) = tools.get(name).map(Arc::clone) {
            // 1. Exact match
            (Some(t), None)
        } else if lower_name != name {
            if let Some(t) = tools.get(&lower_name).map(Arc::clone) {
                // 2. Case-insensitive repair
                (
                    Some(t),
                    Some(ToolRepairInfo {
                        original_name: name.to_string(),
                        repaired_name: lower_name.clone(),
                        repair_type: ToolRepairType::CaseInsensitive,
                    }),
                )
            } else if snake_name != name && snake_name != lower_name {
                // 3. snake_case conversion
                tools
                    .get(&snake_name)
                    .map(Arc::clone)
                    .map_or((None, None), |t| {
                        (
                            Some(t),
                            Some(ToolRepairInfo {
                                original_name: name.to_string(),
                                repaired_name: snake_name.clone(),
                                repair_type: ToolRepairType::SnakeCase,
                            }),
                        )
                    })
            } else {
                (None, None)
            }
        } else if snake_name != name {
            // 3. snake_case conversion (name was already lowercase)
            tools
                .get(&snake_name)
                .map(Arc::clone)
                .map_or((None, None), |t| {
                    (
                        Some(t),
                        Some(ToolRepairInfo {
                            original_name: name.to_string(),
                            repaired_name: snake_name.clone(),
                            repair_type: ToolRepairType::SnakeCase,
                        }),
                    )
                })
        } else {
            (None, None)
        }
    };

    if let Some(tool) = tool {
        if let Some(ref info) = repair_info {
            tracing::info!(
                original = name,
                repaired = %info.repaired_name,
                repair_type = ?info.repair_type,
                "Repaired tool name"
            );
        }
        return (tool.call(args).await, repair_info);
    }

    // 4. Route to "invalid" tool if available
    let invalid = {
        let tools = tools_map.read().await;
        tools.get("invalid").map(Arc::clone)
    };
    if let Some(invalid_tool) = invalid {
        tracing::info!(tool = name, "Routing unknown tool to invalid handler");

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
        assert_eq!(to_snake_case("HTTPRequest"), "http_request");
        assert_eq!(to_snake_case("Search"), "search");
        assert_eq!(to_snake_case("search"), "search");
        assert_eq!(to_snake_case("getHTTPSUrl"), "get_https_url");
        assert_eq!(to_snake_case("XMLParser"), "xml_parser");
        assert_eq!(to_snake_case("parseJSON"), "parse_json");
        // Digit boundary handling
        assert_eq!(to_snake_case("getMP3File"), "get_mp3_file");
        assert_eq!(to_snake_case("list3DModels"), "list3_d_models");
    }
}
