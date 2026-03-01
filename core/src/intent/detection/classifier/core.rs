//! Core IntentClassifier implementation.

use crate::sync_primitives::Arc;

use super::l1_regex::match_regex;
use super::l2_keywords::{
    create_keyword_index_from_policy, match_keywords,
    match_keywords_enhanced,
};
use super::l3_ai::convert_ai_result;
use super::types::{ExecutableTask, ExecutionIntent};
use crate::config::KeywordPolicy;
use crate::error::Result;
use crate::intent::decision::{
    AggregatedIntent, AggregatorConfig, CalibratedSignal, ConfidenceCalibrator, IntentAggregator,
    IntentSignal, RoutingLayer,
};
use crate::intent::detection::ai_detector::AiIntentDetector;
use crate::intent::detection::keyword::KeywordIndex;
use crate::intent::parameters::MatchingContext;
use crate::intent::support::{CachedIntent, IntentCache};
use crate::providers::AiProvider;

/// Intent classifier with 3-level classification
pub struct IntentClassifier {
    /// Confidence threshold for L2/L3 classification
    #[allow(dead_code)] // Architecture reserve: will gate L2/L3 classification results
    confidence_threshold: f32,
    /// Keyword index for enhanced L2 matching
    keyword_index: KeywordIndex,
    /// Optional AI detector for L3 classification
    ai_detector: Option<Arc<AiIntentDetector>>,
    /// Optional confidence calibrator for signal adjustment
    calibrator: Option<ConfidenceCalibrator>,
    /// Optional intent cache for fast-path routing
    cache: Option<Arc<IntentCache>>,
}

impl IntentClassifier {
    /// Create a new intent classifier
    pub fn new() -> Self {
        Self {
            confidence_threshold: 0.7,
            keyword_index: KeywordIndex::new(),
            ai_detector: None,
            calibrator: None,
            cache: None,
        }
    }

    /// Create classifier with keyword policy from config
    pub fn with_keyword_policy(policy: &KeywordPolicy) -> Self {
        Self {
            confidence_threshold: 0.7,
            keyword_index: create_keyword_index_from_policy(policy),
            ai_detector: None,
            calibrator: None,
            cache: None,
        }
    }

