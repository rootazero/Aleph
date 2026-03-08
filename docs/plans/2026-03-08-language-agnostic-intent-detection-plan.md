# Language-Agnostic Intent Detection Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace language-specific intent detection with a unified, language-agnostic pipeline: AbortDetector → SlashCommands → StructuralDetector → UserKeywords → AiBinaryClassifier → Default.

**Architecture:** Single `IntentClassifier` with first-match-wins pipeline. Structural detection (paths, URLs, commands) stays deterministic. Semantic classification (execute vs converse) delegated to LLM. All hardcoded Chinese/English keywords and regex patterns removed.

**Tech Stack:** Rust, Tokio (async), regex crate, serde_json, existing `AiProvider` trait.

**Design Doc:** `docs/plans/2026-03-08-language-agnostic-intent-detection-design.md`

---

## Task Overview

| Task | Description | Depends On |
|------|-------------|------------|
| 1 | New core types (`IntentResult`, `ExecuteMetadata`, `DetectionLayer`) | — |
| 2 | `AbortDetector` | 1 |
| 3 | `StructuralDetector` (paths, URLs, context signals) | 1 |
| 4 | `AiBinaryClassifier` | 1 |
| 5 | Adapt `IntentCache` + `ConfidenceCalibrator` to new types | 1 |
| 6 | Rewrite `IntentClassifier` as unified pipeline | 2, 3, 4, 5 |
| 7 | Migrate downstream callers | 6 |
| 8 | Delete old code + update `mod.rs` re-exports | 7 |
| 9 | Integration tests + final verification | 8 |

---

## Task 1: New Core Types

**Files:**
- Create: `core/src/intent/types/intent_result.rs`
- Modify: `core/src/intent/types/mod.rs`

### Step 1: Write the test

```rust
// In core/src/intent/types/intent_result.rs at the bottom

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_result_direct_tool() {
        let result = IntentResult::DirectTool {
            tool_id: "screenshot".to_string(),
            args: Some("--full".to_string()),
            source: DirectToolSource::SlashCommand,
        };
        assert!(result.is_direct_tool());
        assert!(!result.is_execute());
        assert!(!result.is_converse());
        assert!(!result.is_abort());
    }

    #[test]
    fn intent_result_execute_with_metadata() {
        let meta = ExecuteMetadata {
            detected_path: Some("/tmp/test.txt".to_string()),
            detected_url: None,
            context_hint: None,
            keyword_tag: None,
            layer: DetectionLayer::L1,
        };
        let result = IntentResult::Execute {
            confidence: 0.95,
            metadata: meta,
        };
        assert!(result.is_execute());
        assert_eq!(result.confidence(), 0.95);
    }

    #[test]
    fn intent_result_converse() {
        let result = IntentResult::Converse { confidence: 0.8 };
        assert!(result.is_converse());
        assert_eq!(result.confidence(), 0.8);
    }

    #[test]
    fn intent_result_abort() {
        let result = IntentResult::Abort;
        assert!(result.is_abort());
        assert_eq!(result.confidence(), 1.0);
    }

    #[test]
    fn execute_metadata_default() {
        let meta = ExecuteMetadata::default_with_layer(DetectionLayer::L4Default);
        assert!(meta.detected_path.is_none());
        assert!(meta.detected_url.is_none());
        assert_eq!(meta.layer, DetectionLayer::L4Default);
    }
}
```

### Step 2: Run test to verify it fails

Run: `cargo test -p alephcore --lib intent::types::intent_result::tests -- --no-run 2>&1 | head -5`
Expected: Compilation error — module and types don't exist yet.

### Step 3: Write the implementation

```rust
// core/src/intent/types/intent_result.rs

use serde::{Deserialize, Serialize};

/// Source of a direct tool invocation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectToolSource {
    /// User typed /command
    SlashCommand,
    /// Matched a registered skill
    Skill,
    /// Routed to an MCP server tool
    Mcp,
    /// Custom command from config
    Custom,
}

/// Which pipeline layer produced this result
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionLayer {
    /// L0: Slash command detection
    L0,
    /// L1: Structural detection (paths, URLs, context signals)
    L1,
    /// L2: User-defined keyword rules
    L2,
    /// L3: AI binary classification
    L3,
    /// L4: Default fallback
    L4Default,
}

/// Metadata attached to an Execute intent
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExecuteMetadata {
    /// File system path detected in input
    pub detected_path: Option<String>,
    /// URL detected in input
    pub detected_url: Option<String>,
    /// Hint from context signals (selected_file, clipboard)
    pub context_hint: Option<String>,
    /// Tag from user-defined L2 keyword rules
    pub keyword_tag: Option<String>,
    /// Which pipeline layer produced this result
    pub layer: DetectionLayer,
}

impl ExecuteMetadata {
    pub fn default_with_layer(layer: DetectionLayer) -> Self {
        Self {
            layer,
            ..Default::default()
        }
    }
}

impl Default for DetectionLayer {
    fn default() -> Self {
        Self::L4Default
    }
}

/// Unified output of the intent detection pipeline.
///
/// Replaces `ExecutionIntent`, `ExecutionMode`, `DecisionResult`,
/// `RouteResult`, and `AggregatedIntent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IntentResult {
    /// Slash command or direct tool invocation (L0)
    DirectTool {
        tool_id: String,
        args: Option<String>,
        source: DirectToolSource,
    },
    /// Needs Agent Loop execution — tools, multi-step tasks (L1-L4)
    Execute {
        confidence: f32,
        metadata: ExecuteMetadata,
    },
    /// Pure conversation, no tool needed
    Converse {
        confidence: f32,
    },
    /// User wants to abort the current task
    Abort,
}

impl IntentResult {
    pub fn is_direct_tool(&self) -> bool {
        matches!(self, Self::DirectTool { .. })
    }

    pub fn is_execute(&self) -> bool {
        matches!(self, Self::Execute { .. })
    }

    pub fn is_converse(&self) -> bool {
        matches!(self, Self::Converse { .. })
    }

    pub fn is_abort(&self) -> bool {
        matches!(self, Self::Abort)
    }

    /// Returns the confidence score (1.0 for DirectTool and Abort)
    pub fn confidence(&self) -> f32 {
        match self {
            Self::DirectTool { .. } => 1.0,
            Self::Execute { confidence, .. } => *confidence,
            Self::Converse { confidence, .. } => *confidence,
            Self::Abort => 1.0,
        }
    }

