//! JSON Repair - Greedy repair logic for truncated JSON
//!
//! This module attempts to repair incomplete JSON by closing unclosed
//! brackets, quotes, and removing trailing dangling commas.

use serde_json::Value;

/// Attempt to repair truncated JSON
///
/// This function uses a greedy approach to fix common JSON truncation issues:
/// 1. Removes trailing dangling commas
/// 2. Closes unclosed strings
/// 3. Closes unclosed brackets (] before })
///
/// Returns Some(repaired) if the repair produces valid JSON, None otherwise.
///
/// # Example
///
/// ```
/// use alephcore::dispatcher::cortex::parser::try_repair;
///
/// let incomplete = r#"{"name": "test""#;
/// let repaired = try_repair(incomplete);
/// assert!(repaired.is_some());
/// ```
pub fn try_repair(incomplete: &str) -> Option<String> {
    if incomplete.is_empty() {
        return None;
    }

    let mut repaired = incomplete.to_string();

    // Step 1: Remove trailing dangling comma
    let trimmed = repaired.trim_end();
    if let Some(stripped) = trimmed.strip_suffix(',') {
        repaired = stripped.to_string();
    }

    // Step 2: Track nesting order with a stack and detect unclosed strings
    let mut stack = Vec::new();
    let mut in_string = false;
    let mut escape_next = false;

    for ch in repaired.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        if ch == '\\' && in_string {
            escape_next = true;
            continue;
        }

        if ch == '"' {
            in_string = !in_string;
            continue;
        }

        if !in_string {
            match ch {
                '{' => stack.push('}'),
                '[' => stack.push(']'),
                '}' | ']' => { stack.pop(); },
                _ => {}
            }
        }
    }

    // Step 3: Close unclosed string
    if in_string {
        repaired.push('"');
    }

    // Step 4: Close unclosed brackets/braces in reverse nesting order
    while let Some(closer) = stack.pop() {
        repaired.push(closer);
    }

    // Step 5: Validate result
    if serde_json::from_str::<Value>(&repaired).is_ok() {
        Some(repaired)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repair_missing_brace() {
        let incomplete = r#"{"name": "test""#;
        let repaired = try_repair(incomplete);

        assert!(repaired.is_some());
        let repaired = repaired.unwrap();

        // Verify it's valid JSON
        let value: Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(value["name"], "test");
    }

    #[test]
    fn test_repair_missing_bracket() {
        let incomplete = "[1, 2, 3";
        let repaired = try_repair(incomplete);

        assert!(repaired.is_some());
        let repaired = repaired.unwrap();
        assert_eq!(repaired, "[1, 2, 3]");

        // Verify it's valid JSON
        let value: Value = serde_json::from_str(&repaired).unwrap();
        assert!(value.is_array());
        assert_eq!(value.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_repair_trailing_comma() {
        let incomplete = r#"{"a": 1,"#;
        let repaired = try_repair(incomplete);

        assert!(repaired.is_some());
        let repaired = repaired.unwrap();

        // Verify it's valid JSON
        let value: Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn test_repair_unclosed_string() {
        let incomplete = r#"{"name": "test"#;
        let repaired = try_repair(incomplete);

        assert!(repaired.is_some());
        let repaired = repaired.unwrap();

        // Verify it's valid JSON
        let value: Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(value["name"], "test");
    }

    #[test]
    fn test_repair_nested() {
        let incomplete = r#"{"outer": {"inner": [1, 2"#;
        let repaired = try_repair(incomplete);

        assert!(repaired.is_some());
        let repaired = repaired.unwrap();

        // Verify it's valid JSON with nested structure
        let value: Value = serde_json::from_str(&repaired).unwrap();
        assert!(value["outer"]["inner"].is_array());
    }

    #[test]
    fn test_repair_hopeless() {
        let incomplete = "not json at all";
        let repaired = try_repair(incomplete);

        assert!(repaired.is_none());
    }
}
