//! Natural Language Command Detector
//!
//! Detects command invocations from natural language input:
//! - L1: Explicit mention (e.g., "使用 X", "use X to")
//! - L2: Implicit intent (keyword matching via UnifiedCommandIndex)

use once_cell::sync::Lazy;
use regex::Regex;

/// Explicit command mention patterns
/// Each tuple: (pattern, command_name_group_index)
static EXPLICIT_PATTERNS: Lazy<Vec<(Regex, usize)>> = Lazy::new(|| {
    vec![
        // Chinese: 使用/用/调用/执行/运行 X ...
        (Regex::new(r"(?i)^(使用|用|调用|执行|运行)\s*[「\[「]?([a-zA-Z0-9_-]+)[」\]」]?\s*(.*)$").unwrap(), 2),

        // Chinese: 让/交给 X 来/处理/做
        (Regex::new(r"(?i)(让|交给)\s*[「\[「]?([a-zA-Z0-9_-]+)[」\]」]?\s*(来|处理|做|帮)(.*)$").unwrap(), 2),

        // English: use/invoke/call/run/execute X to/for ...
        (Regex::new(r"(?i)^(use|invoke|call|run|execute)\s+([a-zA-Z0-9_-]+)\s+(to\s+|for\s+)?(.*)$").unwrap(), 2),

        // English: ask/let X to ...
        (Regex::new(r"(?i)(ask|let)\s+([a-zA-Z0-9_-]+)\s+(to\s+)(.*)$").unwrap(), 2),

        // English: with/using X, ...
        (Regex::new(r"(?i)(with|using)\s+([a-zA-Z0-9_-]+)[,\s]+(.*)$").unwrap(), 2),
    ]
});

/// Extract command name from explicit mention patterns
/// Returns (command_name, remaining_input) if matched
pub fn extract_explicit_command(input: &str) -> Option<(String, Option<String>)> {
    let trimmed = input.trim();

    for (pattern, cmd_group) in EXPLICIT_PATTERNS.iter() {
        if let Some(captures) = pattern.captures(trimmed) {
            let command_name = captures.get(*cmd_group)?.as_str().to_string();

            // Get remaining input (last capture group typically)
            let remaining = captures
                .get(captures.len() - 1)
                .map(|m| m.as_str().trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            return Some((command_name, remaining));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explicit_pattern_chinese_use() {
        let result = extract_explicit_command("使用 knowledge-graph 分析代码");
        assert!(result.is_some());
        let (cmd, remaining) = result.unwrap();
        assert_eq!(cmd, "knowledge-graph");
        assert!(remaining.is_some());
    }

    #[test]
    fn test_explicit_pattern_chinese_use_short() {
        let result = extract_explicit_command("用 translate 翻译这段话");
        assert!(result.is_some());
        let (cmd, _) = result.unwrap();
        assert_eq!(cmd, "translate");
    }

    #[test]
    fn test_explicit_pattern_english_use() {
        let result = extract_explicit_command("use knowledge-graph to analyze dependencies");
        assert!(result.is_some());
        let (cmd, remaining) = result.unwrap();
        assert_eq!(cmd, "knowledge-graph");
        assert!(remaining.is_some());
    }

    #[test]
    fn test_explicit_pattern_english_invoke() {
        let result = extract_explicit_command("invoke translator for this text");
        assert!(result.is_some());
        let (cmd, _) = result.unwrap();
        assert_eq!(cmd, "translator");
    }

    #[test]
    fn test_explicit_pattern_no_match() {
        let result = extract_explicit_command("帮我分析一下这段代码");
        assert!(result.is_none());
    }
}
