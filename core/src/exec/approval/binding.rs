//! Parameter binding validation
//!
//! This module provides functionality to validate that runtime parameters
//! match their declared bindings, preventing tools from accessing resources
//! beyond their declared scope.

use super::types::EscalationTrigger;
use super::types::EscalationReason;
use std::collections::HashMap;

/// Check if runtime parameters comply with declared bindings
///
/// # Arguments
/// * `runtime_params` - Actual parameters received at runtime
/// * `declared_bindings` - Expected parameter bindings declared by the tool
///
/// # Returns
/// * `Ok(())` if all parameters match their bindings
/// * `Err(EscalationTrigger)` if any binding violation is detected
///
/// # Binding Types
/// * **Fixed**: Exact value match (e.g., "/tmp/output.txt")
/// * **Pattern**: Glob pattern match (e.g., "/tmp/*.txt", "/tmp/**/*.txt")
/// * **Range**: Numeric range match (e.g., "8000-9000")
pub fn check_binding_compliance(
    runtime_params: &HashMap<String, String>,
    declared_bindings: &HashMap<String, String>,
) -> Result<(), EscalationTrigger> {
    // Check for missing required bindings
    for (param_name, _binding_value) in declared_bindings {
        if !runtime_params.contains_key(param_name) {
            return Err(EscalationTrigger {
                reason: EscalationReason::UndeclaredBinding,
                requested_path: None,
                approved_paths: vec![],
            });
        }
    }

    // Check for extra undeclared parameters
    for (param_name, _param_value) in runtime_params {
        if !declared_bindings.contains_key(param_name) {
            return Err(EscalationTrigger {
                reason: EscalationReason::UndeclaredBinding,
                requested_path: None,
                approved_paths: vec![],
            });
        }
    }

    // Validate each parameter against its binding
    for (param_name, param_value) in runtime_params {
        if let Some(binding_value) = declared_bindings.get(param_name) {
            if !matches_binding(param_value, binding_value) {
                return Err(EscalationTrigger {
                    reason: EscalationReason::UndeclaredBinding,
                    requested_path: None,
                    approved_paths: vec![],
                });
            }
        }
    }

    Ok(())
}

/// Check if a value matches a binding specification
///
/// Supports three binding types:
/// 1. Fixed: Exact string match
/// 2. Pattern: Glob pattern (contains * or **)
/// 3. Range: Numeric range (format: "min-max")
fn matches_binding(value: &str, binding: &str) -> bool {
    // Check if it's a range binding (numeric range)
    if binding.contains('-') && !binding.contains('/') && !binding.contains('*') {
        if let Some((start, end)) = binding.split_once('-') {
            if let (Ok(start_num), Ok(end_num), Ok(value_num)) = (
                start.trim().parse::<i64>(),
                end.trim().parse::<i64>(),
                value.parse::<i64>(),
            ) {
                return value_num >= start_num && value_num <= end_num;
            }
        }
    }

    // Check if it's a pattern binding (contains wildcards)
    if binding.contains('*') {
        return matches_pattern(value, binding);
    }

    // Fixed binding: exact match
    value == binding
}

/// Check if a value matches a glob pattern
///
/// Supports:
/// * `*` - matches any characters except /
/// * `**` - matches any characters including /
fn matches_pattern(value: &str, pattern: &str) -> bool {
    // Convert glob pattern to regex
    let mut regex_pattern = String::new();
    regex_pattern.push('^');

    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if chars.peek() == Some(&'*') {
                    // ** matches everything including /
                    chars.next();
                    regex_pattern.push_str(".*");
                } else {
                    // * matches everything except /
                    regex_pattern.push_str("[^/]*");
                }
            }
            '?' => regex_pattern.push('.'),
            '.' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '+' | '\\' => {
                regex_pattern.push('\\');
                regex_pattern.push(ch);
            }
            _ => regex_pattern.push(ch),
        }
    }

    regex_pattern.push('$');

    // Use simple regex matching
    if let Ok(re) = regex::Regex::new(&regex_pattern) {
        re.is_match(value)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_binding_fixed() {
        assert!(matches_binding("/tmp/output.txt", "/tmp/output.txt"));
        assert!(!matches_binding("/tmp/output.txt", "/tmp/input.txt"));
    }

    #[test]
    fn test_matches_binding_pattern() {
        assert!(matches_binding("/tmp/output.txt", "/tmp/*.txt"));
        assert!(!matches_binding("/tmp/output.json", "/tmp/*.txt"));
        assert!(matches_binding("/tmp/subdir/output.txt", "/tmp/**/*.txt"));
        assert!(!matches_binding("/tmp/subdir/output.json", "/tmp/**/*.txt"));
    }

    #[test]
    fn test_matches_binding_range() {
        assert!(matches_binding("8080", "8000-9000"));
        assert!(matches_binding("8000", "8000-9000"));
        assert!(matches_binding("9000", "8000-9000"));
        assert!(!matches_binding("7999", "8000-9000"));
        assert!(!matches_binding("9001", "8000-9000"));
    }

    #[test]
    fn test_matches_pattern_simple() {
        assert!(matches_pattern("file.txt", "*.txt"));
        assert!(!matches_pattern("file.json", "*.txt"));
    }

    #[test]
    fn test_matches_pattern_with_path() {
        assert!(matches_pattern("/tmp/file.txt", "/tmp/*.txt"));
        assert!(!matches_pattern("/tmp/subdir/file.txt", "/tmp/*.txt"));
        assert!(matches_pattern("/tmp/subdir/file.txt", "/tmp/**/*.txt"));
    }
}