    /// Returns the detection layer if available
    pub fn layer(&self) -> Option<DetectionLayer> {
        match self {
            Self::DirectTool { .. } => Some(DetectionLayer::L0),
            Self::Execute { metadata, .. } => Some(metadata.layer),
            Self::Converse { .. } => None,
            Self::Abort => None,
        }
    }
}
```

### Step 4: Wire up the module

In `core/src/intent/types/mod.rs`, add:
```rust
mod intent_result;
pub use intent_result::*;
```

### Step 5: Run tests to verify they pass

Run: `cargo test -p alephcore --lib intent::types::intent_result::tests -v`
Expected: All 5 tests PASS.

### Step 6: Commit

```bash
git add core/src/intent/types/intent_result.rs core/src/intent/types/mod.rs
git commit -m "intent: add IntentResult, ExecuteMetadata, DetectionLayer types"
```

---

## Task 2: AbortDetector

**Files:**
- Create: `core/src/intent/detection/abort.rs`
- Modify: `core/src/intent/detection/mod.rs`

### Step 1: Write the test

```rust
// In core/src/intent/detection/abort.rs at the bottom

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abort_english_basic() {
        let d = AbortDetector::new();
        assert!(d.is_abort("stop"));
        assert!(d.is_abort("abort"));
        assert!(d.is_abort("halt"));
        assert!(d.is_abort("cancel"));
        assert!(d.is_abort("quit"));
    }

    #[test]
    fn abort_chinese() {
        let d = AbortDetector::new();
        assert!(d.is_abort("停止"));
        assert!(d.is_abort("取消"));
        assert!(d.is_abort("中止"));
        assert!(d.is_abort("停"));
    }

    #[test]
    fn abort_japanese() {
        let d = AbortDetector::new();
        assert!(d.is_abort("やめて"));
        assert!(d.is_abort("止めて"));
    }

    #[test]
    fn abort_korean() {
        let d = AbortDetector::new();
        assert!(d.is_abort("중지"));
        assert!(d.is_abort("멈춰"));
    }

    #[test]
    fn abort_other_languages() {
        let d = AbortDetector::new();
        assert!(d.is_abort("стоп"));       // Russian
        assert!(d.is_abort("stopp"));      // German
        assert!(d.is_abort("arrête"));     // French
        assert!(d.is_abort("pare"));       // Portuguese
        assert!(d.is_abort("توقف"));       // Arabic
        assert!(d.is_abort("रुको"));       // Hindi
    }

    #[test]
    fn abort_case_insensitive() {
        let d = AbortDetector::new();
        assert!(d.is_abort("STOP"));
        assert!(d.is_abort("Stop"));
        assert!(d.is_abort("ABORT"));
    }

    #[test]
    fn abort_with_trailing_punctuation() {
        let d = AbortDetector::new();
        assert!(d.is_abort("stop!"));
        assert!(d.is_abort("stop!!!"));
        assert!(d.is_abort("stop。"));
        assert!(d.is_abort("停止！"));
        assert!(d.is_abort("cancel..."));
    }

    #[test]
    fn abort_with_whitespace() {
        let d = AbortDetector::new();
        assert!(d.is_abort("  stop  "));
        assert!(d.is_abort("\tstop\n"));
    }

    #[test]
    fn abort_no_substring_match() {
        let d = AbortDetector::new();
        assert!(!d.is_abort("don't stop the music"));
        assert!(!d.is_abort("stop the world I want to get off"));
        assert!(!d.is_abort("bus stop"));
        assert!(!d.is_abort("nonstop flight"));
    }

    #[test]
    fn abort_empty_and_short() {
        let d = AbortDetector::new();
        assert!(!d.is_abort(""));
        assert!(!d.is_abort("   "));
        assert!(!d.is_abort("hi"));
    }

    #[test]
    fn abort_not_normal_text() {
        let d = AbortDetector::new();
        assert!(!d.is_abort("please help me"));
        assert!(!d.is_abort("what is the weather"));
        assert!(!d.is_abort("整理我的文件"));
    }
}
```

### Step 2: Run test to verify it fails

Run: `cargo test -p alephcore --lib intent::detection::abort::tests -- --no-run 2>&1 | head -5`
Expected: Compilation error.

### Step 3: Write the implementation

```rust
// core/src/intent/detection/abort.rs

use std::collections::HashSet;

/// Fast-path abort detection.
///
/// Checks whether the entire message (after normalization) is a stop/abort
/// trigger in any supported language. Uses exact matching only — no substring
/// detection, preventing false positives like "don't stop the music".
pub struct AbortDetector {
    triggers: HashSet<String>,
}

impl AbortDetector {
    pub fn new() -> Self {
        let triggers: HashSet<String> = [
            // English
            "stop", "abort", "halt", "cancel", "quit",
            // Chinese
            "停", "停止", "取消", "中止",
            // Japanese
            "やめて", "止めて", "中止",
            // Korean
            "중지", "멈춰",
            // Russian
            "стоп", "остановись",
            // German
            "stopp", "anhalten",
            // French
            "arrête", "arrete",
            // Spanish
            "para", "detente",
            // Portuguese
            "pare",
            // Arabic
            "توقف",
            // Hindi
            "रुको",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        Self { triggers }
    }

    /// Returns true if the entire message is an abort trigger.
    pub fn is_abort(&self, input: &str) -> bool {
        let normalized = Self::normalize(input);
        if normalized.is_empty() {
            return false;
        }
        self.triggers.contains(normalized.as_str())
    }

    /// Strip trailing punctuation, collapse whitespace, lowercase.
    fn normalize(input: &str) -> String {
        input
            .trim()
            .trim_end_matches(|c: char| {
                "。.!！?？…，,;；:：'\"''\"）)]}>"
                    .contains(c)
            })
            .to_lowercase()
    }
}

impl Default for AbortDetector {
    fn default() -> Self {
        Self::new()
    }
}
```

### Step 4: Wire up the module

In `core/src/intent/detection/mod.rs`, add:
```rust
mod abort;
pub use abort::AbortDetector;
```

### Step 5: Run tests to verify they pass

Run: `cargo test -p alephcore --lib intent::detection::abort::tests -v`
Expected: All 11 tests PASS.

### Step 6: Commit

```bash
git add core/src/intent/detection/abort.rs core/src/intent/detection/mod.rs
git commit -m "intent: add AbortDetector with multilingual stop words"
```

---

## Task 3: StructuralDetector

**Files:**
- Create: `core/src/intent/detection/structural.rs`
- Modify: `core/src/intent/detection/mod.rs`

**Reference code to migrate:**
- Path extraction: `detection/classifier/l1_regex.rs:48-78` (PATH_PATTERN + extract_path)
- Context signals: `decision/execution_decider.rs:646-670` (check_context_signals)
- URL detection: new, simple regex

### Step 1: Write the test

```rust
// In core/src/intent/detection/structural.rs at the bottom

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::types::{DetectionLayer, IntentResult};

