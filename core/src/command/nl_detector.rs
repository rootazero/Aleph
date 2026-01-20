//! Natural Language Command Detector
//!
//! Detects command invocations from natural language input:
//! - L1: Explicit mention (e.g., "使用 X", "use X to")
//! - L2: Implicit intent (keyword matching via UnifiedCommandIndex)

use once_cell::sync::Lazy;
use regex::Regex;

use crate::command::unified_index::UnifiedCommandIndex;
use crate::dispatcher::ToolSourceType;

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

/// Detection type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionType {
    /// Explicit mention (e.g., "使用 X", "use X")
    Explicit,
    /// Implicit intent (keyword matching)
    Implicit,
}

/// Detection result
#[derive(Debug, Clone, PartialEq)]
pub struct NLDetection {
    /// Command name that was detected
    pub command_name: String,
    /// Source type of the command
    pub source_type: ToolSourceType,
    /// How it was detected
    pub detection_type: DetectionType,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// Remaining input after command extraction (for explicit)
    pub remaining_input: Option<String>,
}

/// Natural language command detector
pub struct NaturalLanguageCommandDetector {
    /// Unified command index for lookups
    index: UnifiedCommandIndex,
    /// Minimum confidence threshold for implicit detection
    min_confidence: f64,
}

impl NaturalLanguageCommandDetector {
    /// Create a new detector with the given index
    pub fn new(index: UnifiedCommandIndex) -> Self {
        Self {
            index,
            min_confidence: 0.3,
        }
    }

    /// Set minimum confidence threshold for implicit detection
    pub fn with_min_confidence(mut self, threshold: f64) -> Self {
        self.min_confidence = threshold;
        self
    }

    /// Detect command from natural language input
    pub fn detect(&self, input: &str) -> Option<NLDetection> {
        // L1: Try explicit detection first
        if let Some(detection) = self.detect_explicit(input) {
            return Some(detection);
        }

        // L2: Try implicit detection
        self.detect_implicit(input)
    }

    /// L1: Explicit command detection
    fn detect_explicit(&self, input: &str) -> Option<NLDetection> {
        let (command_name, remaining) = extract_explicit_command(input)?;

        // Verify command exists in index
        let matches = self.index.find_matches(&command_name);

        // If exact match found, use it
        if let Some(m) = matches
            .iter()
            .find(|m| m.command_name.eq_ignore_ascii_case(&command_name))
        {
            return Some(NLDetection {
                command_name: m.command_name.clone(),
                source_type: m.source_type,
                detection_type: DetectionType::Explicit,
                confidence: 1.0,
                remaining_input: remaining,
            });
        }

        // Otherwise, return the command name as-is (let caller verify)
        Some(NLDetection {
            command_name,
            source_type: ToolSourceType::Custom, // Default, caller should verify
            detection_type: DetectionType::Explicit,
            confidence: 1.0,
            remaining_input: remaining,
        })
    }

    /// L2: Implicit intent detection
    fn detect_implicit(&self, input: &str) -> Option<NLDetection> {
        let matches = self.index.find_matches(input);

        // Get best match above threshold
        let best = matches.into_iter().next()?;

        // Normalize score
        let normalized_score = (best.score / 3.0).min(1.0); // Assume max 3 trigger matches

        if normalized_score >= self.min_confidence {
            Some(NLDetection {
                command_name: best.command_name,
                source_type: best.source_type,
                detection_type: DetectionType::Implicit,
                confidence: normalized_score,
                remaining_input: Some(input.to_string()),
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandTriggers, UnifiedCommandIndex};
    use crate::dispatcher::ToolSourceType;

    #[test]
    fn test_nl_detector_explicit() {
        let mut index = UnifiedCommandIndex::new();
        let triggers = CommandTriggers::new(vec!["graph".to_string()], Vec::new());
        index.add_command(ToolSourceType::Skill, "knowledge-graph", &triggers);

        let detector = NaturalLanguageCommandDetector::new(index);

        let result = detector.detect("使用 knowledge-graph 分析代码");
        assert!(result.is_some());
        let detection = result.unwrap();
        assert_eq!(detection.command_name, "knowledge-graph");
        assert_eq!(detection.detection_type, DetectionType::Explicit);
        assert_eq!(detection.confidence, 1.0);
    }

    #[test]
    fn test_nl_detector_implicit() {
        let mut index = UnifiedCommandIndex::new();
        let triggers = CommandTriggers::new(vec!["知识图谱".to_string()], Vec::new());
        index.add_command(ToolSourceType::Skill, "knowledge-graph", &triggers);

        let detector = NaturalLanguageCommandDetector::new(index);

        let result = detector.detect("帮我画个知识图谱");
        assert!(result.is_some());
        let detection = result.unwrap();
        assert_eq!(detection.command_name, "knowledge-graph");
        assert_eq!(detection.detection_type, DetectionType::Implicit);
    }

    #[test]
    fn test_nl_detector_no_match() {
        let index = UnifiedCommandIndex::new();
        let detector = NaturalLanguageCommandDetector::new(index);

        let result = detector.detect("今天天气怎么样");
        assert!(result.is_none());
    }

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
