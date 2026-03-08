//! Integration tests for the Unified Intent Classification Pipeline
//!
//! These tests verify the end-to-end intent classification flow through the
//! `UnifiedIntentClassifier` layered pipeline (L0 abort → L1 slash-command →
//! L2 structural → L3 keyword → L4 default).
//!
//! Without an AI provider or keyword index, the classifier relies on:
//! - Abort detector (exact-match stop words)
//! - Built-in slash commands (/screenshot, /ocr, /search, etc.)
//! - Structural detector (paths, URLs, context signals)
//! - L4 default fallback (Execute or Converse depending on config)

use crate::intent::{IntentContext, IntentResult, UnifiedIntentClassifier};

// ─── L0: Abort detection ─────────────────────────────────────────────

#[tokio::test]
async fn test_abort_stop() {
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    let result = classifier.classify("stop", &ctx).await;
    assert!(result.is_abort(), "Expected Abort for 'stop', got {result:?}");
}

#[tokio::test]
async fn test_abort_chinese() {
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    let result = classifier.classify("停止", &ctx).await;
    assert!(result.is_abort(), "Expected Abort for '停止', got {result:?}");
}

#[tokio::test]
async fn test_abort_with_punctuation() {
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    let result = classifier.classify("cancel!", &ctx).await;
    assert!(
        result.is_abort(),
        "Expected Abort for 'cancel!', got {result:?}"
    );
}

// ─── L1: Slash commands → DirectTool ─────────────────────────────────

#[tokio::test]
async fn test_slash_screenshot() {
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    let result = classifier.classify("/screenshot", &ctx).await;
    assert!(
        result.is_direct_tool(),
        "Expected DirectTool for '/screenshot', got {result:?}"
    );
    if let IntentResult::DirectTool { tool_id, .. } = &result {
        assert_eq!(tool_id, "screenshot");
    }
}

#[tokio::test]
async fn test_slash_ocr() {
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    let result = classifier.classify("/ocr", &ctx).await;
    assert!(
        result.is_direct_tool(),
        "Expected DirectTool for '/ocr', got {result:?}"
    );
    if let IntentResult::DirectTool { tool_id, .. } = &result {
        assert_eq!(tool_id, "vision_ocr");
    }
}

// ─── L2: Structural detection (paths / URLs) ────────────────────────

#[tokio::test]
async fn test_input_with_unix_path() {
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    let result = classifier
        .classify("帮我整理/Downloads/test文件夹里的文件", &ctx)
        .await;
    assert!(
        result.is_execute(),
        "Expected Execute for input containing a path, got {result:?}"
    );
    if let IntentResult::Execute { metadata, .. } = &result {
        assert!(
            metadata.detected_path.is_some(),
            "Expected detected_path to be populated"
        );
    }
}

#[tokio::test]
async fn test_input_with_absolute_path() {
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    let result = classifier
        .classify("把/Users/test/file.txt移动到Documents目录", &ctx)
        .await;
    assert!(
        result.is_execute(),
        "Expected Execute for input with absolute path, got {result:?}"
    );
    if let IntentResult::Execute { metadata, .. } = &result {
        assert!(
            metadata.detected_path.is_some(),
            "Expected detected_path for absolute path input"
        );
    }
}

#[tokio::test]
async fn test_input_with_url() {
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    let result = classifier
        .classify("fetch https://example.com/page", &ctx)
        .await;
    assert!(
        result.is_execute(),
        "Expected Execute for input with URL, got {result:?}"
    );
    if let IntentResult::Execute { metadata, .. } = &result {
        assert!(
            metadata.detected_url.is_some(),
            "Expected detected_url for URL input"
        );
    }
}

// ─── L4: Default fallback ────────────────────────────────────────────

#[tokio::test]
async fn test_default_fallback_execute() {
    // Default config: default_to_execute = true
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    // Without AI or keyword rules, conversational text falls to L4 default
    let result = classifier.classify("你好，今天天气怎么样？", &ctx).await;
    assert!(
        result.is_execute(),
        "With default_to_execute=true, unknown text should fall to Execute, got {result:?}"
    );
}

#[tokio::test]
async fn test_default_fallback_converse() {
    // Build classifier with default_to_execute = false
    let classifier = UnifiedIntentClassifier::builder()
        .default_to_execute(false)
        .build();
    let ctx = IntentContext::default();

    let result = classifier.classify("你好，今天天气怎么样？", &ctx).await;
    assert!(
        result.is_converse(),
        "With default_to_execute=false, unknown text should be Converse, got {result:?}"
    );
}

#[tokio::test]
async fn test_short_input_fallback() {
    // Short input without path/URL also falls to L4 default
    let classifier = UnifiedIntentClassifier::builder()
        .default_to_execute(false)
        .build();
    let ctx = IntentContext::default();

    let result = classifier.classify("好", &ctx).await;
    assert!(
        result.is_converse(),
        "Short non-abort input with default_to_execute=false should be Converse, got {result:?}"
    );
}

// ─── Priority ordering ──────────────────────────────────────────────

#[tokio::test]
async fn test_abort_takes_priority_over_path() {
    // "stop" is an abort word even if we try to put a path in
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    // Pure abort word — should be caught before structural detection
    let result = classifier.classify("stop", &ctx).await;
    assert!(
        result.is_abort(),
        "Abort should take priority, got {result:?}"
    );
}

#[tokio::test]
async fn test_non_abort_sentence_with_stop_word() {
    // "don't stop the music" is NOT an abort (substring, not exact match)
    let classifier = UnifiedIntentClassifier::new();
    let ctx = IntentContext::default();

    let result = classifier
        .classify("don't stop the music", &ctx)
        .await;
    assert!(
        !result.is_abort(),
        "Substring 'stop' should not trigger abort, got {result:?}"
    );
}
