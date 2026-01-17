//! Intent classifier for Agent execution mode.
//!
//! Provides 3-level classification: regex → keywords → LLM

use once_cell::sync::Lazy;
use regex::Regex;

use super::keyword::{KeywordIndex, KeywordMatchMode, KeywordRule};
use super::task_category::TaskCategory;
use crate::config::{KeywordPolicy, PolicyKeywordRule};

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

/// Exclusion patterns - inputs containing these should NOT trigger agent mode
/// These are non-executable verbs that indicate analysis/understanding rather than action
static EXCLUSION_VERBS: &[&str] = &[
    // Chinese analysis/understanding verbs
    "分析", "理解", "解释", "描述", "识别", "检测", "看看", "看一下", "看下",
    "告诉我", "说说", "讲讲", "什么是", "是什么", "怎么样",
    // Chinese summarization verbs
    "总结", "摘要", "概括", "归纳", "提炼", "概述", "梳理", "提取要点",
    // English analysis verbs
    "analyze", "analyse", "understand", "explain", "describe", "identify",
    "detect", "recognize", "what is", "tell me", "look at",
    // English summarization verbs
    "summarize", "summarise", "summary", "abstract", "recap", "outline",
    "extract", "highlight", "key points",
];

/// Static keyword sets for L2 matching
static KEYWORD_SETS: &[KeywordSet] = &[
    KeywordSet {
        // Removed "分" - too short, causes false matches (e.g., "分析" contains "分")
        verbs: &["整理", "归类", "分类", "organize", "sort", "classify"],
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
    /// Keyword index for enhanced L2 matching
    keyword_index: KeywordIndex,
}

impl IntentClassifier {
    /// Create a new intent classifier
    pub fn new() -> Self {
        Self {
            confidence_threshold: 0.7,
            keyword_index: KeywordIndex::new(),
        }
    }

    /// Create classifier with keyword policy from config
    pub fn with_keyword_policy(policy: &KeywordPolicy) -> Self {
        let mut classifier = Self::new();
        if policy.enabled {
            classifier.load_keyword_rules(&policy.rules);
        }
        classifier
    }

    /// Load keyword rules from config
    fn load_keyword_rules(&mut self, rules: &[PolicyKeywordRule]) {
        for rule_config in rules {
            let mode = match rule_config.match_mode.as_str() {
                "all" => KeywordMatchMode::All,
                "weighted" => KeywordMatchMode::Weighted,
                _ => KeywordMatchMode::Any,
            };

            let mut rule = KeywordRule::new(&rule_config.id, &rule_config.intent_type);
            for kw in &rule_config.keywords {
                rule = rule.with_keyword(&kw.word, kw.weight);
            }
            rule = rule.with_match_mode(mode).with_min_score(rule_config.min_score);

            self.keyword_index.add_rule(rule);
        }
    }

    /// L1: Regex pattern matching (<5ms)
    pub fn match_regex(&self, input: &str) -> Option<ExecutableTask> {
        let input_lower = input.to_lowercase();

        // Check exclusion patterns first - analysis/understanding verbs override regex matches
        if self.contains_exclusion_verb(&input_lower) {
            return None;
        }

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

        // Check exclusion patterns first - if input contains analysis/understanding verbs,
        // it should NOT trigger agent mode (e.g., "分析图片" is analysis, not file operation)
        if self.contains_exclusion_verb(&input_lower) {
            return None;
        }

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

    /// Check if input contains exclusion verbs (analysis/understanding actions)
    fn contains_exclusion_verb(&self, input: &str) -> bool {
        EXCLUSION_VERBS.iter().any(|v| input.contains(v))
    }

    /// L2 Enhanced: Use KeywordIndex for weighted matching
    pub fn match_keywords_enhanced(&self, input: &str) -> Option<ExecutableTask> {
        // Check exclusion patterns first
        if self.contains_exclusion_verb(&input.to_lowercase()) {
            return None;
        }

        // Try keyword index
        if let Some(km) = self.keyword_index.best_match(input, 0.5) {
            if let Some(category) = self.intent_type_to_category(&km.intent_type) {
                let target = self.extract_path(input);
                return Some(ExecutableTask {
                    category,
                    action: input.to_string(),
                    target,
                    confidence: km.score,
                });
            }
        }
        None
    }

    /// Convert intent type string to TaskCategory
    fn intent_type_to_category(&self, intent_type: &str) -> Option<TaskCategory> {
        match intent_type {
            "FileOrganize" => Some(TaskCategory::FileOrganize),
            "FileTransfer" => Some(TaskCategory::FileTransfer),
            "FileCleanup" => Some(TaskCategory::FileCleanup),
            "CodeExecution" => Some(TaskCategory::CodeExecution),
            "DocumentGenerate" => Some(TaskCategory::DocumentGenerate),
            _ => None,
        }
    }

    /// Main classification entry point
    /// Tries L1 → L2 Enhanced → L2 Fallback → L3 in order, returns first match
    pub async fn classify(&self, input: &str) -> ExecutionIntent {
        // Skip very short inputs
        if input.trim().len() < 3 {
            return ExecutionIntent::Conversational;
        }

        // L1: Regex matching (<5ms)
        if let Some(task) = self.match_regex(input) {
            return ExecutionIntent::Executable(task);
        }

        // L2 Enhanced: KeywordIndex matching
        if let Some(task) = self.match_keywords_enhanced(input) {
            return ExecutionIntent::Executable(task);
        }

        // L2 Fallback: Static keyword matching
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
        // Use "整理" (organize) verb which is more explicit than ambiguous "分"
        let result = classifier.match_keywords("能不能帮忙整理一下下载目录");
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
        // Use clearer expression with "整理" instead of ambiguous "分"
        let result = classifier
            .classify("能不能帮忙整理一下下载里的东西")
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

    // Tests for exclusion patterns - analysis/understanding requests should NOT trigger agent mode

    #[test]
    fn test_exclusion_analyze_image_chinese() {
        let classifier = IntentClassifier::new();
        // "分析图片" should be conversational (analysis), not file operation
        let result = classifier.match_keywords("分析这幅图片");
        assert!(result.is_none(), "Analysis requests should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_analyze_image_english() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("analyze this picture");
        assert!(result.is_none(), "Analysis requests should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_describe_photo() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("描述一下这张照片");
        assert!(result.is_none(), "Description requests should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_explain_file() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("解释这个文件的内容");
        assert!(result.is_none(), "Explanation requests should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_what_is_image() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("这张图片是什么");
        assert!(result.is_none(), "Question about content should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_look_at_photo() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("看看这张照片");
        assert!(result.is_none(), "Look at requests should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_summarize_document_chinese() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("总结这个文档");
        assert!(result.is_none(), "Summarization requests should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_summarize_webpage() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("帮我总结一下这个网页");
        assert!(result.is_none(), "Webpage summarization should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_abstract_file() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("给这个文件写个摘要");
        assert!(result.is_none(), "Abstract requests should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_summarize_english() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("summarize this document");
        assert!(result.is_none(), "English summarization should not trigger agent mode");
    }

    #[test]
    fn test_exclusion_outline_file() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("概括一下这个文件的内容");
        assert!(result.is_none(), "Outline requests should not trigger agent mode");
    }

    #[tokio::test]
    async fn test_classify_analyze_image_is_conversational() {
        let classifier = IntentClassifier::new();
        let result = classifier.classify("分析这幅图片").await;
        assert!(
            matches!(result, ExecutionIntent::Conversational),
            "分析图片 should be classified as Conversational, not Executable"
        );
    }

    #[tokio::test]
    async fn test_classify_describe_photo_is_conversational() {
        let classifier = IntentClassifier::new();
        let result = classifier.classify("描述这张照片里有什么").await;
        assert!(
            matches!(result, ExecutionIntent::Conversational),
            "描述照片 should be classified as Conversational"
        );
    }

    // Ensure real file operations still work

    #[test]
    fn test_real_file_organize_still_works() {
        let classifier = IntentClassifier::new();
        // Clear file organize request should still work
        let result = classifier.match_keywords("帮我整理下载文件夹");
        assert!(result.is_some(), "Clear file organize requests should still work");
        assert_eq!(result.unwrap().category, TaskCategory::FileOrganize);
    }

    #[test]
    fn test_real_file_cleanup_still_works() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("清理一下缓存文件");
        assert!(result.is_some(), "Clear file cleanup requests should still work");
        assert_eq!(result.unwrap().category, TaskCategory::FileCleanup);
    }

    // Tests for KeywordIndex integration (enhanced L2 matching)

    #[test]
    fn test_with_keyword_policy() {
        use crate::config::KeywordPolicy;
        let policy = KeywordPolicy::with_builtin_rules();
        let classifier = IntentClassifier::with_keyword_policy(&policy);

        // Test enhanced matching works
        let result = classifier.match_keywords_enhanced("帮我整理文件");
        assert!(result.is_some());
        assert_eq!(result.unwrap().category, TaskCategory::FileOrganize);
    }

    #[test]
    fn test_enhanced_keywords_exclusion() {
        use crate::config::KeywordPolicy;
        let policy = KeywordPolicy::with_builtin_rules();
        let classifier = IntentClassifier::with_keyword_policy(&policy);

        // Analysis should NOT trigger
        let result = classifier.match_keywords_enhanced("分析这个文件");
        assert!(result.is_none());
    }

    #[test]
    fn test_enhanced_keywords_file_cleanup() {
        use crate::config::KeywordPolicy;
        let policy = KeywordPolicy::with_builtin_rules();
        let classifier = IntentClassifier::with_keyword_policy(&policy);

        // File cleanup should work
        let result = classifier.match_keywords_enhanced("删除这些文件");
        assert!(result.is_some());
        assert_eq!(result.unwrap().category, TaskCategory::FileCleanup);
    }

    #[test]
    fn test_enhanced_keywords_code_execution() {
        use crate::config::KeywordPolicy;
        let policy = KeywordPolicy::with_builtin_rules();
        let classifier = IntentClassifier::with_keyword_policy(&policy);

        // Code execution should work
        let result = classifier.match_keywords_enhanced("运行这个脚本");
        assert!(result.is_some());
        assert_eq!(result.unwrap().category, TaskCategory::CodeExecution);
    }

    #[test]
    fn test_enhanced_keywords_disabled_policy() {
        use crate::config::KeywordPolicy;
        let mut policy = KeywordPolicy::with_builtin_rules();
        policy.enabled = false;
        let classifier = IntentClassifier::with_keyword_policy(&policy);

        // When disabled, enhanced matching should not work
        let result = classifier.match_keywords_enhanced("帮我整理文件");
        assert!(result.is_none());
    }

    #[test]
    fn test_intent_type_to_category() {
        let classifier = IntentClassifier::new();

        assert_eq!(
            classifier.intent_type_to_category("FileOrganize"),
            Some(TaskCategory::FileOrganize)
        );
        assert_eq!(
            classifier.intent_type_to_category("FileTransfer"),
            Some(TaskCategory::FileTransfer)
        );
        assert_eq!(
            classifier.intent_type_to_category("FileCleanup"),
            Some(TaskCategory::FileCleanup)
        );
        assert_eq!(
            classifier.intent_type_to_category("CodeExecution"),
            Some(TaskCategory::CodeExecution)
        );
        assert_eq!(
            classifier.intent_type_to_category("DocumentGenerate"),
            Some(TaskCategory::DocumentGenerate)
        );
        assert_eq!(classifier.intent_type_to_category("Unknown"), None);
    }
}
