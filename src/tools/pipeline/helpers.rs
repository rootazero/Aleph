use serde_json::Value;

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::session::ingress_safety::SafetyError;
use crate::tools::pipeline::{MAX_TOOL_RESULT_TOKENS, TRUNCATION_SUFFIX};

/// Map a `SafetyError` to a human-readable error string.
///
/// `NeedsConfirmation` is downgraded to a denial with explanation when no
/// interactive confirmation handler is available (the common case for automated
/// agent runs). This prevents the LLM from retrying indefinitely.
pub(super) fn map_safety_error(e: &SafetyError) -> String {
    match e {
        SafetyError::Blocked { tool, pattern } => {
            format!(
                "[BLOCKED] Tool '{}' blocked by safety pattern '{}'",
                tool, pattern
            )
        }
        SafetyError::NeedsConfirmation { tool } => {
            format!(
                "[DENIED] Tool '{}' is classified as high-risk and requires user confirmation. \
                 No confirmation handler is available in this session. \
                 Use a safer alternative or ask the user to grant permission for this tool.",
                tool
            )
        }
        SafetyError::PolicyDenied { tool } => {
            format!("[DENIED] Tool '{}' denied by policy", tool)
        }
    }
}

/// Fast-fail input validation against tool schema.
pub(super) fn validate_input_fast(schema: &Value, input: &Value) -> Result<(), String> {
    if !input.is_object() {
        return Err("expected JSON object".into());
    }
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        if let Some(obj) = input.as_object() {
            for field in required {
                if let Some(name) = field.as_str() {
                    if !obj.contains_key(name) {
                        return Err(format!("missing required field: {name}"));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Convert a JSON Value to a display string.
pub(super) fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Check if a tool name corresponds to a file read operation.
pub(super) fn is_file_read_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("read_file") || lower.contains("file_read") || lower == "read"
}

/// Default per-tool result budgets.
pub(crate) fn default_result_budget(tool_name: &str) -> usize {
    match tool_name {
        "Read" => 12_000,
        "WebFetch" | "web_fetch" => 10_000,
        "Bash" | "bash_exec" => 8_000,
        "Grep" => 6_000,
        _ => MAX_TOOL_RESULT_TOKENS,
    }
}

/// Truncate a tool result with head+tail preservation.
pub(crate) fn truncate_tool_result_with_budget(text: &str, budget_tokens: usize) -> String {
    let estimated = estimate_tokens_smart(text);
    if estimated <= budget_tokens {
        return text.to_string();
    }

    let chars_per_token: f64 = 2.5;
    let total_chars = (budget_tokens as f64 * chars_per_token) as usize;
    let head_chars = (total_chars as f64 * 0.7) as usize;
    let tail_chars = total_chars.saturating_sub(head_chars);

    let head_end = text
        .char_indices()
        .take(head_chars)
        .filter(|(_, c)| *c == '\n')
        .last()
        .map(|(i, _)| i + 1)
        .unwrap_or_else(|| {
            text.char_indices()
                .nth(head_chars)
                .map(|(i, _)| i)
                .unwrap_or(text.len())
        });

    let tail_byte_approx = text.len().saturating_sub(tail_chars * 4);
    let tail_byte_approx = (tail_byte_approx..text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let tail_start = text[tail_byte_approx..]
        .find('\n')
        .map(|i| tail_byte_approx + i + 1)
        .unwrap_or(tail_byte_approx);

    if head_end >= tail_start {
        return truncate_tool_result(text);
    }

    let truncated_tokens = estimated.saturating_sub(budget_tokens);
    format!(
        "{}\n\n[... truncated ~{} tokens ...]\n\n{}",
        &text[..head_end],
        truncated_tokens,
        &text[tail_start..],
    )
}

/// Truncate a tool result string if it exceeds `MAX_TOOL_RESULT_TOKENS`.
pub(crate) fn truncate_tool_result(text: &str) -> String {
    let estimated = estimate_tokens_smart(text);
    if estimated <= MAX_TOOL_RESULT_TOKENS {
        return text.to_string();
    }

    let max_chars = MAX_TOOL_RESULT_TOKENS * 25 / 10;

    let byte_limit = text
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len());

    let cut_point = text[..byte_limit]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(byte_limit);

    let truncated = text.get(..cut_point).unwrap_or(text);
    format!("{}{}", truncated, TRUNCATION_SUFFIX)
}
