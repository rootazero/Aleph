//! MatchingContext - Complete context for intent matching decisions

use super::{AppContext, ConversationContext, InputFeatures, PendingParam, TimeContext};

/// Complete context for intent matching decisions
///
/// Combines all context types into a single structure that can be passed
/// to intent matchers for comprehensive context-aware routing.
#[derive(Debug, Clone)]
pub struct MatchingContext {
    /// User input (with any command prefix)
    pub raw_input: String,

    /// Cleaned input (command prefix stripped if applicable)
    pub cleaned_input: Option<String>,

    /// Conversation context (multi-turn)
    pub conversation: ConversationContext,

    /// Application context
    pub app: AppContext,

    /// Time context
    pub time: TimeContext,

    /// Pre-extracted input features
    pub features: InputFeatures,
}

impl MatchingContext {
    /// Create a new MatchingContext builder
    pub fn builder() -> MatchingContextBuilder {
        MatchingContextBuilder::default()
    }

    /// Create a simple context for testing
    ///
    /// Uses default values for conversation, app, and time context.
    pub fn simple(input: impl Into<String>) -> Self {
        let raw_input = input.into();
        let features = InputFeatures::extract(&raw_input);

        Self {
            raw_input,
            cleaned_input: None,
            conversation: ConversationContext::default(),
            app: AppContext::unknown(),
            time: TimeContext::now(),
            features,
        }
    }

    /// Get the effective input (cleaned if available, otherwise raw)
    pub fn effective_input(&self) -> &str {
        self.cleaned_input.as_deref().unwrap_or(&self.raw_input)
    }

    /// Check if input contains URLs
    pub fn has_urls(&self) -> bool {
        !self.features.urls.is_empty()
    }

    /// Check if input is a question
    pub fn is_question(&self) -> bool {
        self.features.has_question_mark
    }

    /// Check if conversation has pending parameters
    pub fn has_pending_params(&self) -> bool {
        !self.conversation.pending_params.is_empty()
    }

    /// Get pending parameter for a specific intent
    pub fn get_pending_param(&self, intent: &str) -> Option<&PendingParam> {
        self.conversation.get_pending_for_intent(intent)
    }

    /// Check if input contains CJK characters
    pub fn has_cjk(&self) -> bool {
        self.features.has_cjk
    }

    /// Get word count of effective input
    pub fn word_count(&self) -> usize {
        self.features.word_count
    }

    /// Get character count of effective input
    pub fn char_count(&self) -> usize {
        self.features.char_count
    }
}

/// Builder for MatchingContext
#[derive(Debug, Default)]
pub struct MatchingContextBuilder {
    raw_input: String,
    cleaned_input: Option<String>,
    conversation: Option<ConversationContext>,
    app: Option<AppContext>,
    time: Option<TimeContext>,
    features: Option<InputFeatures>,
}

impl MatchingContextBuilder {
    /// Set raw input
    pub fn raw_input(mut self, input: impl Into<String>) -> Self {
        self.raw_input = input.into();
        self
    }

    /// Set cleaned input
    pub fn cleaned_input(mut self, input: impl Into<String>) -> Self {
        self.cleaned_input = Some(input.into());
        self
    }

    /// Set conversation context
    pub fn conversation(mut self, ctx: ConversationContext) -> Self {
        self.conversation = Some(ctx);
        self
    }

    /// Set application context
    pub fn app(mut self, ctx: AppContext) -> Self {
        self.app = Some(ctx);
        self
    }

    /// Set time context
    pub fn time(mut self, ctx: TimeContext) -> Self {
        self.time = Some(ctx);
        self
    }

    /// Set pre-extracted features
    pub fn features(mut self, features: InputFeatures) -> Self {
        self.features = Some(features);
        self
    }

    /// Build the MatchingContext
    pub fn build(self) -> MatchingContext {
        let features = self
            .features
            .unwrap_or_else(|| InputFeatures::extract(&self.raw_input));

        MatchingContext {
            raw_input: self.raw_input,
            cleaned_input: self.cleaned_input,
            conversation: self.conversation.unwrap_or_default(),
            app: self.app.unwrap_or_else(AppContext::unknown),
            time: self.time.unwrap_or_else(TimeContext::now),
            features,
        }
    }
}
