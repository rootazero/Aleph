//! Intent classifier for Agent execution mode.
//!
//! Provides 3-level classification: regex → keywords → LLM
//!
//! Module structure:
//! - `types`: Core types (ExecutionIntent, ExecutableTask)
//! - `keywords`: Keyword sets and exclusion patterns
//! - `l1_regex`: L1 regex pattern matching (<5ms)
//! - `l2_keywords`: L2 keyword matching (<20ms)
//! - `l3_ai`: L3 AI-based classification (1-3s)
//! - `core`: IntentClassifier implementation

mod core;
mod keywords;
mod l1_regex;
mod l2_keywords;
mod l3_ai;
mod types;

// Re-exports for backward compatibility
pub use core::IntentClassifier;
pub use l2_keywords::intent_type_to_category;
pub use types::{ExecutableTask, ExecutionIntent};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::types::TaskCategory;

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
        let result = classifier.classify("能不能帮忙整理一下下载里的东西").await;
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
        assert!(
            result.is_none(),
            "Analysis requests should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_analyze_image_english() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("analyze this picture");
        assert!(
            result.is_none(),
            "Analysis requests should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_describe_photo() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("描述一下这张照片");
        assert!(
            result.is_none(),
            "Description requests should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_explain_file() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("解释这个文件的内容");
        assert!(
            result.is_none(),
            "Explanation requests should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_what_is_image() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("这张图片是什么");
        assert!(
            result.is_none(),
            "Question about content should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_look_at_photo() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("看看这张照片");
        assert!(
            result.is_none(),
            "Look at requests should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_summarize_document_chinese() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("总结这个文档");
        assert!(
            result.is_none(),
            "Summarization requests should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_summarize_webpage() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("帮我总结一下这个网页");
        assert!(
            result.is_none(),
            "Webpage summarization should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_abstract_file() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("给这个文件写个摘要");
        assert!(
            result.is_none(),
            "Abstract requests should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_summarize_english() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("summarize this document");
        assert!(
            result.is_none(),
            "English summarization should not trigger agent mode"
        );
    }

    #[test]
    fn test_exclusion_outline_file() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("概括一下这个文件的内容");
        assert!(
            result.is_none(),
            "Outline requests should not trigger agent mode"
        );
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
        assert!(
            result.is_some(),
            "Clear file organize requests should still work"
        );
        assert_eq!(result.unwrap().category, TaskCategory::FileOrganize);
    }

    #[test]
    fn test_real_file_cleanup_still_works() {
        let classifier = IntentClassifier::new();
        let result = classifier.match_keywords("清理一下缓存文件");
        assert!(
            result.is_some(),
            "Clear file cleanup requests should still work"
        );
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
        assert_eq!(
            intent_type_to_category("FileOrganize"),
            Some(TaskCategory::FileOrganize)
        );
        assert_eq!(
            intent_type_to_category("FileTransfer"),
            Some(TaskCategory::FileTransfer)
        );
        assert_eq!(
            intent_type_to_category("FileCleanup"),
            Some(TaskCategory::FileCleanup)
        );
        assert_eq!(
            intent_type_to_category("CodeExecution"),
            Some(TaskCategory::CodeExecution)
        );
        assert_eq!(
            intent_type_to_category("DocumentGenerate"),
            Some(TaskCategory::DocumentGenerate)
        );
        assert_eq!(intent_type_to_category("Unknown"), None);
    }

    // Tests for L3 AI detector integration

    #[test]
    fn test_with_ai_detector_builder() {
        let classifier = IntentClassifier::new();
        // Can't access private field, just ensure the API compiles
        let _ = classifier;
    }

    #[test]
    fn test_convert_ai_result() {
        use crate::intent::AiIntentResult;
        use std::collections::HashMap;

        let result = AiIntentResult {
            intent: "file_organize".to_string(),
            confidence: 0.9,
            params: HashMap::new(),
            missing: vec![],
        };

        let task = super::l3_ai::convert_ai_result(&result, "organize files");
        assert!(task.is_some());
        assert_eq!(task.unwrap().category, TaskCategory::FileOrganize);
    }

    #[test]
    fn test_convert_ai_result_with_path() {
        use crate::intent::AiIntentResult;
        use std::collections::HashMap;

        let mut params = HashMap::new();
        params.insert("path".to_string(), "/Downloads".to_string());

        let result = AiIntentResult {
            intent: "file_cleanup".to_string(),
            confidence: 0.85,
            params,
            missing: vec![],
        };

        let task = super::l3_ai::convert_ai_result(&result, "delete temp files");
        assert!(task.is_some());
        let task = task.unwrap();
        assert_eq!(task.category, TaskCategory::FileCleanup);
        assert_eq!(task.target, Some("/Downloads".to_string()));
        assert!((task.confidence - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_convert_ai_result_unknown_intent() {
        use crate::intent::AiIntentResult;
        use std::collections::HashMap;

        let result = AiIntentResult {
            intent: "unknown".to_string(),
            confidence: 0.9,
            params: HashMap::new(),
            missing: vec![],
        };

        let task = super::l3_ai::convert_ai_result(&result, "test");
        assert!(task.is_none());
    }

    #[test]
    fn test_convert_ai_result_all_categories() {
        use crate::intent::AiIntentResult;
        use std::collections::HashMap;

        let test_cases = vec![
            ("file_organize", TaskCategory::FileOrganize),
            ("file_cleanup", TaskCategory::FileCleanup),
            ("code_execution", TaskCategory::CodeExecution),
            ("file_transfer", TaskCategory::FileTransfer),
            ("document_generate", TaskCategory::DocumentGenerate),
        ];

        for (intent_str, expected_category) in test_cases {
            let result = AiIntentResult {
                intent: intent_str.to_string(),
                confidence: 0.9,
                params: HashMap::new(),
                missing: vec![],
            };

            let task = super::l3_ai::convert_ai_result(&result, "test");
            assert!(task.is_some(), "Failed for intent: {}", intent_str);
            assert_eq!(
                task.unwrap().category,
                expected_category,
                "Category mismatch for intent: {}",
                intent_str
            );
        }
    }
}
