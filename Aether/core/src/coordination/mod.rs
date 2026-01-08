//! Conversation-Aware Routing Coordination Layer
//!
//! This module provides a higher-level abstraction over the Router that integrates
//! conversation context, semantic matching, and intent detection for enhanced routing.
//!
//! # Architecture
//!
//! ```text
//! ConversationAwareRouter (coordination layer)
//! ├── Router (core rule matching)
//! ├── ConversationManager (session/history tracking)
//! ├── SemanticMatcher (optional semantic matching)
//! └── AiIntentDetector (optional AI-powered intent detection)
//! ```
//!
//! # Design Principles
//!
//! - **Separation of Concerns**: Each component has a single responsibility
//! - **Gradual Enhancement**: Optional components can be enabled independently
//! - **Backward Compatible**: Works with existing Router API
//!
//! # Semantic Matching
//!
//! The coordination layer can optionally use SemanticMatcher for enhanced matching:
//! - Layer 1: Fast path (command/regex matching via Router)
//! - Layer 2: Keyword index matching
//! - Layer 3: Context-aware inference
//! - Layer 4: AI detection fallback
//!
//! SemanticMatcher can be provided in two ways:
//! 1. Via `with_semantic_matcher()` - independent instance
//! 2. Via Router's embedded matcher - for backward compatibility

pub mod routing_result;

use crate::conversation::ConversationManager;
use crate::core::CapturedContext;
use crate::router::Router;
use crate::semantic::{MatchResult, MatchingContext, SemanticMatcher};
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

// Re-exports
pub use routing_result::RoutingResult;

/// Context for routing decisions that includes conversation history
#[derive(Debug, Clone, Default)]
pub struct RoutingContext {
    /// User input text (original, before any modifications)
    pub input: String,
    /// Captured context (app, window)
    pub captured_context: Option<CapturedContext>,
    /// Conversation history context (formatted string for AI prompt)
    pub conversation_context: Option<String>,
    /// Whether this is a continuation of an existing conversation
    pub is_continuation: bool,
    /// Current turn number in conversation
    pub turn_number: u32,
}

impl RoutingContext {
    /// Create a new routing context
    pub fn new(input: String) -> Self {
        Self {
            input,
            ..Default::default()
        }
    }

    /// Add captured context
    pub fn with_captured_context(mut self, context: CapturedContext) -> Self {
        self.captured_context = Some(context);
        self
    }

    /// Add conversation context
    pub fn with_conversation(
        mut self,
        context: String,
        is_continuation: bool,
        turn_number: u32,
    ) -> Self {
        self.conversation_context = Some(context);
        self.is_continuation = is_continuation;
        self.turn_number = turn_number;
        self
    }

    /// Build the routing string for Router.match_rules()
    ///
    /// Format: ClipboardContent\n---\n[AppName] WindowTitle
    /// This preserves backward compatibility with ^/prefix rules.
    pub fn build_routing_string(&self) -> String {
        if let Some(ref ctx) = self.captured_context {
            // Extract app name from bundle ID (e.g., "com.apple.Notes" → "Notes")
            let app_name = ctx
                .app_bundle_id
                .split('.')
                .next_back()
                .unwrap_or("Unknown");

            format!(
                "{}\n---\n[{}] {}",
                self.input,
                app_name,
                ctx.window_title.as_deref().unwrap_or("")
            )
        } else {
            self.input.clone()
        }
    }
}

/// Conversation-aware router that coordinates routing with conversation context
///
/// This is the main entry point for routing in Aether. It wraps the core Router
/// and adds conversation awareness, semantic matching, and intent detection.
pub struct ConversationAwareRouter {
    /// Core router for rule-based matching
    router: Arc<Router>,
    /// Conversation manager for session tracking
    conversation_manager: Arc<Mutex<ConversationManager>>,
    /// Optional semantic matcher for enhanced matching
    #[allow(dead_code)]
    semantic_matcher: Option<Arc<SemanticMatcher>>,
    /// Whether conversation-aware routing is enabled
    enabled: bool,
}

impl ConversationAwareRouter {
    /// Create a new conversation-aware router
    ///
    /// # Arguments
    /// * `router` - The core Router instance
    /// * `conversation_manager` - Shared conversation manager
    pub fn new(
        router: Arc<Router>,
        conversation_manager: Arc<Mutex<ConversationManager>>,
    ) -> Self {
        Self {
            router,
            conversation_manager,
            semantic_matcher: None,
            enabled: true,
        }
    }

