//! Intent classifier for Agent execution mode.
//!
//! Provides 3-level classification: regex → keywords → LLM

use once_cell::sync::Lazy;
use regex::Regex;

use super::task_category::TaskCategory;

/// Regex patterns for L1 classification (Chinese + English)
static EXECUTABLE_PATTERNS: Lazy<Vec<(Regex, TaskCategory)>> = Lazy::new(|| {
    vec![
        // FileOrganize: organize/sort/classify + file
        (
            Regex::new(r"(?i)(整理|归类|分类|organize|sort|classify).*(文件|files?|folder|文件夹)")
                .unwrap(),
            TaskCategory::FileOrganize,
        ),
        // FileTransfer: move/copy/transfer + to
        (
            Regex::new(r"(?i)(移动|复制|拷贝|转移|move|copy|transfer).*(到|to)")
                .unwrap(),
            TaskCategory::FileTransfer,
        ),
        // FileCleanup: delete/remove/clean
        (
            Regex::new(r"(?i)(删除|清理|清空|清除|delete|remove|clean)")
                .unwrap(),
            TaskCategory::FileCleanup,
        ),
        // CodeExecution: run/execute + script/code
        (
            Regex::new(r"(?i)(运行|执行|跑一下|run|execute).*(脚本|代码|script|code)")
                .unwrap(),
            TaskCategory::CodeExecution,
        ),
        // DocumentGenerate: generate/create/export + document/report
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

/// Keyword sets for L2 classification
struct KeywordSet {
    verbs: &'static [&'static str],
    nouns: &'static [&'static str],
    category: TaskCategory,
}

/// Static keyword sets for L2 matching
static KEYWORD_SETS: &[KeywordSet] = &[
    KeywordSet {
        verbs: &["整理", "归类", "分类", "分", "organize", "sort", "classify"],
        nouns: &[
            "文件", "文件夹", "目录", "下载", "照片", "图片",
            "files", "folder", "directory", "downloads", "photos", "pictures",
        ],
        category: TaskCategory::FileOrganize,
    },
    KeywordSet {
        verbs: &["移动", "复制", "拷贝", "转移", "move", "copy", "transfer"],
        nouns: &["文件", "文件夹", "到", "files", "folder", "to"],
        category: TaskCategory::FileTransfer,
    },
    KeywordSet {
        verbs: &["删除", "清理", "清空", "移除", "delete", "remove", "clean", "clear"],
        nouns: &["文件", "缓存", "垃圾", "files", "cache", "trash"],
        category: TaskCategory::FileCleanup,
    },
    KeywordSet {
        verbs: &["运行", "执行", "跑", "run", "execute"],
        nouns: &["脚本", "代码", "程序", "script", "code", "program"],
        category: TaskCategory::CodeExecution,
    },
    KeywordSet {
        verbs: &["生成", "创建", "导出", "写", "generate", "create", "export", "write"],
        nouns: &["文档", "报告", "document", "report", "pdf"],
        category: TaskCategory::DocumentGenerate,
    },
];

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

    /// L2: Keyword + rule matching (<20ms)
    pub fn match_keywords(&self, input: &str) -> Option<ExecutableTask> {
        let input_lower = input.to_lowercase();

        for set in KEYWORD_SETS {
            let has_verb = set.verbs.iter().any(|v| input_lower.contains(v));
            let has_noun = set.nouns.iter().any(|n| input_lower.contains(n));

            if has_verb && has_noun {
                let target = self.extract_path(input);
                return Some(ExecutableTask {
                    category: set.category,
                    action: input.to_string(),
                    target,
                    confidence: 0.85, // Keyword match = good confidence
                });
            }
        }
        None
    }

    /// Main classification entry point
    /// Tries L1 → L2 → L3 in order, returns first match
    pub async fn classify(&self, input: &str) -> ExecutionIntent {
        // Skip very short inputs
        if input.trim().len() < 3 {
            return ExecutionIntent::Conversational;
        }

        // L1: Regex matching (<5ms)
        if let Some(task) = self.match_regex(input) {
            return ExecutionIntent::Executable(task);
        }

        // L2: Keyword matching (<20ms)
        if let Some(task) = self.match_keywords(input) {
            return ExecutionIntent::Executable(task);
        }

        // L3: LLM classification (future - for now return Conversational)
        // TODO: Implement LLM-based classification when needed
        ExecutionIntent::Conversational
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

    #[test]
    fn test_l2_keywords_file_organize() {
        let classifier = IntentClassifier::new();
        // This input doesn't match L1 regex exactly but has keywords
        let result = classifier.match_keywords("能不能帮忙把下载里的东西按类型分一下");
        assert!(result.is_some());
        let task = result.unwrap();
        assert_eq!(task.category, TaskCategory::FileOrganize);
        assert!(task.confidence < 1.0); // Lower confidence than regex
    }

    #[test]
    fn test_l2_keywords_file_cleanup() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("帮我清理一下缓存");
        assert!(result.is_some());
        let task = result.unwrap();
        assert_eq!(task.category, TaskCategory::FileCleanup);
    }

    #[test]
    fn test_l2_keywords_no_match() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("你好，请问你是谁");
        assert!(result.is_none());
    }

    #[test]
    fn test_l2_keywords_english() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("can you sort my folder contents");
        assert!(result.is_some());
        let task = result.unwrap();
        assert_eq!(task.category, TaskCategory::FileOrganize);
    }

    #[tokio::test]
    async fn test_classify_executable_l1() {
        let classifier = IntentClassifier::new();
        let result = classifier
            .classify("帮我整理一下/Downloads/文件夹里的文件")
            .await;
        assert!(matches!(result, ExecutionIntent::Executable(_)));
        if let ExecutionIntent::Executable(task) = result {
            assert_eq!(task.category, TaskCategory::FileOrganize);
            assert_eq!(task.confidence, 1.0); // L1 regex = full confidence
        }
    }

    #[tokio::test]
    async fn test_classify_executable_l2() {
        let classifier = IntentClassifier::new();
        let result = classifier
            .classify("能不能帮忙把下载里的东西按类型分一下")
            .await;
        assert!(matches!(result, ExecutionIntent::Executable(_)));
        if let ExecutionIntent::Executable(task) = result {
            assert_eq!(task.category, TaskCategory::FileOrganize);
            assert!(task.confidence < 1.0); // L2 = lower confidence
        }
    }

    #[tokio::test]
    async fn test_classify_conversational() {
        let classifier = IntentClassifier::new();
        let result = classifier.classify("你好").await;
        assert!(matches!(result, ExecutionIntent::Conversational));
    }

    #[tokio::test]
    async fn test_classify_short_input() {
        let classifier = IntentClassifier::new();
        let result = classifier.classify("hi").await;
        assert!(matches!(result, ExecutionIntent::Conversational));
    }
}
