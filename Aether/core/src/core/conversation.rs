//! Conversation management for AetherCore
//!
//! This module contains all multi-turn conversation methods:
//! - Starting and ending conversations
//! - Continuing conversations with follow-up input
//! - Conversation state queries

use super::types::CapturedContext;
use super::AetherCore;
use crate::error::AetherException;
use std::sync::Arc;
use tracing::info;

impl AetherCore {
    // ========================================================================
    // Multi-Turn Conversation API (add-multi-turn-conversation)
    // ========================================================================

    /// Start a new conversation session.
    ///
    /// This initiates a multi-turn conversation. The first AI response will be
    /// printed to the target window and cached. Subsequent inputs can be processed
    /// via `continue_conversation()`.
    ///
    /// # Arguments
    /// * `initial_input` - The user's initial message
    /// * `context` - The captured context (app, window) at session start
    ///
    /// # Returns
    /// * `Result<String>` - The AI's response, or an error
    pub fn start_conversation(
        &self,
        initial_input: String,
        context: CapturedContext,
    ) -> std::result::Result<String, AetherException> {
        info!(
            input_preview = %initial_input.chars().take(50).collect::<String>(),
            app = %context.app_bundle_id,
            "Starting new conversation session"
        );

        // Start a new session in the conversation manager
        let session_id = {
            let mut manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            manager.start_session(context.clone())
        };

        // Notify UI that conversation started
        self.event_handler
            .on_conversation_started(session_id.clone());

        // Store context for memory operations (required for memory storage)
        self.set_current_context(context.clone());

        // Process the initial input using AI-first mode
        let start_time = std::time::Instant::now();
        let response =
            match self.process_with_ai_first(initial_input.clone(), context.clone(), start_time) {
                Ok(r) => r,
                Err(e) => {
                    // End the session on error
                    let mut manager = self
                        .conversation_manager
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    manager.end_session();
                    drop(manager);
                    return Err(self.handle_processing_error(&e));
                }
            };

        // Add the turn to conversation history
        {
            let mut manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(turn) = manager.add_turn(initial_input, response.clone()) {
                // Notify UI about the completed turn
                self.event_handler.on_conversation_turn_completed(
                    crate::conversation::ConversationTurn {
                        turn_id: turn.turn_id,
                        user_input: turn.user_input,
                        ai_response: turn.ai_response,
                        timestamp: turn.timestamp,
                    },
                );
            }
        }

        // Notify UI that continuation is available
        info!("Notifying UI: conversation continuation ready");
        self.event_handler.on_conversation_continuation_ready();
        info!("UI notified: conversation continuation ready callback completed");

        Ok(response)
    }

    /// Continue an existing conversation with follow-up input.
    ///
    /// This method:
    /// 1. Builds context from conversation history
    /// 2. Processes the follow-up with AI (including any new attachments)
    /// 3. Adds the turn to history
    /// 4. Returns the AI response (for printing to target window)
    ///
    /// # Arguments
    /// * `follow_up_input` - The user's follow-up message
    /// * `context` - Optional context with new attachments for this turn
    ///
    /// # Returns
    /// * `Result<String>` - The AI's response, or an error
    pub fn continue_conversation(
        &self,
        follow_up_input: String,
        context: Option<CapturedContext>,
    ) -> std::result::Result<String, AetherException> {
        // Check if there's an active session first
        {
            let manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !manager.has_active_session() {
                drop(manager); // Release lock before calling on_error
                self.event_handler.on_error(
                    "No active conversation. Start a new conversation first.".to_string(),
                    Some("Call start_conversation() to begin a new session.".to_string()),
                );
                return Err(AetherException::Error);
            }
        }

        // Get conversation context (session exists, checked above)
        let (context_prompt, session_context, turn_count) = {
            let manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let session = manager.active_session().unwrap();
            (
                manager.build_context_prompt(),
                session.context.clone(),
                session.turn_count(),
            )
        };

        // Use new context if provided (with attachments), otherwise use session context
        // This allows subsequent turns to include new attachments (e.g., copied files)
        let effective_context = if let Some(new_ctx) = context {
            // Log attachment info for debugging
            let attachment_count = new_ctx.attachments.as_ref().map_or(0, |a| a.len());
            if attachment_count > 0 {
                info!(
                    attachment_count = attachment_count,
                    "Continuation includes new attachments"
                );
            }
            // Use new context but preserve session's app/window info if not provided
            CapturedContext {
                app_bundle_id: if new_ctx.app_bundle_id.is_empty() {
                    session_context.app_bundle_id.clone()
                } else {
                    new_ctx.app_bundle_id
                },
                window_title: new_ctx.window_title.or(session_context.window_title.clone()),
                attachments: new_ctx.attachments,
                topic_id: new_ctx.topic_id.or(session_context.topic_id.clone()),
            }
        } else {
            session_context.clone()
        };

        info!(
            input_preview = %follow_up_input.chars().take(50).collect::<String>(),
            turn_count = turn_count,
            has_attachments = effective_context.attachments.is_some(),
            "Continuing conversation"
        );

        // Store context for memory operations (required for memory storage)
        self.set_current_context(effective_context.clone());

        // Build augmented input with conversation history
        let augmented_input = if context_prompt.is_empty() {
            follow_up_input.clone()
        } else {
            format!("{}\n\n当前问题: {}", context_prompt, follow_up_input)
        };

        // Process with AI using AI-first mode (with effective context including new attachments)
        let start_time = std::time::Instant::now();
        let response =
            match self.process_with_ai_first(augmented_input.clone(), effective_context.clone(), start_time)
            {
                Ok(r) => r,
                Err(e) => {
                    // End the session on error
                    let mut manager = self
                        .conversation_manager
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    manager.end_session();
                    drop(manager);
                    return Err(self.handle_processing_error(&e));
                }
            };

        // Add the turn to conversation history
        {
            let mut manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(turn) = manager.add_turn(follow_up_input, response.clone()) {
                // Notify UI about the completed turn
                self.event_handler.on_conversation_turn_completed(
                    crate::conversation::ConversationTurn {
                        turn_id: turn.turn_id,
                        user_input: turn.user_input,
                        ai_response: turn.ai_response,
                        timestamp: turn.timestamp,
                    },
                );
            }
        }

        // Notify UI that continuation is still available
        self.event_handler.on_conversation_continuation_ready();

        Ok(response)
    }

