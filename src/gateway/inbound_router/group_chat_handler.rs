//! Group chat command and session handling

use crate::sync_primitives::Arc;
use tracing::{error, info};

use crate::gateway::channel::{InboundMessage, OutboundMessage};
use crate::gateway::handlers::group_chat::SharedOrchestrator;
use crate::group_chat::{
    DefaultGroupChatCommandParser, GroupChatExecutor, GroupChatRequest,
    GroupChatStatus,
};

use super::types::RoutingError;
use super::InboundMessageRouter;

impl InboundMessageRouter {
    /// Handle /groupchat command: dispatch to group chat orchestrator
    pub(super) async fn handle_groupchat_command(
        &self,
        msg: &InboundMessage,
    ) -> Result<(), RoutingError> {
        if self.group_chat_orch.is_some() {
            if let Some(handled) = self.try_handle_group_chat(msg).await {
                match handled {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        let error_msg = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("Group chat error: {e}"),
                        );
                        let _ = self.channel_registry.send(&msg.channel_id, error_msg).await;
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// Try to handle the message as a group chat command or active session message.
    ///
    /// Returns:
    /// - `Some(Ok(()))` if the message was handled by group chat
    /// - `Some(Err(...))` if group chat handling failed
    /// - `None` if the message is not group-chat-related (proceed to normal agent)
    pub(super) async fn try_handle_group_chat(
        &self,
        msg: &InboundMessage,
    ) -> Option<Result<(), String>> {
        let orch = self.group_chat_orch.as_ref()?;
        let executor = self.group_chat_executor.as_ref()?;

        let conversation_key = format!(
            "{}:{}",
            msg.channel_id.as_str(),
            msg.conversation_id.as_str()
        );
        let parser = DefaultGroupChatCommandParser;

        // 1. Check for /groupchat command
        if let Some(request) = parser.parse_group_chat_command(&msg.text) {
            match request {
                GroupChatRequest::Start {
                    personas,
                    topic,
                    initial_message,
                } => {
                    return Some(
                        self.handle_group_chat_start(
                            orch,
                            executor,
                            msg,
                            &conversation_key,
                            personas,
                            topic,
                            initial_message,
                        )
                        .await,
                    );
                }
                GroupChatRequest::End { session_id } => {
                    return Some(
                        self.handle_group_chat_end(orch, msg, &conversation_key, &session_id)
                            .await,
                    );
                }
                GroupChatRequest::Continue {
                    session_id,
                    message,
                } => {
                    return Some(
                        self.handle_group_chat_continue(
                            orch,
                            executor,
                            msg,
                            &session_id,
                            &message,
                            &[],
                        )
                        .await,
                    );
                }
                GroupChatRequest::Mention {
                    session_id,
                    message,
                    targets,
                } => {
                    return Some(
                        self.handle_group_chat_continue(
                            orch,
                            executor,
                            msg,
                            &session_id,
                            &message,
                            &targets,
                        )
                        .await,
                    );
                }
            }
        }

        // 2. Check if conversation has an active group chat session
        let session_id = {
            let sessions = self.active_group_sessions.lock().await;
            sessions.get(&conversation_key).cloned()
        };

        if let Some(session_id) = session_id {
            // Verify session is still active
            let session_handle = {
                let orch_guard = orch.lock().await;
                orch_guard.get_session(&session_id)
            };

            if let Some(handle) = session_handle {
                let is_active = {
                    let session = handle.lock().await;
                    session.status == GroupChatStatus::Active
                };

                if is_active {
                    return Some(
                        self.handle_group_chat_continue(
                            orch,
                            executor,
                            msg,
                            &session_id,
                            &msg.text,
                            &[],
                        )
                        .await,
                    );
                } else {
                    // Session ended, clean up tracking
                    let mut sessions = self.active_group_sessions.lock().await;
                    sessions.remove(&conversation_key);
                }
            } else {
                // Session gone, clean up tracking
                let mut sessions = self.active_group_sessions.lock().await;
                sessions.remove(&conversation_key);
            }
        }

        // Not a group chat message
        None
    }

    /// Handle `/groupchat start` command
    #[allow(clippy::too_many_arguments)]
    async fn handle_group_chat_start(
        &self,
        orch: &SharedOrchestrator,
        executor: &Arc<GroupChatExecutor>,
        msg: &InboundMessage,
        conversation_key: &str,
        personas: Vec<crate::group_chat::PersonaSource>,
        topic: String,
        initial_message: String,
    ) -> Result<(), String> {
        let channel_id = msg.channel_id.as_str().to_string();
        let conversation_id = msg.conversation_id.as_str().to_string();

        // Create session via orchestrator
        let (session_id, session_handle) = {
            let mut orch_guard = orch.lock().await;
            orch_guard
                .create_session(
                    personas,
                    if topic.is_empty() {
                        None
                    } else {
                        Some(topic.clone())
                    },
                    channel_id.clone(),
                    conversation_key.to_string(),
                )
                .map_err(|e| e.to_string())?
        };

        // Track the active session for this conversation
        {
            let mut sessions = self.active_group_sessions.lock().await;
            sessions.insert(conversation_key.to_string(), session_id.clone());
        }

        // Send session start notification
        let participant_names: Vec<String> = {
            let session = session_handle.lock().await;
            session
                .participants
                .iter()
                .map(|p| p.name.clone())
                .collect()
        };
        let topic_line = if topic.is_empty() {
            String::new()
        } else {
            format!("\nTopic: {topic}")
        };
        let start_msg = format!(
            "🎭 Group chat started!\nParticipants: {}{}\n\nSend messages to continue, /groupchat end to finish.",
            participant_names.join(", "),
            topic_line,
        );
        let outbound = OutboundMessage::text(&conversation_id, start_msg);
        let _ = self.channel_registry.send(&msg.channel_id, outbound).await;

        // Execute first round if initial_message is not empty
        if !initial_message.is_empty() {
            let mut session = session_handle.lock().await;
            match executor
                .execute_round(&mut session, &initial_message, &[])
                .await
            {
                Ok(messages) => {
                    self.send_group_chat_messages(msg, &messages).await;
                }
                Err(e) => {
                    let err_msg = OutboundMessage::text(
                        &conversation_id,
                        format!("Round execution failed: {e}"),
                    );
                    let _ = self.channel_registry.send(&msg.channel_id, err_msg).await;
                }
            }
        }

        info!(
            subsystem = "group_chat",
            event = "session_started_via_channel",
            session_id = %session_id,
            channel = %channel_id,
            "group chat session started from channel"
        );

        Ok(())
    }

    /// Handle `/groupchat end` command
    async fn handle_group_chat_end(
        &self,
        orch: &SharedOrchestrator,
        msg: &InboundMessage,
        conversation_key: &str,
        session_id_hint: &str,
    ) -> Result<(), String> {
        // Determine which session to end: explicit session_id or the active one
        let session_id = if session_id_hint.is_empty() {
            let sessions = self.active_group_sessions.lock().await;
            sessions
                .get(conversation_key)
                .cloned()
                .ok_or_else(|| "No active group chat in this conversation".to_string())?
        } else {
            session_id_hint.to_string()
        };

        // End the session via orchestrator (removes from map + persists)
        let session_handle = {
            let mut orch_guard = orch.lock().await;
            orch_guard.end_session(&session_id)
        };

        if let Some(handle) = session_handle {
            let mut session = handle.lock().await;
            session.end();
        } else {
            return Err(format!("Session not found: {session_id}"));
        }

        // Remove from active tracking
        {
            let mut sessions = self.active_group_sessions.lock().await;
            sessions.remove(conversation_key);
        }

        // Send end notification
        let outbound = OutboundMessage::text(msg.conversation_id.as_str(), "🎭 Group chat ended.");
        let _ = self.channel_registry.send(&msg.channel_id, outbound).await;

        info!(
            subsystem = "group_chat",
            event = "session_ended_via_channel",
            session_id = %session_id,
            "group chat session ended from channel"
        );

        Ok(())
    }

    /// Handle a continuation message for an active group chat session
    async fn handle_group_chat_continue(
        &self,
        orch: &SharedOrchestrator,
        executor: &Arc<GroupChatExecutor>,
        msg: &InboundMessage,
        session_id: &str,
        user_message: &str,
        targets: &[String],
    ) -> Result<(), String> {
        let (session_handle, max_rounds) = {
            let orch_guard = orch.lock().await;
            let handle = orch_guard
                .get_session(session_id)
                .ok_or_else(|| format!("Session not found: {session_id}"))?;
            (handle, orch_guard.max_rounds())
        };

        let mut session = session_handle.lock().await;

        // Check round limit
        if session.current_round >= max_rounds {
            let conversation_key = format!(
                "{}:{}",
                msg.channel_id.as_str(),
                msg.conversation_id.as_str()
            );
            session.end();
            drop(session);

            // Remove from orchestrator and tracking
            {
                let mut orch_guard = orch.lock().await;
                orch_guard.end_session(session_id);
            }
            {
                let mut sessions = self.active_group_sessions.lock().await;
                sessions.remove(&conversation_key);
            }
            let outbound = OutboundMessage::text(
                msg.conversation_id.as_str(),
                format!("🎭 Group chat ended (max {max_rounds} rounds reached)."),
            );
            let _ = self.channel_registry.send(&msg.channel_id, outbound).await;
            return Ok(());
        }

        // Execute the round
        match executor
            .execute_round(&mut session, user_message, targets)
            .await
        {
            Ok(messages) => {
                drop(session);
                self.send_group_chat_messages(msg, &messages).await;
            }
            Err(e) => {
                let err_msg = OutboundMessage::text(
                    msg.conversation_id.as_str(),
                    format!("Round failed: {e}"),
                );
                let _ = self.channel_registry.send(&msg.channel_id, err_msg).await;
            }
        }

        Ok(())
    }

    /// Send group chat persona messages back to the channel
    pub(super) async fn send_group_chat_messages(
        &self,
        msg: &InboundMessage,
        messages: &[crate::group_chat::GroupChatMessage],
    ) {
        for gc_msg in messages {
            let text = format!("**[{}]**: {}", gc_msg.speaker.name(), gc_msg.content);
            let outbound = OutboundMessage::text(msg.conversation_id.as_str(), text);
            if let Err(e) = self.channel_registry.send(&msg.channel_id, outbound).await {
                error!("Failed to send group chat message: {}", e);
            }
        }
    }
}
