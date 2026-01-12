//! Quick Heuristics for Multi-Step Detection
//!
//! Fast detection (<10ms) to identify inputs that likely require
//! multi-step execution before invoking the LLM.
//!
//! # Design
//!
//! The heuristics use simple pattern matching to detect:
//! - Multiple action verbs (e.g., "translate", "summarize")
//! - Connector words indicating sequence (e.g., "then", "然后")
//!
//! This avoids expensive LLM calls for simple single-tool requests.

use std::time::Instant;

/// Quick heuristics detector for multi-step task detection
///
/// Provides fast detection (<10ms) to identify inputs that likely
/// require multi-step execution.
pub struct QuickHeuristics;

/// Result of heuristics detection
#[derive(Debug, Clone)]
pub struct HeuristicsResult {
    /// Whether input is likely multi-step
    pub is_likely_multi_step: bool,
    /// Number of action verbs detected
    pub action_count: usize,
    /// Whether connector words were found
    pub has_connector: bool,
    /// Detection latency in microseconds
    pub latency_us: u64,
    /// Matched action words (for debugging)
    pub matched_actions: Vec<String>,
    /// Matched connectors (for debugging)
    pub matched_connectors: Vec<String>,
}

impl QuickHeuristics {
    // Chinese action verbs
    const CN_ACTIONS: &'static [&'static str] = &[
        "翻译", "总结", "发送", "保存", "搜索", "分析", "生成", "创建", "删除", "移动", "复制",
        "格式化", "转换", "提取", "压缩", "解压", "合并", "拆分", "下载", "上传", "编辑", "修改",
        "读取", "写入", "运行", "执行", "计算", "查找", "替换", "排序", "过滤", "统计",
    ];

    // English action verbs
    const EN_ACTIONS: &'static [&'static str] = &[
        "translate",
        "summarize",
        "send",
        "save",
        "search",
        "analyze",
        "generate",
        "create",
        "delete",
        "move",
        "copy",
        "format",
        "convert",
        "extract",
        "compress",
        "decompress",
        "merge",
        "split",
        "download",
        "upload",
        "edit",
        "modify",
        "read",
        "write",
        "run",
        "execute",
        "calculate",
        "find",
        "replace",
        "sort",
        "filter",
        "count",
    ];

    // Chinese connector words indicating sequence
    const CN_CONNECTORS: &'static [&'static str] = &[
        "然后", "接着", "之后", "并且", "同时", "再", "并", "以及", "还要", "最后", "首先",
        "其次", "接下来", "随后",
    ];

    // English connector words indicating sequence
    const EN_CONNECTORS: &'static [&'static str] = &[
        "then",
        "and then",
        "after that",
        "also",
        "next",
        "afterwards",
        "finally",
        "first",
        "second",
        "third",
        "subsequently",
        "followed by",
        "as well as",
        "in addition",
        "plus",
        "and also",
    ];

    /// Check if input likely requires multi-step execution
    ///
    /// Returns `true` if:
    /// - 2+ action verbs are detected, OR
    /// - A connector word is present with 1+ action verb
    ///
    /// # Performance
    ///
    /// This method is designed to complete in <10ms for typical inputs.
    /// It uses simple string contains checks, no regex or complex parsing.
    pub fn is_likely_multi_step(input: &str) -> bool {
        Self::analyze(input).is_likely_multi_step
    }

    /// Analyze input with detailed results
    ///
    /// Returns a detailed analysis including:
    /// - Action verb count
    /// - Connector detection
    /// - Matched words (for debugging)
    /// - Processing latency
    pub fn analyze(input: &str) -> HeuristicsResult {
        let start = Instant::now();

        let input_lower = input.to_lowercase();
        let mut matched_actions = Vec::new();
        let mut matched_connectors = Vec::new();

        // Count action words (both Chinese and English)
        for action in Self::CN_ACTIONS.iter().chain(Self::EN_ACTIONS.iter()) {
            if input_lower.contains(*action) || input.contains(*action) {
                matched_actions.push((*action).to_string());
            }
        }

        // Check for connectors
        for connector in Self::CN_CONNECTORS.iter().chain(Self::EN_CONNECTORS.iter()) {
            if input_lower.contains(*connector) || input.contains(*connector) {
                matched_connectors.push((*connector).to_string());
            }
        }

        let action_count = matched_actions.len();
        let has_connector = !matched_connectors.is_empty();

        // Multi-step if: 2+ actions OR connector present with 1+ action
        let is_likely_multi_step = action_count >= 2 || (has_connector && action_count >= 1);

        let latency_us = start.elapsed().as_micros() as u64;

        HeuristicsResult {
            is_likely_multi_step,
            action_count,
            has_connector,
            latency_us,
            matched_actions,
            matched_connectors,
        }
    }

    /// Analyze with timeout check
    ///
    /// Returns an error if analysis takes longer than the specified timeout.
    /// This is mainly for defensive purposes - analysis should never exceed 10ms.
    pub fn analyze_with_timeout(input: &str, timeout_us: u64) -> Result<HeuristicsResult, String> {
        let result = Self::analyze(input);
        if result.latency_us > timeout_us {
            Err(format!(
                "Heuristics analysis exceeded timeout: {}us > {}us",
                result.latency_us, timeout_us
            ))
        } else {
            Ok(result)
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chinese_multi_action() {
        // "Translate this document to English, then summarize the key points"
        let result = QuickHeuristics::analyze("把这个文档翻译成英文，然后总结要点");
        assert!(result.is_likely_multi_step);
        assert!(result.action_count >= 2); // translate, summarize
        assert!(result.has_connector); // "then" connector
    }

    #[test]
    fn test_english_multi_action() {
        let result =
            QuickHeuristics::analyze("Search for the latest news about AI, then summarize the top 3 articles");
        assert!(result.is_likely_multi_step);
        assert!(result.action_count >= 2); // search, summarize
        assert!(result.has_connector); // then
    }

    #[test]
    fn test_simple_single_tool() {
        // "Translate this text"
        let result = QuickHeuristics::analyze("翻译这段文字");
        assert!(!result.is_likely_multi_step);
        assert_eq!(result.action_count, 1); // translate
        assert!(!result.has_connector);
    }

    #[test]
    fn test_english_single_tool() {
        let result = QuickHeuristics::analyze("search for weather in Tokyo");
        assert!(!result.is_likely_multi_step);
        assert_eq!(result.action_count, 1); // search
        assert!(!result.has_connector);
    }

    #[test]
    fn test_connector_with_single_action() {
        // "Then translate it" - connector + 1 action
        let result = QuickHeuristics::analyze("然后翻译它");
        assert!(result.is_likely_multi_step);
        assert!(result.has_connector); // "then" connector
        assert_eq!(result.action_count, 1); // translate
    }

    #[test]
    fn test_no_action_words() {
        let result = QuickHeuristics::analyze("Hello, how are you?");
        assert!(!result.is_likely_multi_step);
        assert_eq!(result.action_count, 0);
        assert!(!result.has_connector);
    }

    #[test]
    fn test_multiple_actions_no_connector() {
        // "Translate and summarize this"
        let result = QuickHeuristics::analyze("translate and summarize this");
        assert!(result.is_likely_multi_step);
        assert!(result.action_count >= 2); // translate, summarize
    }

    #[test]
    fn test_mixed_language() {
        // Mixed language: "search" + Chinese "then summarize"
        let result = QuickHeuristics::analyze("search然后总结");
        assert!(result.is_likely_multi_step);
        assert!(result.action_count >= 2); // search, summarize
        assert!(result.has_connector); // "then" connector
    }

    #[test]
    fn test_complex_sequence() {
        let result = QuickHeuristics::analyze(
            "first search for data, then analyze the results, and finally generate a report",
        );
        assert!(result.is_likely_multi_step);
        assert!(result.action_count >= 3); // search, analyze, generate
        assert!(result.has_connector); // first, then, finally
    }

    #[test]
    fn test_performance() {
        // Test that analysis completes in reasonable time
        let input = "翻译这段文字然后总结要点并发送给用户";
        let result = QuickHeuristics::analyze(input);

        // Should complete in less than 10ms (10000us)
        assert!(
            result.latency_us < 10_000,
            "Analysis took {}us, expected <10000us",
            result.latency_us
        );
    }

    #[test]
    fn test_timeout_check() {
        let result =
            QuickHeuristics::analyze_with_timeout("translate this", 100_000); // 100ms timeout
        assert!(result.is_ok());

        // This should basically never fail with a realistic timeout
        let result2 = QuickHeuristics::analyze_with_timeout("translate this", 1); // 1us timeout
                                                                                  // This might or might not fail depending on system speed
        let _ = result2; // Just verify it doesn't panic
    }

    #[test]
    fn test_case_insensitivity() {
        let result1 = QuickHeuristics::analyze("TRANSLATE and SUMMARIZE");
        let result2 = QuickHeuristics::analyze("translate and summarize");

        assert_eq!(
            result1.is_likely_multi_step,
            result2.is_likely_multi_step
        );
        assert_eq!(result1.action_count, result2.action_count);
    }

    #[test]
    fn test_empty_input() {
        let result = QuickHeuristics::analyze("");
        assert!(!result.is_likely_multi_step);
        assert_eq!(result.action_count, 0);
        assert!(!result.has_connector);
    }

    #[test]
    fn test_special_characters() {
        let result = QuickHeuristics::analyze("@#$%^&*() translate!!! then??? summarize...");
        assert!(result.is_likely_multi_step);
        assert!(result.action_count >= 2);
    }
}
