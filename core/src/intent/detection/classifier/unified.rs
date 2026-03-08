//! Unified intent classification pipeline.
//!
//! Combines all detection layers (abort, command, structural, keyword, AI)
//! into a single async pipeline. This replaces the older multi-module
//! `IntentClassifier` + intent decider combination.
//!
//! # Pipeline Order
//!
//! 1. Abort check (exact match against multilingual stop words)
//! 2. Cache lookup
//! 3. L0: Slash command parsing
//! 4. L1: Structural detection (paths, URLs, context signals)
//! 5. L2: Keyword matching (weighted, CJK-aware)
//! 6. L3: AI binary classification (lightweight LLM)
//! 7. L4: Default fallback

use crate::command::{CommandContext, CommandParser, ParsedCommand};
use crate::dispatcher::ToolSourceType;
use crate::intent::decision::ConfidenceCalibrator;
use crate::intent::detection::abort::AbortDetector;
use crate::intent::detection::ai_binary::AiBinaryClassifier;
use crate::intent::detection::keyword::KeywordIndex;
use crate::intent::detection::structural::{StructuralContext, StructuralDetector};
use crate::intent::support::IntentCache;
use crate::intent::types::{
    DetectionLayer, DirectToolSource, ExecuteMetadata, IntentResult,
};
use crate::sync_primitives::Arc;

// ─── Context & Config ────────────────────────────────────────────────

/// Context passed to the classifier pipeline.
#[derive(Debug, Clone, Default)]
pub struct IntentContext {
    /// Structural context signals from the UI environment.
    pub structural: StructuralContext,
}

/// Pipeline configuration.
#[derive(Debug, Clone)]
pub struct IntentConfig {
    /// When all layers fail to classify, default to Execute (true) or Converse (false).
    pub default_to_execute: bool,
}

impl Default for IntentConfig {
    fn default() -> Self {
        Self {
            default_to_execute: true,
        }
    }
}

// ─── Built-in Slash Command Table ────────────────────────────────────

/// Map a built-in slash command name to its tool_id.
fn builtin_slash_tool(cmd: &str) -> Option<&'static str> {
    match cmd {
        "screenshot" => Some("screenshot"),
        "ocr" => Some("vision_ocr"),
        "search" => Some("search"),
        "webfetch" => Some("web_fetch"),
        "gen" => Some("generate_image"),
        _ => None,
    }
}

// ─── UnifiedIntentClassifier ─────────────────────────────────────────

/// Unified intent classifier combining all detection layers.
pub struct UnifiedIntentClassifier {
    abort_detector: AbortDetector,
    command_parser: Option<Arc<CommandParser>>,
    structural_detector: StructuralDetector,
    keyword_index: Option<KeywordIndex>,
    ai_classifier: Option<AiBinaryClassifier>,
    cache: Option<Arc<IntentCache>>,
    calibrator: Option<ConfidenceCalibrator>,
    config: IntentConfig,
}

impl UnifiedIntentClassifier {
    /// Create a classifier with sensible defaults (no AI, no keywords, no cache).
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

    /// Return a builder for fine-grained configuration.
    pub fn builder() -> UnifiedIntentClassifierBuilder {
        UnifiedIntentClassifierBuilder::default()
    }