    /// Add semantic matcher for enhanced matching
    pub fn with_semantic_matcher(mut self, matcher: Arc<SemanticMatcher>) -> Self {
        self.semantic_matcher = Some(matcher);
        self
    }

    /// Enable or disable conversation-aware routing
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if there's an active conversation session
    pub fn has_active_session(&self) -> bool {
        self.conversation_manager
            .lock()
            .map(|m| m.has_active_session())
            .unwrap_or(false)
    }

    /// Start a new conversation session
    pub fn start_session(&self, context: CapturedContext) -> String {
        self.conversation_manager
            .lock()
            .map(|mut m| m.start_session(context))
            .unwrap_or_else(|_| String::new())
    }

    /// End the current conversation session
    pub fn end_session(&self) {
        let _ = self
            .conversation_manager
            .lock()
            .map(|mut m| m.end_session());
    }

    /// Add a turn to the current conversation
    pub fn add_turn(&self, user_input: String, ai_response: String) {
        let _ = self
            .conversation_manager
            .lock()
            .map(|mut m| m.add_turn(user_input, ai_response));
    }

    /// Get the current turn count
    pub fn turn_count(&self) -> u32 {
        self.conversation_manager
            .lock()
            .map(|m| m.turn_count())
            .unwrap_or(0)
    }

    /// Route with conversation context
    ///
    /// This is the main routing method that considers conversation history
    /// when making routing decisions.
    ///
    /// # Arguments
    /// * `input` - User input text
    /// * `context` - Captured context (app, window, attachments)
    ///
    /// # Returns
    /// A RoutingResult containing the routing decision and conversation context
    pub fn route_with_context(&self, input: &str, context: &CapturedContext) -> RoutingResult {
        // Build routing context with conversation history
        let routing_ctx = self.build_routing_context(input, context);

        // Build the routing string for the core router
        let routing_string = routing_ctx.build_routing_string();

        // Get rule-based routing match from core router
        let routing_match = self.router.match_rules(&routing_string);

        // Build the result
        RoutingResult {
            routing_match,
            conversation_context: routing_ctx.conversation_context,
            is_continuation: routing_ctx.is_continuation,
            turn_number: routing_ctx.turn_number,
            semantic_match: None, // TODO: Add semantic matching in Step 2.2
        }
    }

    /// Build routing context with conversation history
    fn build_routing_context(&self, input: &str, context: &CapturedContext) -> RoutingContext {
        let mut routing_ctx =
            RoutingContext::new(input.to_string()).with_captured_context(context.clone());

        // Add conversation context if available
        if let Ok(manager) = self.conversation_manager.lock() {
            if manager.has_active_session() {
                let conversation_prompt = manager.build_context_prompt();
                let turn_number = manager.turn_count();
                let is_continuation = turn_number > 0;

                if !conversation_prompt.is_empty() {
                    routing_ctx = routing_ctx.with_conversation(
                        conversation_prompt,
                        is_continuation,
                        turn_number,
                    );

                    debug!(
                        turn_number = turn_number,
                        is_continuation = is_continuation,
                        "Added conversation context to routing"
                    );
                }
            }
        }

        routing_ctx
    }

    /// Get the underlying router
    pub fn router(&self) -> &Arc<Router> {
        &self.router
    }

    /// Get the conversation manager
    pub fn conversation_manager(&self) -> &Arc<Mutex<ConversationManager>> {
        &self.conversation_manager
    }

    // ========================================================================
    // Semantic Matching Methods
    // ========================================================================

    /// Check if semantic matching is available
    ///
    /// Returns true if either:
    /// - An independent SemanticMatcher was provided
    /// - The Router has an embedded SemanticMatcher
    pub fn is_semantic_matching_available(&self) -> bool {
        self.semantic_matcher.is_some() || self.router.is_semantic_matching_enabled()
    }

    /// Get the semantic matcher
    ///
    /// Returns the independent matcher if provided, otherwise tries the router's embedded matcher.
    pub fn get_semantic_matcher(&self) -> Option<&SemanticMatcher> {
        self.semantic_matcher
            .as_ref()
            .map(|m| m.as_ref())
            .or_else(|| self.router.semantic_matcher())
    }

