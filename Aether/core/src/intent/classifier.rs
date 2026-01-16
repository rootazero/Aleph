//! Intent classifier for Agent execution mode.
//!
//! Provides 3-level classification: regex → keywords → LLM

use once_cell::sync::Lazy;
use regex::Regex;

use super::task_category::TaskCategory;

/// Regex patterns for L1 classification (Chinese + English)
static EXECUTABLE_PATTERNS: Lazy<Vec<(Regex, TaskCategory)>> = Lazy::new(|| {
    vec![
        // FileOrganize: 整理/归类/分类 + 文件
        (
            Regex::new(r"(?i)(整理|归类|分类|organize|sort|classify).*(文件|files?|folder|文件夹)")
                .unwrap(),
            TaskCategory::FileOrganize,
        ),
        // FileTransfer: 移动/复制/拷贝 + 到
        (
            Regex::new(r"(?i)(移动|复制|拷贝|转移|move|copy|transfer).*(到|to)")
                .unwrap(),
            TaskCategory::FileTransfer,
        ),
        // FileCleanup: 删除/清理/清空
        (
            Regex::new(r"(?i)(删除|清理|清空|清除|delete|remove|clean)")
                .unwrap(),
            TaskCategory::FileCleanup,
        ),
        // CodeExecution: 运行/执行 + 脚本/代码
        (
            Regex::new(r"(?i)(运行|执行|跑一下|run|execute).*(脚本|代码|script|code)")
                .unwrap(),
            TaskCategory::CodeExecution,
        ),
        // DocumentGenerate: 生成/创建/导出 + 文档/报告
        (
            Regex::new(r"(?i)(生成|创建|导出|写|generate|create|export).*(文档|报告|document|report)")
                .unwrap(),
            TaskCategory::DocumentGenerate,
        ),
    ]
});

/// Path extraction pattern
/// Matches Unix paths (/path or ~/path) and Windows paths (C:\path)
/// Stops at whitespace, quotes, or CJK characters (U+4E00-U+9FFF)
static PATH_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"['"]?([/~][A-Za-z0-9_./-]+|[A-Za-z]:\\[A-Za-z0-9_.\\/]+)['"]?"#).unwrap()
});

/// Result of intent classification
#[derive(Debug, Clone)]
pub enum ExecutionIntent {
    /// Directly executable task - trigger Agent mode
    Executable(ExecutableTask),
    /// Needs clarification - ask ONE question max
    Ambiguous {
        task_hint: String,
        clarification: String,
    },
    /// Pure conversation - normal chat flow
    Conversational,
}

/// An executable task with metadata
#[derive(Debug, Clone)]
pub struct ExecutableTask {
    /// Task category
    pub category: TaskCategory,
    /// Action description extracted from input
    pub action: String,
    /// Target path or object (if detected)
    pub target: Option<String>,
    /// Classification confidence (0.0-1.0)
    pub confidence: f32,
}

impl ExecutionIntent {
    /// Check if this intent is directly executable
    pub fn is_executable(&self) -> bool {
        matches!(self, Self::Executable(_))
    }

    /// Check if this intent needs clarification
    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous { .. })
    }

    /// Check if this is a conversational intent
    pub fn is_conversational(&self) -> bool {
        matches!(self, Self::Conversational)
    }
}

/// Intent classifier with 3-level classification
pub struct IntentClassifier {
    /// Confidence threshold for L2/L3 classification
    #[allow(dead_code)]
    confidence_threshold: f32,
}

impl IntentClassifier {
    /// Create a new intent classifier
    pub fn new() -> Self {
        Self {
            confidence_threshold: 0.7,
        }
    }

    /// L1: Regex pattern matching (<5ms)
    pub fn match_regex(&self, input: &str) -> Option<ExecutableTask> {
        for (pattern, category) in EXECUTABLE_PATTERNS.iter() {
            if pattern.is_match(input) {
                let target = self.extract_path(input);
                return Some(ExecutableTask {
                    category: *category,
                    action: input.to_string(),
                    target,
                    confidence: 1.0, // Regex match = high confidence
                });
            }
        }
        None
    }

    /// Extract file path from input
    fn extract_path(&self, input: &str) -> Option<String> {
        PATH_PATTERN.captures(input).map(|c| c[1].to_string())
    }
}

impl Default for IntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_intent_is_executable() {
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "整理文件".to_string(),
            target: Some("/Downloads".to_string()),
            confidence: 0.95,
        };
        let intent = ExecutionIntent::Executable(task);
        assert!(intent.is_executable());
        assert!(!intent.is_ambiguous());
        assert!(!intent.is_conversational());
    }

    #[test]
    fn test_execution_intent_ambiguous() {
        let intent = ExecutionIntent::Ambiguous {
            task_hint: "file operation".to_string(),
            clarification: "Which folder?".to_string(),
        };
        assert!(!intent.is_executable());
        assert!(intent.is_ambiguous());
        assert!(!intent.is_conversational());
    }

    #[test]
    fn test_execution_intent_conversational() {
        let intent = ExecutionIntent::Conversational;
        assert!(!intent.is_executable());
        assert!(!intent.is_ambiguous());
        assert!(intent.is_conversational());
    }

    #[test]
    fn test_l1_regex_file_organize() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_regex("帮我整理一下这个文件夹里的文件");
        assert!(result.is_some());
        let task = result.unwrap();
        assert_eq!(task.category, TaskCategory::FileOrganize);
        assert_eq!(task.confidence, 1.0);
    }

    #[test]
    fn test_l1_regex_file_transfer() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_regex("把这些文件移动到Documents目录");
        assert!(result.is_some());
        let task = result.unwrap();
        assert_eq!(task.category, TaskCategory::FileTransfer);
    }

    #[test]
    fn test_l1_regex_file_cleanup() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_regex("删除这些临时文件");
        assert!(result.is_some());
        let task = result.unwrap();
        assert_eq!(task.category, TaskCategory::FileCleanup);
    }

    #[test]
    fn test_l1_regex_no_match() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_regex("今天天气怎么样");
        assert!(result.is_none());
    }

    #[test]
    fn test_l1_regex_path_extraction() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_regex("帮我整理/Downloads/test文件夹里的文件");
        assert!(result.is_some());
        let task = result.unwrap();
        assert_eq!(task.target, Some("/Downloads/test".to_string()));
    }

    #[test]
    fn test_l1_regex_english() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_regex("organize files in this folder");
        assert!(result.is_some());
        let task = result.unwrap();
        assert_eq!(task.category, TaskCategory::FileOrganize);
    }
}