    /// Set AI provider for L3 classification
    pub fn with_ai_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.ai_detector = Some(Arc::new(AiIntentDetector::new(provider)));
        self
    }

    /// Set AI detector directly
    pub fn with_ai_detector(mut self, detector: Arc<AiIntentDetector>) -> Self {
        self.ai_detector = Some(detector);
        self
    }

    /// Set confidence calibrator for signal adjustment
    pub fn with_calibrator(mut self, calibrator: ConfidenceCalibrator) -> Self {
        self.calibrator = Some(calibrator);
        self
    }

    /// Set intent cache for fast-path routing
    pub fn with_cache(mut self, cache: Arc<IntentCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Get a reference to the calibrator (if set)
    pub fn calibrator(&self) -> Option<&ConfidenceCalibrator> {
        self.calibrator.as_ref()
    }

    /// Get a reference to the cache (if set)
    pub fn cache(&self) -> Option<&Arc<IntentCache>> {
        self.cache.as_ref()
    }

    /// L1: Regex pattern matching (<5ms)
    pub fn match_regex(&self, input: &str) -> Option<ExecutableTask> {
        match_regex(input)
    }

    /// L2: Keyword + rule matching (<20ms)
    pub fn match_keywords(&self, input: &str) -> Option<ExecutableTask> {
        match_keywords(input)
    }

    /// L2 Enhanced: Use KeywordIndex for weighted matching
    pub fn match_keywords_enhanced(&self, input: &str) -> Option<ExecutableTask> {
        match_keywords_enhanced(input, &self.keyword_index)
    }

    /// Convert AiIntentResult to ExecutableTask
    fn convert_ai_result(
        &self,
        result: &crate::intent::detection::ai_detector::AiIntentResult,
        input: &str,
    ) -> Option<ExecutableTask> {
        convert_ai_result(result, input)
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

        // L3: AI-based classification (optional)
        if let Some(ref detector) = self.ai_detector {
            if let Ok(Some(ai_result)) = detector.detect(input).await {
                if let Some(task) = self.convert_ai_result(&ai_result, input) {
                    return ExecutionIntent::Executable(task);
                }
            }
        }

        ExecutionIntent::Conversational
    }

    /// Classify with full context and return AggregatedIntent
    ///
    /// This method provides a more comprehensive classification using the full
    /// MatchingContext and returning an AggregatedIntent that includes:
    /// - Calibrated confidence scores
    /// - Action recommendations (Execute, Confirm, Clarify, GeneralChat)
    /// - Alternative signals
    /// - Conflict detection
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text
    /// * `context` - Full matching context with conversation, app, and time info
    ///
    /// # Returns
    ///
    /// `AggregatedIntent` with calibrated signals and action recommendation
    #[allow(unused_variables)]
    pub async fn classify_with_context(
        &self,
        input: &str,
        context: &MatchingContext,
    ) -> Result<AggregatedIntent> {
        // 1. Check cache first (if enabled)
        if let Some(ref cache) = self.cache {
            if let Some(cached) = cache.get(input).await {
                // Create signal from cached intent
                let signal = IntentSignal::with_tool(
                    &cached.intent_type,
                    &cached.tool_name,
                    cached.confidence,
                );
                let calibrated = CalibratedSignal::from_signal(
                    &signal,
                    cached.confidence,
                    RoutingLayer::L2Keyword, // Cached entries assumed to be L2-level
                );
                let aggregator = IntentAggregator::new(AggregatorConfig::default());
                return Ok(aggregator.from_single(calibrated));
            }
        }

        // 2. Try L1 regex matching (highest confidence)
        if let Some(task) = self.match_regex(input) {
            let category_str = format!("{:?}", task.category);
            let signal =
                IntentSignal::with_tool(category_str.clone(), category_str, task.confidence);
            let calibrated =
                CalibratedSignal::from_signal(&signal, task.confidence, RoutingLayer::L1Regex);
            let aggregator = IntentAggregator::new(AggregatorConfig::default());
            return Ok(aggregator.from_single(calibrated));
        }

        // 3. Try L2 keyword matching
        let l2_result = self
            .match_keywords_enhanced(input)
            .or_else(|| self.match_keywords(input));

        if let Some(task) = l2_result {
            let mut confidence = task.confidence;
            let category_str = format!("{:?}", task.category);

            // Apply calibration if calibrator is available
            if let Some(ref calibrator) = self.calibrator {
                let signal = IntentSignal::with_tool(
                    category_str.clone(),
                    category_str.clone(),
                    task.confidence,
                );
                // Get recent tools from conversation context for context boost
                let recent_tools = context.conversation.recent_intents.to_vec();

                let calibrated =
                    calibrator.calibrate(signal, RoutingLayer::L2Keyword, &recent_tools);
                confidence = calibrated.calibrated_confidence;
            }

            let signal =
                IntentSignal::with_tool(category_str.clone(), category_str, task.confidence);
            let calibrated =
                CalibratedSignal::from_signal(&signal, confidence, RoutingLayer::L2Keyword);
            let aggregator = IntentAggregator::new(AggregatorConfig::default());
            return Ok(aggregator.from_single(calibrated));
        }

        // 4. Try L3 AI detection (optional)
        if let Some(ref detector) = self.ai_detector {
            if let Ok(Some(ai_result)) = detector.detect(input).await {
                if let Some(task) = self.convert_ai_result(&ai_result, input) {
                    let mut confidence = task.confidence;

                    // Apply calibration if calibrator is available
                    if let Some(ref calibrator) = self.calibrator {
                        let signal = IntentSignal::with_tool(
                            ai_result.intent.clone(),
                            ai_result.intent.clone(),
                            task.confidence,
                        );
                        let recent_tools = context.conversation.recent_intents.to_vec();

                        let calibrated =
                            calibrator.calibrate(signal, RoutingLayer::L3Ai, &recent_tools);
                        confidence = calibrated.calibrated_confidence;
                    }

                    let signal = IntentSignal::with_tool(
                        ai_result.intent.clone(),
                        ai_result.intent,
                        task.confidence,
                    );
                    let calibrated =
                        CalibratedSignal::from_signal(&signal, confidence, RoutingLayer::L3Ai);
                    let aggregator = IntentAggregator::new(AggregatorConfig::default());
                    return Ok(aggregator.from_single(calibrated));
                }
            }
        }

        // 5. No match - return general chat
        Ok(AggregatedIntent::general_chat())
    }

    /// Cache an intent result for future fast-path routing
    ///
    /// This should be called after successful tool execution to improve
    /// future classification speed.
    pub async fn cache_intent(
        &self,
        input: &str,
        tool_name: &str,
        intent_type: &str,
        confidence: f32,
    ) {
        if let Some(ref cache) = self.cache {
            let cached = CachedIntent::new(input, tool_name, intent_type, confidence);
            cache.put(input, cached).await;
        }
    }

    /// Record a successful tool execution for learning
    ///
    /// Updates both cache and calibrator history if available.
    pub async fn record_success(&self, input: &str, tool_name: &str) {
        if let Some(ref cache) = self.cache {
            cache.record_success(input).await;
        }
        if let Some(ref calibrator) = self.calibrator {
            calibrator.record_success(tool_name, input).await;
        }
    }

    /// Record a failed/cancelled tool execution for learning
    ///
    /// Updates both cache and calibrator history if available.
    pub async fn record_failure(&self, input: &str, tool_name: &str) {
        if let Some(ref cache) = self.cache {
            cache.record_failure(input).await;
        }
        if let Some(ref calibrator) = self.calibrator {
            calibrator.record_failure(tool_name, input).await;
        }
    }
}

impl Default for IntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}
