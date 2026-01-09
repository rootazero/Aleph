//! Conversation management for AetherCore
//!
//! This module contains all multi-turn conversation methods:
//! - Starting and ending conversations
//! - Continuing conversations with follow-up input
//! - Conversation state queries

use super::types::CapturedContext;
use super::AetherCore;
use crate::error::AetherException;
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
    /// 2. Processes the follow-up with AI
    /// 3. Adds the turn to history
    /// 4. Returns the AI response (for printing to target window)
    ///
    /// # Arguments
    /// * `follow_up_input` - The user's follow-up message
    ///
    /// # Returns
    /// * `Result<String>` - The AI's response, or an error
    pub fn continue_conversation(
        &self,
        follow_up_input: String,
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
        let (context_prompt, context, turn_count) = {
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

        info!(
            input_preview = %follow_up_input.chars().take(50).collect::<String>(),
            turn_count = turn_count,
            "Continuing conversation"
        );

        // Store context for memory operations (required for memory storage)
        self.set_current_context(context.clone());

        // Build augmented input with conversation history
        let augmented_input = if context_prompt.is_empty() {
            follow_up_input.clone()
        } else {
            format!("{}\n\n当前问题: {}", context_prompt, follow_up_input)
        };

        // Process with AI using AI-first mode
        let start_time = std::time::Instant::now();
        let response =
            match self.process_with_ai_first(augmented_input.clone(), context.clone(), start_time)
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
}