    /// Main classification entry point.
    ///
    /// Runs through the pipeline layers in order and returns as soon as
    /// a decision is made.
    pub async fn classify(&self, input: &str, context: &IntentContext) -> IntentResult {
        // 1. Abort check — highest priority
        if self.abort_detector.is_abort(input) {
            return IntentResult::Abort;
        }

        // 2. Cache lookup
        if let Some(ref cache) = self.cache {
            if let Some(cached) = cache.get_cached_result(input).await {
                return cached;
            }
        }

        // 3. L0: Slash commands
        if input.trim().starts_with('/') {
            if let Some(result) = self.detect_command(input).await {
                return result;
            }
        }

        // 4. L1: Structural detection (paths, URLs, context signals)
        if let Some(result) = self.structural_detector.detect(input, &context.structural) {
            let result = self.apply_calibration(result);
            self.update_cache(input, &result).await;
            return result;
        }

        // 5. L2: Keyword matching
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
                self.update_cache(input, &result).await;
                return result;
            }
        }

        // 6. L3: AI binary classification
        if let Some(ref ai) = self.ai_classifier {
            if let Some(result) = ai.classify(input).await {
                let result = self.apply_calibration(result);
                self.update_cache(input, &result).await;
                return result;
            }
        }

        // 7. L4: Default fallback
        if self.config.default_to_execute {
            IntentResult::Execute {
                confidence: 0.5,
                metadata: ExecuteMetadata::default_with_layer(DetectionLayer::L4Default),
            }
        } else {
            IntentResult::Converse { confidence: 0.5 }
        }
    }

    /// Detect a slash command and convert to `IntentResult`.
    async fn detect_command(&self, input: &str) -> Option<IntentResult> {
        let trimmed = input.trim();

        // Try the dynamic CommandParser first
        if let Some(ref parser) = self.command_parser {
            if let Some(parsed) = parser.parse_async(trimmed).await {
                return Some(Self::parsed_command_to_result(parsed));
            }
        }

        // Fallback: manual parsing
        let without_slash = &trimmed[1..];
        let (raw_cmd, args) = match without_slash.split_once(char::is_whitespace) {
            Some((name, rest)) => (name.to_lowercase(), Some(rest.trim().to_string())),
            None => (without_slash.to_lowercase(), None),
        };
        // Strip @botname suffix (Telegram group commands)
        let cmd = match raw_cmd.split_once('@') {
            Some((name, _)) => name.to_string(),
            None => raw_cmd,
        };

        // Check built-in table
        let tool_id = builtin_slash_tool(&cmd)?;
        Some(IntentResult::DirectTool {
            tool_id: tool_id.to_string(),
            args,
            source: DirectToolSource::SlashCommand,
        })
    }

    /// Convert a `ParsedCommand` (from `CommandParser`) into an `IntentResult`.
    fn parsed_command_to_result(cmd: ParsedCommand) -> IntentResult {
        let args = cmd.arguments.clone();

        let (tool_id, source) = match cmd.source_type {
            ToolSourceType::Builtin | ToolSourceType::Native => {
                let id = if let CommandContext::Builtin { tool_name } = &cmd.context {
                    tool_name.clone()
                } else {
                    cmd.command_name.clone()
                };
                (id, DirectToolSource::SlashCommand)
            }
            ToolSourceType::Skill => {
                let id = if let CommandContext::Skill { skill_id, .. } = &cmd.context {
                    skill_id.clone()
                } else {
                    cmd.command_name.clone()
                };
                (id, DirectToolSource::Skill)
            }
            ToolSourceType::Mcp => {
                let id = if let CommandContext::Mcp {
                    server_name,
                    tool_name,
                } = &cmd.context
                {
                    tool_name
                        .clone()
                        .unwrap_or_else(|| server_name.clone())
                } else {
                    cmd.command_name.clone()
                };
                (id, DirectToolSource::Mcp)
            }
            ToolSourceType::Custom => (cmd.command_name.clone(), DirectToolSource::Custom),
        };

        IntentResult::DirectTool {
            tool_id,
            args,
            source,
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn apply_calibration(&self, result: IntentResult) -> IntentResult {
        if let Some(ref cal) = self.calibrator {
            cal.calibrate_result(&result, &[])
        } else {
            result
        }
    }

    async fn update_cache(&self, input: &str, result: &IntentResult) {
        if let Some(ref cache) = self.cache {
            cache.cache_result(input, result).await;
        }
    }

    // ── Accessors ────────────────────────────────────────────────────

    /// Access the intent cache, if configured.
    pub fn cache(&self) -> Option<&Arc<IntentCache>> {
        self.cache.as_ref()
    }

    /// Access the calibrator, if configured.
    pub fn calibrator(&self) -> Option<&ConfidenceCalibrator> {
        self.calibrator.as_ref()
    }

    /// Replace the command parser at runtime.
    pub fn set_command_parser(&mut self, parser: Arc<CommandParser>) {
        self.command_parser = Some(parser);
    }
}

impl Default for UnifiedIntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Builder ─────────────────────────────────────────────────────────

/// Builder for `UnifiedIntentClassifier`.
pub struct UnifiedIntentClassifierBuilder {
    command_parser: Option<Arc<CommandParser>>,
    keyword_index: Option<KeywordIndex>,
    ai_classifier: Option<AiBinaryClassifier>,
    cache: Option<Arc<IntentCache>>,
    calibrator: Option<ConfidenceCalibrator>,
    default_to_execute: bool,
}

impl Default for UnifiedIntentClassifierBuilder {
    fn default() -> Self {
        Self {
            command_parser: None,
            keyword_index: None,
            ai_classifier: None,
            cache: None,
            calibrator: None,
            default_to_execute: true, // Match IntentConfig::default()
        }
    }
}

impl UnifiedIntentClassifierBuilder {
    /// Set the default fallback behaviour.
    pub fn default_to_execute(mut self, value: bool) -> Self {
        self.default_to_execute = value;
        self
    }

    /// Attach a command parser for dynamic slash-command resolution.
    pub fn with_command_parser(mut self, parser: Arc<CommandParser>) -> Self {
        self.command_parser = Some(parser);
        self
    }

    /// Attach a keyword index for L2 matching.
    pub fn with_keyword_index(mut self, index: KeywordIndex) -> Self {
        self.keyword_index = Some(index);
        self
    }

    /// Attach an AI binary classifier for L3 matching.
    pub fn with_ai_classifier(mut self, ai: AiBinaryClassifier) -> Self {
        self.ai_classifier = Some(ai);
        self
    }

    /// Attach an intent cache.
    pub fn with_cache(mut self, cache: Arc<IntentCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Attach a confidence calibrator.
    pub fn with_calibrator(mut self, cal: ConfidenceCalibrator) -> Self {
        self.calibrator = Some(cal);
        self
    }

    /// Build the classifier.
    pub fn build(self) -> UnifiedIntentClassifier {
        UnifiedIntentClassifier {
            abort_detector: AbortDetector::new(),
            command_parser: self.command_parser,
            structural_detector: StructuralDetector::new(),
            keyword_index: self.keyword_index,
            ai_classifier: self.ai_classifier,
            cache: self.cache,
            calibrator: self.calibrator,
            config: IntentConfig {
                default_to_execute: self.default_to_execute,
            },
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn abort_wins() {
        let c = UnifiedIntentClassifier::new();
        let result = c.classify("stop", &IntentContext::default()).await;
        assert!(result.is_abort());
    }

    #[tokio::test]
    async fn slash_command_screenshot() {
        let c = UnifiedIntentClassifier::new();
        let result = c.classify("/screenshot", &IntentContext::default()).await;
        assert!(result.is_direct_tool());
        if let IntentResult::DirectTool { tool_id, .. } = &result {
            assert_eq!(tool_id, "screenshot");
        }
    }

    #[tokio::test]
    async fn slash_command_with_args() {
        let c = UnifiedIntentClassifier::new();
        let result = c
            .classify("/search rust async", &IntentContext::default())
            .await;
        assert!(result.is_direct_tool());
        if let IntentResult::DirectTool { tool_id, args, .. } = &result {
            assert_eq!(tool_id, "search");
            assert_eq!(args.as_deref(), Some("rust async"));
        }
    }

    #[tokio::test]
    async fn structural_path() {
        let c = UnifiedIntentClassifier::new();
        let result = c
            .classify("read /etc/hosts", &IntentContext::default())
            .await;
        assert!(result.is_execute());
        if let IntentResult::Execute { metadata, .. } = &result {
            assert_eq!(metadata.layer, DetectionLayer::L1);
            assert!(metadata.detected_path.is_some());
        }
    }

    #[tokio::test]
    async fn context_signal() {
        let c = UnifiedIntentClassifier::new();
        let ctx = IntentContext {
            structural: StructuralContext {
                selected_file: Some("photo.jpg".to_string()),
                ..Default::default()
            },
        };
        let result = c.classify("what is this", &ctx).await;
        assert!(result.is_execute());
    }

    #[tokio::test]
    async fn default_execute_fallback() {
        let c = UnifiedIntentClassifier::builder()
            .default_to_execute(true)
            .build();
        let result = c
            .classify(
                "hello world how are you doing today",
                &IntentContext::default(),
            )
            .await;
        assert!(result.is_execute());
        if let IntentResult::Execute { metadata, .. } = &result {
            assert_eq!(metadata.layer, DetectionLayer::L4Default);
        }
    }

    #[tokio::test]
    async fn default_converse_fallback() {
        let c = UnifiedIntentClassifier::builder()
            .default_to_execute(false)
            .build();
        let result = c
            .classify(
                "hello world how are you doing today",
                &IntentContext::default(),
            )
            .await;
        assert!(result.is_converse());
    }

    #[tokio::test]
    async fn abort_beats_slash_command() {
        // "stop" is both an abort trigger and could look like a word
        let c = UnifiedIntentClassifier::new();
        let result = c.classify("stop", &IntentContext::default()).await;
        assert!(result.is_abort()); // Abort checked first
    }

    #[tokio::test]
    async fn unknown_slash_command_falls_through() {
        let c = UnifiedIntentClassifier::new();
        let result = c
            .classify("/unknowncmd", &IntentContext::default())
            .await;
        // Should fall through to L1/L2/L3/L4 since /unknowncmd is not a built-in
        assert!(!result.is_direct_tool());
    }
}