    #[test]
    fn detect_unix_path() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext::default();
        let result = d.detect("please read /home/user/file.txt", &ctx);
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.is_execute());
        if let IntentResult::Execute { metadata, .. } = &result {
            assert_eq!(metadata.detected_path.as_deref(), Some("/home/user/file.txt"));
            assert_eq!(metadata.layer, DetectionLayer::L1);
        }
    }

    #[test]
    fn detect_home_path() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext::default();
        let result = d.detect("organize ~/Downloads", &ctx);
        assert!(result.is_some());
        if let Some(IntentResult::Execute { metadata, .. }) = &result {
            assert_eq!(metadata.detected_path.as_deref(), Some("~/Downloads"));
        }
    }

    #[test]
    fn detect_windows_path() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext::default();
        let result = d.detect("open C:\\Users\\test\\file.txt", &ctx);
        assert!(result.is_some());
        if let Some(IntentResult::Execute { metadata, .. }) = &result {
            assert!(metadata.detected_path.is_some());
        }
    }

    #[test]
    fn detect_url() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext::default();
        let result = d.detect("fetch https://example.com/page", &ctx);
        assert!(result.is_some());
        if let Some(IntentResult::Execute { metadata, .. }) = &result {
            assert_eq!(metadata.detected_url.as_deref(), Some("https://example.com/page"));
            assert!(metadata.detected_path.is_none());
        }
    }

    #[test]
    fn detect_context_selected_image() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext {
            selected_file: Some("photo.jpg".to_string()),
            ..Default::default()
        };
        let result = d.detect("do something with this", &ctx);
        assert!(result.is_some());
        if let Some(IntentResult::Execute { metadata, .. }) = &result {
            assert_eq!(metadata.context_hint.as_deref(), Some("image_file:photo.jpg"));
        }
    }

    #[test]
    fn detect_context_selected_file() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext {
            selected_file: Some("report.pdf".to_string()),
            ..Default::default()
        };
        let result = d.detect("summarize this", &ctx);
        assert!(result.is_some());
        if let Some(IntentResult::Execute { metadata, .. }) = &result {
            assert_eq!(metadata.context_hint.as_deref(), Some("file:report.pdf"));
        }
    }

    #[test]
    fn detect_context_clipboard_image() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext {
            clipboard_type: Some("image".to_string()),
            ..Default::default()
        };
        let result = d.detect("what is this", &ctx);
        assert!(result.is_some());
        if let Some(IntentResult::Execute { metadata, .. }) = &result {
            assert_eq!(metadata.context_hint.as_deref(), Some("clipboard:image"));
        }
    }

    #[test]
    fn no_structural_signal() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext::default();
        let result = d.detect("what is quantum computing", &ctx);
        assert!(result.is_none());
    }

    #[test]
    fn no_detect_bare_slash() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext::default();
        // Paths starting with / need at least one more segment
        let result = d.detect("just a / character", &ctx);
        assert!(result.is_none());
    }

    #[test]
    fn url_takes_priority_over_path() {
        let d = StructuralDetector::new();
        let ctx = StructuralContext::default();
        let result = d.detect("check https://example.com/path/to/page", &ctx);
        assert!(result.is_some());
        if let Some(IntentResult::Execute { metadata, .. }) = &result {
            // URL detected, path should not be extracted from the URL
            assert!(metadata.detected_url.is_some());
        }
    }
}
```

### Step 2: Run test to verify it fails

Run: `cargo test -p alephcore --lib intent::detection::structural::tests -- --no-run 2>&1 | head -5`
Expected: Compilation error.

### Step 3: Write the implementation

```rust
// core/src/intent/detection/structural.rs

use once_cell::sync::Lazy;
use regex::Regex;

use crate::intent::types::{DetectionLayer, ExecuteMetadata, IntentResult};

/// Context signals from the UI/environment
#[derive(Debug, Clone, Default)]
pub struct StructuralContext {
    pub selected_file: Option<String>,
    pub clipboard_type: Option<String>,
}

/// Detects language-agnostic structural patterns in user input:
/// file paths, URLs, and context signals from the environment.
pub struct StructuralDetector;

/// Unix/Windows path pattern (migrated from l1_regex.rs)
static PATH_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"['"]?([/~][A-Za-z0-9_./-]+|[A-Za-z]:\\[A-Za-z0-9_.\\/]+)['"]?"#).unwrap()
});

/// URL pattern — http(s) and ftp(s) schemes
static URL_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(https?://[^\s,;'\")\]}>]+|ftps?://[^\s,;'\")\]}>]+)").unwrap()
});

/// Image file extensions
const IMAGE_EXTENSIONS: &[&str] = &[
    ".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tiff", ".svg", ".heic",
];

impl StructuralDetector {
    pub fn new() -> Self {
        Self
    }

    /// Detect structural patterns in input + context.
    /// Returns None if no structural signal found (pass to next layer).
    pub fn detect(&self, input: &str, context: &StructuralContext) -> Option<IntentResult> {
        // 1. Check context signals first (highest confidence — user has something selected)
        if let Some(result) = self.check_context(context) {
            return Some(result);
        }

        // 2. Extract URL (check before path to avoid extracting paths from URLs)
        let url = self.extract_url(input);

        // 3. Extract file path (skip if URL was found at same position)
        let path = if url.is_none() {
            self.extract_path(input)
        } else {
            // Still try to extract a path that's separate from the URL
            self.extract_path_excluding_url(input, url.as_deref().unwrap_or(""))
        };

        // If we found either a path or URL, it's structural
        if url.is_some() || path.is_some() {
            return Some(IntentResult::Execute {
                confidence: 0.9,
                metadata: ExecuteMetadata {
                    detected_path: path,
                    detected_url: url,
                    context_hint: None,
                    keyword_tag: None,
                    layer: DetectionLayer::L1,
                },
            });
        }

        None
    }

    fn check_context(&self, context: &StructuralContext) -> Option<IntentResult> {
        if let Some(ref file_path) = context.selected_file {
            let lower = file_path.to_lowercase();
            let is_image = IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext));
            let hint = if is_image {
                format!("image_file:{}", file_path)
            } else {
                format!("file:{}", file_path)
            };
            return Some(IntentResult::Execute {
                confidence: 0.85,
                metadata: ExecuteMetadata {
                    detected_path: None,
                    detected_url: None,
                    context_hint: Some(hint),
                    keyword_tag: None,
                    layer: DetectionLayer::L1,
                },
            });
        }

        if let Some(ref clip_type) = context.clipboard_type {
            if clip_type == "image" {
                return Some(IntentResult::Execute {
                    confidence: 0.8,
                    metadata: ExecuteMetadata {
                        detected_path: None,
                        detected_url: None,
                        context_hint: Some("clipboard:image".to_string()),
                        keyword_tag: None,
                        layer: DetectionLayer::L1,
                    },
                });
            }
        }

        None
    }

    fn extract_url(&self, input: &str) -> Option<String> {
        URL_PATTERN
            .captures(input)
            .map(|c| c[1].to_string())
    }

    fn extract_path(&self, input: &str) -> Option<String> {
        PATH_PATTERN
            .captures(input)
            .map(|c| c[1].to_string())
    }

    fn extract_path_excluding_url(&self, input: &str, url: &str) -> Option<String> {
        // Remove the URL from input, then try to find a path
        let without_url = input.replace(url, "");
        self.extract_path(&without_url)
    }
}

