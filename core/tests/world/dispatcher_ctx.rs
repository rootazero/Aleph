//! Dispatcher Cortex test context for security pipeline, JSON parsing, and decision flow

use alephcore::dispatcher::cortex::{
    parser::{JsonFragment, JsonStreamDetector},
    security::{
        rules::{InstructionOverrideRule, PiiMaskerRule, TagInjectionRule},
        Locale, PipelineResult, SanitizeContext, SecurityConfig, SecurityPipeline,
    },
    DecisionAction, DecisionConfig,
};

/// Dispatcher context for BDD tests
pub struct DispatcherContext {
    // === Security Pipeline ===
    /// Security pipeline instance
    pub pipeline: Option<SecurityPipeline>,
    /// Sanitize context for pipeline processing
    pub sanitize_ctx: SanitizeContext,
    /// Result from pipeline processing
    pub pipeline_result: Option<PipelineResult>,

    // === JSON Stream Parsing ===
    /// JSON stream detector
    pub json_detector: Option<JsonStreamDetector>,
    /// Collected JSON fragments from streaming
    pub json_fragments: Vec<JsonFragment>,

    // === Decision Flow ===
    /// Decision configuration
    pub decision_config: Option<DecisionConfig>,
    /// Last decision action
    pub decision_action: Option<DecisionAction>,
    /// Test case results for decision thresholds
    pub decision_test_results: Vec<(f32, DecisionAction, bool)>, // (confidence, expected, passed)
}

impl std::fmt::Debug for DispatcherContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatcherContext")
            .field("pipeline", &self.pipeline.is_some())
            .field("sanitize_ctx", &self.sanitize_ctx)
            .field("pipeline_result", &self.pipeline_result.is_some())
            .field("json_detector", &self.json_detector.is_some())
            .field("json_fragments", &self.json_fragments.len())
            .field("decision_config", &self.decision_config)
            .field("decision_action", &self.decision_action)
            .field("decision_test_results", &self.decision_test_results.len())
            .finish()
    }
}

impl Default for DispatcherContext {
    fn default() -> Self {
        Self {
            pipeline: None,
            sanitize_ctx: SanitizeContext::default(),
            pipeline_result: None,
            json_detector: None,
            json_fragments: Vec::new(),
            decision_config: None,
            decision_action: None,
            decision_test_results: Vec::new(),
        }
    }
}

impl DispatcherContext {
    /// Create a security pipeline with all standard rules
    pub fn create_full_pipeline(&mut self) {
        let mut pipeline = SecurityPipeline::new(SecurityConfig::default_enabled());
        pipeline.add_rule(Box::new(InstructionOverrideRule::default()));
        pipeline.add_rule(Box::new(TagInjectionRule::default()));
        pipeline.add_rule(Box::new(PiiMaskerRule::new()));
        self.pipeline = Some(pipeline);
    }

    /// Create a security pipeline with only PII masking
    pub fn create_pii_only_pipeline(&mut self) {
        let mut pipeline = SecurityPipeline::new(SecurityConfig::default_enabled());
        pipeline.add_rule(Box::new(PiiMaskerRule::new()));
        self.pipeline = Some(pipeline);
    }

    /// Set locale on the sanitize context
    pub fn set_locale(&mut self, locale: Locale) {
        self.sanitize_ctx.locale = locale;
    }

    /// Process input through the security pipeline
    pub fn process_input(&mut self, input: &str) {
        if let Some(pipeline) = &self.pipeline {
            let result = pipeline.process(input, &self.sanitize_ctx);
            self.pipeline_result = Some(result);
        }
    }

    /// Initialize JSON stream detector
    pub fn init_json_detector(&mut self) {
        self.json_detector = Some(JsonStreamDetector::new());
        self.json_fragments.clear();
    }

    /// Push a chunk to the JSON detector
    pub fn push_json_chunk(&mut self, chunk: &str) {
        if let Some(detector) = &mut self.json_detector {
            let fragments = detector.push(chunk);
            self.json_fragments.extend(fragments);
        }
    }

    /// Initialize decision config with defaults
    pub fn init_decision_config(&mut self) {
        self.decision_config = Some(DecisionConfig::default());
    }

    /// Test a confidence value against expected action
    pub fn test_decision(&mut self, confidence: f32, expected: DecisionAction) {
        if let Some(config) = &self.decision_config {
            let actual = config.decide(confidence);
            let passed = actual == expected;
            self.decision_action = Some(actual.clone());
            self.decision_test_results
                .push((confidence, expected, passed));
        }
    }

    /// Get triggered rule names from pipeline result
    pub fn get_triggered_rules(&self) -> Vec<String> {
        self.pipeline_result
            .as_ref()
            .map(|r| r.actions.iter().map(|(name, _)| name.clone()).collect())
            .unwrap_or_default()
    }
}
