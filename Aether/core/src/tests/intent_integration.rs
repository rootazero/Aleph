//! Integration tests for Agent Execution Mode
//!
//! These tests verify the end-to-end intent classification and parameter resolution flow.

use crate::intent::{
    DefaultsResolver, ExecutableTask, ExecutionIntent, IntentClassifier, OrganizeMethod,
    ParameterSource, TaskCategory,
};

/// Test full intent classification flow from input to task + parameters
#[tokio::test]
async fn test_full_intent_classification_flow() {
    let classifier = IntentClassifier::new();
    let resolver = DefaultsResolver::new();

    // Test file organize scenario with Downloads path
    // Since "Downloads" matches the "downloads" preset, it should return ByCategory
    let intent = classifier
        .classify("帮我整理/Downloads/test文件夹里的文件")
        .await;

    if let ExecutionIntent::Executable(task) = intent {
        assert_eq!(task.category, TaskCategory::FileOrganize);
        assert!(task.target.is_some());
        assert!(task.confidence >= 0.85, "Confidence should be >= 0.85");

        let params = resolver.resolve(&task).await;
        // "Downloads" in the path matches "downloads" preset, so ByCategory is expected
        assert_eq!(params.organize_method, OrganizeMethod::ByCategory);
        assert_eq!(params.source, ParameterSource::Preset);
    } else {
        panic!("Expected Executable intent, got {:?}", intent);
    }
}

/// Test conversational input doesn't trigger agent mode
#[tokio::test]
async fn test_conversational_input() {
    let classifier = IntentClassifier::new();

    let intent = classifier.classify("你好，今天天气怎么样？").await;
    assert!(
        matches!(intent, ExecutionIntent::Conversational),
        "Expected Conversational intent"
    );
}

/// Test file transfer classification
#[tokio::test]
async fn test_file_transfer_intent() {
    let classifier = IntentClassifier::new();

    let intent = classifier
        .classify("把/Users/test/file.txt移动到Documents目录")
        .await;

    if let ExecutionIntent::Executable(task) = intent {
        assert_eq!(task.category, TaskCategory::FileTransfer);
    } else {
        panic!("Expected Executable intent for file transfer");
    }
}

/// Test file cleanup classification
#[tokio::test]
async fn test_file_cleanup_intent() {
    let classifier = IntentClassifier::new();

    let intent = classifier.classify("删除/tmp目录下的临时文件").await;

    if let ExecutionIntent::Executable(task) = intent {
        assert_eq!(task.category, TaskCategory::FileCleanup);
    } else {
        panic!("Expected Executable intent for file cleanup");
    }
}

/// Test photo organization preset
#[tokio::test]
async fn test_photo_organization_preset() {
    let classifier = IntentClassifier::new();
    let resolver = DefaultsResolver::new();

    let intent = classifier.classify("帮我整理照片").await;

    if let ExecutionIntent::Executable(task) = intent {
        let params = resolver.resolve(&task).await;
        // Photos should use ByDate organization
        assert_eq!(params.organize_method, OrganizeMethod::ByDate);
    } else {
        panic!("Expected Executable intent for photo organization");
    }
}

/// Test downloads organization preset
#[tokio::test]
async fn test_downloads_organization_preset() {
    let classifier = IntentClassifier::new();
    let resolver = DefaultsResolver::new();

    let intent = classifier.classify("整理我的下载文件夹").await;

    if let ExecutionIntent::Executable(task) = intent {
        let params = resolver.resolve(&task).await;
        // Downloads should use ByCategory organization
        assert_eq!(params.organize_method, OrganizeMethod::ByCategory);
    } else {
        panic!("Expected Executable intent for downloads organization");
    }
}

/// Test English input classification
#[tokio::test]
async fn test_english_input_classification() {
    let classifier = IntentClassifier::new();

    let intent = classifier
        .classify("organize files in /Users/test/downloads")
        .await;

    if let ExecutionIntent::Executable(task) = intent {
        assert_eq!(task.category, TaskCategory::FileOrganize);
    } else {
        panic!("Expected Executable intent for English input");
    }
}

/// Test short input doesn't trigger agent mode
#[tokio::test]
async fn test_short_input() {
    let classifier = IntentClassifier::new();

    let intent = classifier.classify("好").await;
    assert!(
        matches!(intent, ExecutionIntent::Conversational),
        "Short input should be conversational"
    );
}

/// Test FFI conversion
#[test]
fn test_executable_task_ffi_conversion() {
    use crate::intent::ExecutableTaskFFI;

    let task = ExecutableTask {
        category: TaskCategory::FileOrganize,
        action: "整理文件".to_string(),
        target: Some("/Downloads".to_string()),
        confidence: 0.95,
    };

    let ffi: ExecutableTaskFFI = (&task).into();
    assert_eq!(ffi.action, "整理文件");
    assert_eq!(ffi.target, Some("/Downloads".to_string()));
    assert_eq!(ffi.confidence, 0.95);
}