impl Default for StructuralDetector {
    fn default() -> Self {
        Self::new()
    }
}
```

### Step 4: Wire up the module

In `core/src/intent/detection/mod.rs`, add:
```rust
mod structural;
pub use structural::{StructuralContext, StructuralDetector};
```

### Step 5: Run tests to verify they pass

Run: `cargo test -p alephcore --lib intent::detection::structural::tests -v`
Expected: All 10 tests PASS.

### Step 6: Commit

```bash
git add core/src/intent/detection/structural.rs core/src/intent/detection/mod.rs
git commit -m "intent: add StructuralDetector for paths, URLs, and context signals"
```

---

## Task 4: AiBinaryClassifier

**Files:**
- Create: `core/src/intent/detection/ai_binary.rs`
- Modify: `core/src/intent/detection/mod.rs`

**Reference:** `detection/ai_detector.rs` for provider interface pattern and JSON extraction.

### Step 1: Write the test

```rust
// In core/src/intent/detection/ai_binary.rs at the bottom

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::future::Future;

    struct MockProvider {
        response: String,
    }

    impl MockProvider {
        fn with_response(response: &str) -> Self {
            Self { response: response.to_string() }
        }
    }

    impl AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
            let resp = self.response.clone();
            Box::pin(async move { Ok(resp) })
        }
        fn name(&self) -> &str { "mock" }
        fn color(&self) -> &str { "white" }
    }

    struct HangingProvider;

    impl AiProvider for HangingProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok("never".to_string())
            })
        }
        fn name(&self) -> &str { "hanging" }
        fn color(&self) -> &str { "white" }
    }

    fn make_classifier(provider: impl AiProvider + 'static) -> AiBinaryClassifier {
        AiBinaryClassifier::new(
            Arc::new(provider),
            AiBinaryConfig {
                min_input_length: 5,
                timeout: Duration::from_millis(100),
                confidence_threshold: 0.6,
            },
        )
    }

    #[tokio::test]
    async fn classify_execute() {
        let provider = MockProvider::with_response(r#"{"intent":"execute","confidence":0.92}"#);
        let c = make_classifier(provider);
        let result = c.classify("organize my downloads folder").await;
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.is_execute());
        assert!(result.confidence() > 0.9);
    }

    #[tokio::test]
    async fn classify_converse() {
        let provider = MockProvider::with_response(r#"{"intent":"converse","confidence":0.88}"#);
        let c = make_classifier(provider);
        let result = c.classify("what is quantum computing").await;
        assert!(result.is_some());
        assert!(result.unwrap().is_converse());
    }

    #[tokio::test]
    async fn classify_low_confidence_returns_none() {
        let provider = MockProvider::with_response(r#"{"intent":"execute","confidence":0.3}"#);
        let c = make_classifier(provider);
        let result = c.classify("maybe do something").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn classify_too_short_returns_none() {
        let provider = MockProvider::with_response(r#"{"intent":"execute","confidence":0.9}"#);
        let c = make_classifier(provider);
        let result = c.classify("hi").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn classify_timeout_returns_none() {
        let c = make_classifier(HangingProvider);
        let result = c.classify("a long enough input to pass length check").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn classify_json_in_markdown_block() {
        let provider = MockProvider::with_response(
            "```json\n{\"intent\":\"execute\",\"confidence\":0.85}\n```"
        );
        let c = make_classifier(provider);
        let result = c.classify("run the test suite").await;
        assert!(result.is_some());
        assert!(result.unwrap().is_execute());
    }

    #[tokio::test]
    async fn classify_malformed_json_returns_none() {
        let provider = MockProvider::with_response("I'm not sure what you mean");
        let c = make_classifier(provider);
        let result = c.classify("some input text here").await;
        assert!(result.is_none());
    }
}
```

### Step 2: Run test to verify it fails

Run: `cargo test -p alephcore --lib intent::detection::ai_binary::tests -- --no-run 2>&1 | head -5`
Expected: Compilation error.

### Step 3: Write the implementation

```rust
// core/src/intent/detection/ai_binary.rs

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use crate::intent::types::{DetectionLayer, ExecuteMetadata, IntentResult};
use crate::providers::AiProvider;

/// Configuration for the AI binary classifier
#[derive(Debug, Clone)]
pub struct AiBinaryConfig {
    /// Minimum input length in chars to invoke AI (default: 8)
    pub min_input_length: usize,
    /// Maximum time to wait for AI response (default: 3s)
    pub timeout: Duration,
    /// Minimum confidence to accept AI result (default: 0.6)
    pub confidence_threshold: f32,
}

impl Default for AiBinaryConfig {
    fn default() -> Self {
        Self {
            min_input_length: 8,
            timeout: Duration::from_secs(3),
            confidence_threshold: 0.6,
        }
    }
}

/// Binary intent classifier using LLM.
///
/// Asks the LLM a single question: "Does this input require tool execution
/// or is it pure conversation?" This is language-agnostic — the LLM handles
/// any language naturally.
pub struct AiBinaryClassifier {
    provider: Arc<dyn AiProvider>,
    config: AiBinaryConfig,
}

#[derive(Debug, Deserialize)]
struct AiResponse {
    intent: String,
    confidence: f32,
}

const SYSTEM_PROMPT: &str = r#"You are an intent classifier. Given a user message, determine if it requires tool execution or is pure conversation.

Respond with JSON only:
{"intent": "execute" | "converse", "confidence": 0.0-1.0}

Guidelines:
- "execute": user wants to perform an action (file operations, code execution, web search, image generation, system commands, downloads, etc.)
- "converse": user wants information, explanation, analysis, creative writing, translation, or general chat

Examples:
- "organize my downloads folder" → execute
- "what is quantum computing" → converse
- "run the test suite" → execute
- "explain this error message" → converse
- "search for flights to Tokyo" → execute
- "write me a poem about rain" → converse"#;

impl AiBinaryClassifier {
    pub fn new(provider: Arc<dyn AiProvider>, config: AiBinaryConfig) -> Self {
        Self { provider, config }
    }

    /// Classify input as execute or converse.
    /// Returns None if: input too short, AI unavailable/timeout, low confidence.
    pub async fn classify(&self, input: &str) -> Option<IntentResult> {
        if input.chars().count() < self.config.min_input_length {
            return None;
        }

        // Combine system + user prompt (compatible with all provider modes)
        let combined_prompt = format!(
            "[TASK: Intent Classification - Return JSON ONLY]\n\n{}\n\n---\nUser message: {}",
            SYSTEM_PROMPT, input
        );

        let response = tokio::time::timeout(
            self.config.timeout,
            self.provider.process(&combined_prompt, None),
        )
        .await
        .ok()?  // timeout → None
        .ok()?; // provider error → None

        self.parse_response(&response)
    }

    fn parse_response(&self, response: &str) -> Option<IntentResult> {
        let json_str = Self::extract_json(response)?;
        let parsed: AiResponse = serde_json::from_str(&json_str).ok()?;

        if parsed.confidence < self.config.confidence_threshold {
            return None;
        }

        match parsed.intent.as_str() {
            "execute" => Some(IntentResult::Execute {
                confidence: parsed.confidence,
                metadata: ExecuteMetadata::default_with_layer(DetectionLayer::L3),
            }),
            "converse" => Some(IntentResult::Converse {
                confidence: parsed.confidence,
            }),
            _ => None,
        }
    }

    /// Extract JSON from potentially wrapped response (plain, markdown block, or prefixed)
    fn extract_json(text: &str) -> Option<String> {
        let trimmed = text.trim();

        // Try plain JSON
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            return Some(trimmed.to_string());
        }

        // Try markdown code block
        if let Some(start) = trimmed.find("```") {
            let after_backticks = &trimmed[start + 3..];
            // Skip optional language tag (e.g., "json")
            let content_start = after_backticks.find('\n').map(|i| i + 1).unwrap_or(0);
            let content = &after_backticks[content_start..];
            if let Some(end) = content.find("```") {
                let json_str = content[..end].trim();
                if json_str.starts_with('{') && json_str.ends_with('}') {
                    return Some(json_str.to_string());
                }
            }
        }

        // Try finding last JSON object in text
        if let Some(end) = trimmed.rfind('}') {
            if let Some(start) = trimmed[..=end].rfind('{') {
                let candidate = &trimmed[start..=end];
                if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                    return Some(candidate.to_string());
                }
            }
        }

        None
    }
}
```

### Step 4: Wire up the module

In `core/src/intent/detection/mod.rs`, add:
```rust
mod ai_binary;
pub use ai_binary::{AiBinaryClassifier, AiBinaryConfig};
```

### Step 5: Run tests to verify they pass

Run: `cargo test -p alephcore --lib intent::detection::ai_binary::tests -v`
Expected: All 7 tests PASS.

**Important:** The `AiProvider` trait must be accessible from this module. If the import path `crate::providers::AiProvider` doesn't compile, check the actual location by running:
```bash
grep -rn "pub trait AiProvider" core/src/
```
Adjust the import accordingly.

### Step 6: Commit

```bash
git add core/src/intent/detection/ai_binary.rs core/src/intent/detection/mod.rs
git commit -m "intent: add AiBinaryClassifier for language-agnostic LLM classification"
```

---

## Task 5: Adapt IntentCache + ConfidenceCalibrator

**Files:**
- Modify: `core/src/intent/support/cache.rs`
- Modify: `core/src/intent/decision/calibrator.rs`

### Step 1: Read both files to understand current types

Run: Read `core/src/intent/support/cache.rs` and `core/src/intent/decision/calibrator.rs` fully.

### Step 2: Modify IntentCache

The cache currently stores `CachedIntent` with fields like `intent_type: String`, `tool_name: String`, `confidence: f32`. Modify it to store `IntentResult` directly.

Key changes:
- `CachedIntent.intent_type` + `tool_name` + `confidence` → `CachedIntent.result: IntentResult`
- `cache_intent(input, tool_name, intent_type, confidence)` → `cache_result(input, result: &IntentResult)`
- `get_cached(input) -> Option<CachedIntent>` → `get_cached(input) -> Option<IntentResult>` (with confidence decay applied)
- Keep the LRU eviction, confidence decay, and failure tracking logic

### Step 3: Modify ConfidenceCalibrator

The calibrator currently outputs `CalibratedSignal` with `RoutingLayer`, `intent_type`, `original_confidence`, `calibrated_confidence`. Simplify to work with `IntentResult`:

Key changes:
- `calibrate(signal: &IntentSignal, layer: RoutingLayer, ...) -> CalibratedSignal` → `calibrate(result: &IntentResult, recent_tools: &[String]) -> IntentResult`
- Keep the dampening factors and history boost logic
- Map `DetectionLayer` to dampening: L2 → 0.9, L3 → 0.95, others → 1.0
- Return a new `IntentResult` with adjusted confidence

### Step 4: Run tests

Run: `cargo test -p alephcore --lib intent::support::cache -v && cargo test -p alephcore --lib intent::decision::calibrator -v`
Expected: All existing tests pass (update test code to use new API).

### Step 5: Commit

```bash
git add core/src/intent/support/cache.rs core/src/intent/decision/calibrator.rs
git commit -m "intent: adapt IntentCache and ConfidenceCalibrator to IntentResult"
```

---

## Task 6: Rewrite IntentClassifier as Unified Pipeline

**Files:**
- Rewrite: `core/src/intent/detection/classifier/core.rs`
- Modify: `core/src/intent/detection/classifier/mod.rs`
- Modify: `core/src/intent/detection/mod.rs`

This is the central task. The new `IntentClassifier` replaces both the old `IntentClassifier` and `ExecutionIntentDecider`.

### Step 1: Write the test

```rust
// Tests for the new unified IntentClassifier pipeline

#[cfg(test)]
mod tests {
    use super::*;

    // Use AbortDetector, StructuralDetector, etc. from earlier tasks

    #[test]
    fn abort_wins_over_everything() {
        let classifier = IntentClassifier::new();
        let ctx = IntentContext::default();
        let result = tokio_test::block_on(classifier.classify("stop", &ctx));
        assert!(result.is_abort());
    }

    #[test]
    fn slash_command_detected() {
        let classifier = IntentClassifier::new();
        let ctx = IntentContext::default();
        let result = tokio_test::block_on(classifier.classify("/screenshot", &ctx));
        assert!(result.is_direct_tool());
        if let IntentResult::DirectTool { tool_id, .. } = &result {
            assert_eq!(tool_id, "screenshot");
        }
    }

    #[test]
    fn slash_command_with_args() {
        let classifier = IntentClassifier::new();
        let ctx = IntentContext::default();
        let result = tokio_test::block_on(classifier.classify("/search rust async", &ctx));
        assert!(result.is_direct_tool());
        if let IntentResult::DirectTool { tool_id, args, .. } = &result {
            assert_eq!(tool_id, "search");
            assert_eq!(args.as_deref(), Some("rust async"));
        }
    }

    #[test]
    fn structural_path_detected() {
        let classifier = IntentClassifier::new();
        let ctx = IntentContext::default();
        let result = tokio_test::block_on(classifier.classify("read /etc/hosts", &ctx));
        assert!(result.is_execute());
        if let IntentResult::Execute { metadata, .. } = &result {
            assert_eq!(metadata.detected_path.as_deref(), Some("/etc/hosts"));
            assert_eq!(metadata.layer, DetectionLayer::L1);
        }
    }

    #[test]
    fn context_signal_selected_file() {
        let classifier = IntentClassifier::new();
        let ctx = IntentContext {
            structural: StructuralContext {
                selected_file: Some("image.png".to_string()),
                ..Default::default()
            },
        };
        let result = tokio_test::block_on(classifier.classify("what is this", &ctx));
        assert!(result.is_execute());
    }

    #[test]
    fn default_fallback_when_no_ai() {
        // No AI provider configured → falls through to L4 default
        let classifier = IntentClassifier::builder().default_to_execute(true).build();
        let ctx = IntentContext::default();
        let result = tokio_test::block_on(classifier.classify("hello world how are you", &ctx));
        assert!(result.is_execute());
        if let IntentResult::Execute { metadata, .. } = &result {
            assert_eq!(metadata.layer, DetectionLayer::L4Default);
        }
    }

    #[test]
    fn default_converse_when_configured() {
        let classifier = IntentClassifier::builder().default_to_execute(false).build();
        let ctx = IntentContext::default();
        let result = tokio_test::block_on(classifier.classify("hello world how are you", &ctx));
        assert!(result.is_converse());
    }
}
```

### Step 2: Write the implementation

The new `IntentClassifier` struct:

```rust
use std::sync::Arc;
use crate::intent::detection::abort::AbortDetector;
use crate::intent::detection::structural::{StructuralContext, StructuralDetector};
use crate::intent::detection::ai_binary::{AiBinaryClassifier, AiBinaryConfig};
use crate::intent::detection::keyword::{KeywordIndex, KeywordMatch};
use crate::intent::support::IntentCache;
use crate::intent::decision::ConfidenceCalibrator;
use crate::intent::types::*;

/// Context for intent classification
#[derive(Debug, Clone, Default)]
pub struct IntentContext {
    pub structural: StructuralContext,
}

/// Configuration for the unified intent classifier
#[derive(Debug, Clone)]
pub struct IntentConfig {
    pub default_to_execute: bool,
}

impl Default for IntentConfig {
    fn default() -> Self {
        Self { default_to_execute: true }
    }
}

/// Unified intent classifier — single entry point replacing both
/// IntentClassifier (old) and ExecutionIntentDecider.
///
/// Pipeline: Abort → L0 (commands) → L1 (structural) → L2 (user keywords) → L3 (AI) → L4 (default)
pub struct IntentClassifier {
    abort_detector: AbortDetector,
    command_parser: Option<Arc<CommandParser>>,
    structural_detector: StructuralDetector,
    keyword_index: Option<KeywordIndex>,
    ai_classifier: Option<AiBinaryClassifier>,
    cache: Option<Arc<IntentCache>>,
    calibrator: Option<ConfidenceCalibrator>,
    config: IntentConfig,
}

impl IntentClassifier {
    pub fn new() -> Self {
        Self {
            abort_detector: AbortDetector::new(),
            command_parser: None,
            structural_detector: StructuralDetector::new(),
            keyword_index: None,
            ai_classifier: None,
            cache: None,
            calibrator: None,
            config: IntentConfig::default(),
        }
    }

    pub fn builder() -> IntentClassifierBuilder {
        IntentClassifierBuilder::default()
    }

    /// Single entry point for intent classification.
    pub async fn classify(&self, input: &str, context: &IntentContext) -> IntentResult {
        // Fast abort
        if self.abort_detector.is_abort(input) {
            return IntentResult::Abort;
        }

        // Check cache (skip L0 — always re-evaluate commands)
        if let Some(ref cache) = self.cache {
            if let Some(cached) = cache.get_cached(input) {
                return cached;
            }
        }

        // L0: Slash commands
        if input.trim().starts_with('/') {
            if let Some(result) = self.detect_command(input) {
                return result;
            }
        }

        // L1: Structural signals
        if let Some(result) = self.structural_detector.detect(input, &context.structural) {
            return result;
        }

        // L2: User keyword rules (optional)
        if let Some(ref index) = self.keyword_index {
            if let Some(km) = index.best_match(input, 0.5) {
                let result = IntentResult::Execute {
                    confidence: km.score.min(1.0),
                    metadata: ExecuteMetadata {
                        keyword_tag: Some(km.intent_type.clone()),
                        layer: DetectionLayer::L2,
                        ..Default::default()
                    },
                };
                let result = self.apply_calibration(result);
                self.update_cache(input, &result);
                return result;
            }
        }

        // L3: AI binary classification (optional)
        if let Some(ref ai) = self.ai_classifier {
            if let Some(result) = ai.classify(input).await {
                let result = self.apply_calibration(result);
                self.update_cache(input, &result);
                return result;
            }
        }

        // L4: Default
        if self.config.default_to_execute {
            IntentResult::Execute {
                confidence: 0.5,
                metadata: ExecuteMetadata::default_with_layer(DetectionLayer::L4Default),
            }
        } else {
            IntentResult::Converse { confidence: 0.5 }
        }
    }

    fn detect_command(&self, input: &str) -> Option<IntentResult> {
        let trimmed = input.trim();

        // Use CommandParser if available
        if let Some(ref parser) = self.command_parser {
            if let Some(parsed) = parser.parse(trimmed) {
                return Some(self.parsed_command_to_result(parsed));
            }
        }

        // Fallback: built-in slash commands
        let without_slash = &trimmed[1..];
        let (raw_cmd, args) = match without_slash.split_once(char::is_whitespace) {
            Some((name, rest)) => (name.to_lowercase(), Some(rest.trim().to_string())),
            None => (without_slash.to_lowercase(), None),
        };
        // Strip @botname suffix
        let cmd = match raw_cmd.split_once('@') {
            Some((name, _)) => name.to_string(),
            None => raw_cmd,
        };

        // Built-in commands (migrated from SLASH_COMMANDS)
        let tool_id = match cmd.as_str() {
            "screenshot" => "screenshot",
            "ocr" => "vision_ocr",
            "search" => "search",
            "webfetch" => "web_fetch",
            "gen" => "generate_image",
            _ => return None,
        };

        Some(IntentResult::DirectTool {
            tool_id: tool_id.to_string(),
            args: args.filter(|a| !a.is_empty()),
            source: DirectToolSource::SlashCommand,
        })
    }

    fn apply_calibration(&self, result: IntentResult) -> IntentResult {
        if let Some(ref calibrator) = self.calibrator {
            calibrator.calibrate(&result, &[])
        } else {
            result
        }
    }

    fn update_cache(&self, input: &str, result: &IntentResult) {
        if let Some(ref cache) = self.cache {
            cache.cache_result(input, result);
        }
    }

    // Access for downstream recording
    pub fn cache(&self) -> Option<&Arc<IntentCache>> {
        self.cache.as_ref()
    }

    pub fn calibrator(&self) -> Option<&ConfidenceCalibrator> {
        self.calibrator.as_ref()
    }

    pub fn set_command_parser(&mut self, parser: Arc<CommandParser>) {
        self.command_parser = Some(parser);
    }
}

/// Builder for IntentClassifier
#[derive(Default)]
pub struct IntentClassifierBuilder { /* fields mirror IntentClassifier */ }

impl IntentClassifierBuilder {
    pub fn default_to_execute(mut self, v: bool) -> Self { /* ... */ self }
    pub fn with_command_parser(mut self, p: Arc<CommandParser>) -> Self { /* ... */ self }
    pub fn with_keyword_index(mut self, idx: KeywordIndex) -> Self { /* ... */ self }
    pub fn with_ai_classifier(mut self, ai: AiBinaryClassifier) -> Self { /* ... */ self }
    pub fn with_cache(mut self, cache: Arc<IntentCache>) -> Self { /* ... */ self }
    pub fn with_calibrator(mut self, cal: ConfidenceCalibrator) -> Self { /* ... */ self }
    pub fn build(self) -> IntentClassifier { /* ... */ }
}
```

### Step 3: Handle `parsed_command_to_result`

Migrate the `parsed_command_to_mode` logic from `execution_decider.rs:535-624`. Map `ToolSourceType` variants to `DirectToolSource`:
- `ToolSourceType::Builtin` / `ToolSourceType::Custom` → `DirectToolSource::SlashCommand`
- `ToolSourceType::Skill` → `DirectToolSource::Skill`
- `ToolSourceType::Mcp` → `DirectToolSource::Mcp`

### Step 4: Run tests

Run: `cargo test -p alephcore --lib intent::detection::classifier::tests -v`
Expected: All tests PASS.

### Step 5: Commit

```bash
git add core/src/intent/detection/classifier/
git commit -m "intent: rewrite IntentClassifier as unified pipeline"
```

---

## Task 7: Migrate Downstream Callers

This task has multiple sub-steps, one per caller group. Each is an independent commit.

### 7a: Migrate `components/intent_analyzer.rs`

**File:** `core/src/components/intent_analyzer.rs`

This is the primary orchestrator. Currently holds both `IntentClassifier` (old) and `ExecutionIntentDecider`.

Changes:
- Replace `ExecutionIntentDecider` with new `IntentClassifier`
- Replace `ExecutionIntent` / `DecisionResult` matching with `IntentResult` matching
- `ContextSignals` → `StructuralContext` (field mapping: `selected_file`, `clipboard_type` carry over)
- Remove dual-classifier logic

### 7b: Migrate `gateway/inbound_router.rs`

**File:** `core/src/gateway/inbound_router.rs`

Changes:
- Replace `ExecutionIntentDecider` field with `IntentClassifier`
- Replace `serialize_execution_mode(mode: &ExecutionMode)` with `serialize_intent_result(result: &IntentResult)`
- Replace `parsed_command_to_mode()` calls — now handled inside `IntentClassifier`
- `ContextSignals` → `StructuralContext`

### 7c: Migrate `payload/assembler/intent.rs` + `mod.rs`

**Files:**
- `core/src/payload/assembler/intent.rs`
- `core/src/payload/assembler/mod.rs`

Changes:
- `build_prompt_with_intent(intent: Option<&ExecutionIntent>)` → `build_prompt_with_intent_result(result: &IntentResult)`
- `build_prompt_with_execution_mode(mode: &ExecutionMode>)` → merge into `build_prompt_with_intent_result`
- Pattern match `IntentResult::DirectTool` → direct tool prompt, `Execute` → executor prompt, `Converse` → conversational prompt
- Remove `AgentModePrompt` usage (deleted in Task 8)
- Remove `filter_tools_for_category` (stub that was a passthrough)

### 7d: Migrate tool filter chain

**Files:**
- `core/src/thinker/tool_filter.rs`
- `core/src/dispatcher/tool_filter.rs`
- `core/src/dispatcher/smart_filter.rs`

These use `TaskCategory` as HashMap keys for pre-filtering tools. With `TaskCategory` removed:

Option A (recommended): Remove category-based pre-filtering entirely. The LLM selects tools itself. This simplifies the code significantly.

Option B: Replace `TaskCategory` with `Option<String>` hint from `ExecuteMetadata.keyword_tag` or `context_hint`. Map hints to tool sets.

**Implement Option A** unless testing reveals performance issues with tool count.

Changes:
- `ToolFilterConfig.category_tools: HashMap<TaskCategory, Vec<String>>` → remove
- `pre_filter_by_category(tools, category)` → remove
- `detect_categories(observation) -> Vec<TaskCategory>` → remove or replace with `detect_hints(observation) -> Vec<String>`
- `SmartFilter.filter(tools, category, input)` → `filter(tools, input)` (drop category param)

### 7e: Migrate model router

**File:** `core/src/dispatcher/model_router/core/intent.rs`

Changes:
- `TaskIntent::from_task_category(category: TaskCategory)` → remove this conversion
- If `TaskIntent` still needs input, derive it from `IntentResult` metadata or use a default

### 7f: Migrate prompt builder

**Files:**
- `core/src/prompt/builder.rs`
- `core/src/prompt/executor.rs`

Changes:
- `executor_prompt(category: TaskCategory, tools, config)` → `executor_prompt(hint: Option<&str>, tools, config)` where hint comes from `ExecuteMetadata.context_hint` or `keyword_tag`
- `ExecutorPrompt::with_category(category: TaskCategory)` → `ExecutorPrompt::with_hint(hint: Option<&str>)`
- Category-specific prompt guidelines become hint-based or removed (LLM handles context)

### 7g: Migrate integration tests

**File:** `core/src/tests/intent_integration.rs`

Rewrite tests to use `IntentResult` instead of `ExecutionIntent`, `TaskCategory`, etc.

### Step: Commit after each sub-step

```bash
# After 7a:
git commit -m "intent: migrate intent_analyzer to IntentResult"
# After 7b:
git commit -m "intent: migrate inbound_router to IntentResult"
# After 7c:
git commit -m "intent: migrate payload assembler to IntentResult"
# After 7d:
git commit -m "intent: simplify tool filter chain, remove category-based filtering"
# After 7e:
git commit -m "intent: migrate model router to IntentResult"
# After 7f:
git commit -m "intent: migrate prompt builder to IntentResult"
# After 7g:
git commit -m "intent: rewrite integration tests for IntentResult"
```

**Verify after each sub-step:**
```bash
cargo check -p alephcore
```

---

## Task 8: Delete Old Code + Update Re-exports

**Files to delete:**
- `core/src/intent/detection/classifier/l1_regex.rs`
- `core/src/intent/detection/classifier/l2_keywords.rs`
- `core/src/intent/detection/classifier/keywords.rs`
- `core/src/intent/detection/classifier/types.rs` (old `ExecutionIntent`, `ExecutableTask`)
- `core/src/intent/detection/ai_detector.rs`
- `core/src/intent/decision/execution_decider.rs`
- `core/src/intent/decision/router.rs`
- `core/src/intent/decision/aggregator.rs`
- `core/src/intent/parameters/presets.rs`
- `core/src/intent/support/agent_prompt.rs`
- `core/src/intent/types/task_category.rs`

**Files to modify:**
- `core/src/intent/mod.rs` — update all re-exports
- `core/src/intent/detection/mod.rs` — remove old submodule refs
- `core/src/intent/detection/classifier/mod.rs` — remove old file refs
- `core/src/intent/decision/mod.rs` — remove old file refs
- `core/src/intent/parameters/mod.rs` — remove presets
- `core/src/intent/support/mod.rs` — remove agent_prompt
- `core/src/intent/types/mod.rs` — remove task_category

### Step 1: Delete files

```bash
rm core/src/intent/detection/classifier/l1_regex.rs
rm core/src/intent/detection/classifier/l2_keywords.rs
rm core/src/intent/detection/classifier/keywords.rs
rm core/src/intent/detection/classifier/types.rs
rm core/src/intent/detection/ai_detector.rs
rm core/src/intent/decision/execution_decider.rs
rm core/src/intent/decision/router.rs
rm core/src/intent/decision/aggregator.rs
rm core/src/intent/parameters/presets.rs
rm core/src/intent/support/agent_prompt.rs
rm core/src/intent/types/task_category.rs
```

### Step 2: Update mod.rs re-exports

The new `core/src/intent/mod.rs` should export:

```rust
// New types
pub use types::{
    IntentResult, ExecuteMetadata, DetectionLayer, DirectToolSource,
};

// Detection
pub use detection::{
    AbortDetector,
    StructuralContext, StructuralDetector,
    AiBinaryClassifier, AiBinaryConfig,
    IntentClassifier, IntentClassifierBuilder, IntentConfig, IntentContext,
    KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule,
};

// Decision (kept)
pub use decision::{
    ConfidenceCalibrator, CalibratorConfig,
};

// Parameters (kept, minus presets)
pub use parameters::{
    AppContext, ConversationContext, InputFeatures,
    MatchingContext, MatchingContextBuilder,
    ConflictResolution, OrganizeMethod, ParameterSource,
    TaskParameters, TimeContext, PendingParam,
};

// Support (kept, minus agent_prompt)
pub use support::{
    IntentCache, CacheConfig, CacheMetrics,
    RollbackCapable, RollbackConfig, RollbackEntry,
    RollbackManager, RollbackResult,
};
```

### Step 3: Verify

```bash
cargo check -p alephcore
cargo test -p alephcore --lib intent -v
```

### Step 4: Commit

```bash
git add -A core/src/intent/
git commit -m "intent: delete old language-specific code, update re-exports"
```

---

## Task 9: Integration Tests + Final Verification

**Files:**
- Create or update: `core/src/tests/intent_integration.rs`

### Step 1: Write language-agnostic integration tests

```rust
#[cfg(test)]
mod tests {
    use crate::intent::*;

    #[test]
    fn pipeline_abort_all_languages() {
        let c = IntentClassifier::new();
        let ctx = IntentContext::default();

        for word in &["stop", "停止", "やめて", "중지", "стоп", "arrête"] {
            let result = tokio_test::block_on(c.classify(word, &ctx));
            assert!(result.is_abort(), "Expected abort for '{}'", word);
        }
    }

    #[test]
    fn pipeline_slash_commands() {
        let c = IntentClassifier::new();
        let ctx = IntentContext::default();

        let result = tokio_test::block_on(c.classify("/screenshot", &ctx));
        assert!(result.is_direct_tool());

        let result = tokio_test::block_on(c.classify("/search quantum computing", &ctx));
        if let IntentResult::DirectTool { tool_id, args, .. } = &result {
            assert_eq!(tool_id, "search");
            assert_eq!(args.as_deref(), Some("quantum computing"));
        } else {
            panic!("Expected DirectTool");
        }
    }

    #[test]
    fn pipeline_structural_path() {
        let c = IntentClassifier::new();
        let ctx = IntentContext::default();

        let result = tokio_test::block_on(c.classify("read /etc/hosts", &ctx));
        assert!(result.is_execute());
        if let IntentResult::Execute { metadata, .. } = &result {
            assert_eq!(metadata.layer, DetectionLayer::L1);
            assert!(metadata.detected_path.is_some());
        }
    }

    #[test]
    fn pipeline_structural_url() {
        let c = IntentClassifier::new();
        let ctx = IntentContext::default();

        let result = tokio_test::block_on(c.classify("fetch https://example.com", &ctx));
        assert!(result.is_execute());
        if let IntentResult::Execute { metadata, .. } = &result {
            assert!(metadata.detected_url.is_some());
        }
    }

    #[test]
    fn pipeline_context_signal() {
        let c = IntentClassifier::new();
        let ctx = IntentContext {
            structural: StructuralContext {
                selected_file: Some("photo.jpg".to_string()),
                ..Default::default()
            },
        };

        let result = tokio_test::block_on(c.classify("what is this", &ctx));
        assert!(result.is_execute());
    }

    #[test]
    fn pipeline_fallback_default() {
        let c = IntentClassifier::builder().default_to_execute(true).build();
        let ctx = IntentContext::default();

        // No path, no URL, no AI → L4 default
        let result = tokio_test::block_on(c.classify("hello world", &ctx));
        assert!(result.is_execute());
        if let IntentResult::Execute { metadata, .. } = &result {
            assert_eq!(metadata.layer, DetectionLayer::L4Default);
        }
    }

    #[test]
    fn pipeline_layer_priority() {
        let c = IntentClassifier::new();

        // Abort beats everything
        let result = tokio_test::block_on(c.classify("stop", &IntentContext::default()));
        assert!(result.is_abort());

        // Slash command beats structural
        let result = tokio_test::block_on(c.classify("/screenshot", &IntentContext::default()));
        assert!(result.is_direct_tool());
    }
}
```

### Step 2: Run full test suite

```bash
cargo test -p alephcore --lib -v 2>&1 | tail -20
```

Expected: All tests pass. No compilation errors. No regressions.

### Step 3: Run cargo clippy

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20
```

Fix any warnings.

### Step 4: Final commit

```bash
git add -A
git commit -m "intent: add integration tests for language-agnostic pipeline"
```

---

## Verification Checklist

After all tasks are complete, verify:

1. **No hardcoded Chinese/English keywords** in `core/src/intent/`:
   ```bash
   # Should return 0 matches (excluding test data strings and keyword.rs engine code)
   grep -rn "整理\|删除\|运行\|打开\|搜索" core/src/intent/ --include="*.rs" | grep -v "test\|mod tests\|keyword.rs"
   ```

2. **No references to deleted types**:
   ```bash
   grep -rn "TaskCategory\|ExecutionIntent\|ExecutionMode\|ExecutionIntentDecider\|IntentRouter\|AggregatedIntent\|DecisionResult\|RouteResult" core/src/ --include="*.rs"
   ```
   Expected: 0 matches.

3. **Compilation**:
   ```bash
   cargo check -p alephcore
   ```

4. **Tests**:
   ```bash
   cargo test -p alephcore --lib
   ```

5. **No dead code warnings from deleted modules**:
   ```bash
   cargo check -p alephcore 2>&1 | grep "warning.*dead_code" | grep "intent"
   ```
   Expected: 0 matches.

---

## Risk Notes

- **Task 7 is the largest and riskiest** — 10+ caller files need migration. If compilation breaks mid-way, ensure each sub-step compiles independently before moving to the next.
- **`CommandParser` type** — used in both old `ExecutionIntentDecider` and new `IntentClassifier`. Verify it's defined outside the intent module (likely in `gateway/` or `routing/`). Do NOT delete it.
- **`ContextSignals` vs `StructuralContext`** — the old type has `active_app` and `ui_mode` fields not in the new type. If any caller uses those fields, add them to `StructuralContext`.
- **`RollbackManager`** — kept as-is, no changes needed. It's independent of intent classification.
- **`KeywordPolicy` in config** — the config struct in `config/types/policies/keyword.rs` stays. Only `with_builtin_rules()` changes to return an empty rule set.