    /// Perform semantic matching with conversation context
    ///
    /// This method combines conversation history with semantic detection:
    /// 1. Builds MatchingContext with conversation history
    /// 2. Runs semantic matcher (keyword + context inference)
    /// 3. Returns MatchResult with intent classification
    ///
    /// # Arguments
    /// * `input` - User input text
    /// * `context` - Captured context (app, window)
    ///
    /// # Returns
    /// * `Some(MatchResult)` - Semantic match result
    /// * `None` - Semantic matching is not available
    pub async fn route_semantic(
        &self,
        input: &str,
        context: &CapturedContext,
    ) -> Option<MatchResult> {
        // Get the matcher (independent or from router)
        let matcher = self.get_semantic_matcher()?;

        // Build MatchingContext with conversation history
        let matching_context = self.build_matching_context(input, context);

        info!(
            input_length = input.len(),
            has_conversation = !matching_context.conversation.history.is_empty(),
            "Performing semantic matching with conversation context"
        );

        // Perform semantic matching
        Some(matcher.match_input(&matching_context).await)
    }

    /// Build a MatchingContext for semantic detection
    ///
    /// Creates a context that includes:
    /// - User input
    /// - App context (bundle ID, window title)
    /// - Conversation history (if available)
    fn build_matching_context(&self, input: &str, context: &CapturedContext) -> MatchingContext {
        use crate::semantic::{
            AppContext, ConversationContext as SemanticConversationContext,
            ConversationTurn as SemanticTurn,
        };

        // Use the builder pattern from semantic module
        let mut builder = MatchingContext::builder().raw_input(input);

        // Extract app name from bundle ID (e.g., "com.apple.Notes" → "Notes")
        let app_name = context
            .app_bundle_id
            .split('.')
            .next_back()
            .unwrap_or("Unknown");

        // Build app context
        let mut app_context = AppContext::new(&context.app_bundle_id, app_name);
        if let Some(ref title) = context.window_title {
            app_context = app_context.with_window_title(title);
        }
        builder = builder.app(app_context);

        // Build conversation context if available
        if let Ok(manager) = self.conversation_manager.lock() {
            if let Some(session) = manager.active_session() {
                // Convert ConversationManager turns to semantic ConversationTurns
                let semantic_turns: Vec<SemanticTurn> = session
                    .turns
                    .iter()
                    .map(|turn| SemanticTurn::new(&turn.user_input, &turn.ai_response))
                    .collect();

                if !semantic_turns.is_empty() {
                    let conv_ctx = SemanticConversationContext {
                        session_id: Some(session.id().to_string()),
                        turn_count: session.turn_count(),
                        previous_intents: Vec::new(),
                        pending_params: std::collections::HashMap::new(),
                        last_response_summary: session
                            .turns
                            .last()
                            .map(|t| t.ai_response.chars().take(200).collect()),
                        history: semantic_turns,
                    };
                    builder = builder.conversation(conv_ctx);
                }
            }
        }

        builder.build()
    }

    /// Perform keyword matching only (synchronous)
    ///
    /// This is useful for quick keyword checks without the full semantic pipeline.
    pub fn match_keywords(&self, input: &str) -> Vec<crate::semantic::KeywordMatch> {
        if let Some(matcher) = self.get_semantic_matcher() {
            matcher.match_keywords_only(input)
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_context() -> CapturedContext {
        CapturedContext {
            app_bundle_id: "com.apple.Notes".to_string(),
            window_title: Some("Test Note".to_string()),
            attachments: None,
        }
    }

    #[test]
    fn test_routing_context_builder() {
        let ctx = RoutingContext::new("Hello".to_string())
            .with_captured_context(create_test_context())
            .with_conversation("Previous: Hi".to_string(), true, 1);

        assert_eq!(ctx.input, "Hello");
        assert!(ctx.captured_context.is_some());
        assert!(ctx.conversation_context.is_some());
        assert!(ctx.is_continuation);
        assert_eq!(ctx.turn_number, 1);
    }

    #[test]
    fn test_build_routing_string() {
        let ctx = RoutingContext::new("/en Hello world".to_string())
            .with_captured_context(create_test_context());

        let routing_string = ctx.build_routing_string();

        assert!(routing_string.contains("/en Hello world"));
        assert!(routing_string.contains("[Notes]"));
        assert!(routing_string.contains("Test Note"));
    }
}