    /// End the current conversation session.
    ///
    /// This should be called when the user presses ESC to close the Halo input.
    pub fn end_conversation(&self) {
        let session = {
            let mut manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            manager.end_session()
        };

        if let Some(ended_session) = session {
            info!(
                session_id = %ended_session.session_id,
                turns = ended_session.turn_count(),
                "Conversation session ended"
            );

            // Notify UI
            self.event_handler.on_conversation_ended(
                ended_session.session_id.clone(),
                ended_session.turn_count(),
            );
        }
    }

    /// Check if there's an active conversation session.
    pub fn has_active_conversation(&self) -> bool {
        let manager = self
            .conversation_manager
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        manager.has_active_session()
    }

    /// Get the current turn count for the active conversation.
    pub fn conversation_turn_count(&self) -> u32 {
        let manager = self
            .conversation_manager
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        manager.turn_count()
    }

    // ========================================================================
    // Topic Title Generation
    // ========================================================================

    /// Generate a concise title for a conversation topic.
    ///
    /// This method uses the default AI provider to generate a short title
    /// based on the first exchange in a conversation. The title should be
    /// suitable for display in a conversation history list.
    ///
    /// Note: This is a synchronous function that internally uses the stored
    /// Tokio runtime to execute async operations. This avoids issues with
    /// UniFFI async calls from non-Tokio threads.
    ///
    /// # Arguments
    /// * `user_input` - The user's first message in the conversation
    /// * `ai_response` - The AI's first response in the conversation
    ///
    /// # Returns
    /// * `Result<String>` - A short title (max 50 chars), or a default title if AI fails
    pub fn generate_topic_title(
        &self,
        user_input: String,
        ai_response: String,
    ) -> std::result::Result<String, AetherException> {
        use crate::title_generator;

        info!(
            user_input_len = user_input.len(),
            ai_response_len = ai_response.len(),
            "Generating topic title"
        );

        // Get the default provider
        let provider = match self.get_default_provider_instance() {
            Some(p) => p,
            None => {
                // No provider available, use default title
                let default = title_generator::default_title(&user_input);
                info!(default_title = %default, "No provider available, using default title");
                return Ok(default);
            }
        };

        // Build the title prompt
        let prompt = title_generator::build_title_prompt(&user_input, &ai_response);

        // Execute async AI call using the stored runtime
        // This avoids the "no reactor running" panic when called from non-Tokio threads
        let runtime = Arc::clone(&self.runtime);
        let ai_result: Result<String, crate::error::AetherError> = runtime.block_on(async move {
            provider.process(&prompt, None).await
        });

        match ai_result {
            Ok(ref title) => {
                // Validate and clean the title
                let validated = title_generator::validate_title(title, &user_input);
                info!(
                    raw_title = %title.trim(),
                    validated_title = %validated,
                    "Topic title generated"
                );
                Ok(validated)
            }
            Err(e) => {
                // Log error and return default title
                tracing::warn!(error = %e, "Failed to generate topic title, using default");
                let default = title_generator::default_title(&user_input);
                Ok(default)
            }
        }
    }
}
