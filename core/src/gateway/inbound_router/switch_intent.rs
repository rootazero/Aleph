//! Natural language switch intent detection

use tracing::{error, info, warn};

use crate::gateway::channel::{InboundMessage, OutboundMessage};
use crate::gateway::intent_detector::{DetectedIntent, build_soul_generation_prompt};

use super::types::{RoutingError, check_link_access};
use super::InboundMessageRouter;
use crate::providers::message::UnifiedMessage;
use crate::providers::adapter::RequestPayload;

impl InboundMessageRouter {
    /// Try to handle a switch intent from the message.
    /// Returns Some(Ok(())) if handled (message consumed), None if not a switch intent.
    pub(super) async fn try_handle_switch_intent(
        &self,
        msg: &InboundMessage,
    ) -> Option<Result<(), RoutingError>> {
        let detector = self.intent_detector.as_ref()?;
        let manager = self.workspace_manager.as_ref()?;
        let registry = self.agent_registry.as_ref()?;

        let mut intent = detector.detect(&msg.text).await;

        // If LLM returned an id, try to resolve it against registered agents
        if let DetectedIntent::SwitchAgent { ref id, ref name, .. } = intent {
            if id.is_empty() {
                // LLM didn't provide an id — try name match against registered agents
                if let Some(matched_id) = registry.find_by_name(name).await {
                    info!("[Router] Resolved agent by name match: '{}' -> '{}'", name, matched_id);
                    let task = if let DetectedIntent::SwitchAgent { task, .. } = &intent { task.clone() } else { None };
                    intent = DetectedIntent::SwitchAgent {
                        id: matched_id,
                        name: name.clone(),
                        task,
                    };
                }
            }
        }

        match intent {
            DetectedIntent::SwitchAgent { ref id, ref name, ref task } if !id.is_empty() => {
                let channel_id = msg.channel_id.as_str();
                let sender_id = msg.sender_id.as_str();

                // Create agent dynamically if it doesn't exist
                if registry.get(id).await.is_none() {
                    info!("[Router] Agent '{}' not found, creating dynamically", id);

                    let soul_content = if let Some(ref provider) = self.llm_provider {
                        let prompt = build_soul_generation_prompt(id, name);
                        let soul_msgs = [UnifiedMessage::user(&prompt)];
                        match provider.process(RequestPayload::new(&soul_msgs)).await {
                            Ok(resp) => resp.text_content(),
                            Err(e) => {
                                warn!("[Router] Failed to generate soul: {}, using default", e);
                                format!("You are {}, an AI assistant.", name)
                            }
                        }
                    } else {
                        format!("You are {}, an AI assistant.", name)
                    };

                    if let Err(e) = registry.create_dynamic(id, &soul_content, None).await {
                        let reply = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("Failed to create agent '{}': {}", id, e),
                        );
                        let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                        return Some(Ok(()));
                    }
                }

                // Check link access control before switching
                if let Some(allowed_links) = registry.get_allowed_links(id).await {
                    if let Err(e) = check_link_access(&allowed_links, channel_id, id) {
                        let reply = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("\u{26d4} {}", e),
                        );
                        let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                        return Some(Ok(()));
                    }
                }

                // Switch active agent
                let switch_ok = match manager.set_active_agent(channel_id, sender_id, id) {
                    Ok(()) => {
                        info!("[Router] Switched agent for {}:{} -> {} ({})", channel_id, sender_id, id, name);
                        let reply = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("✅ Switched to {} ({})", name, id),
                        );
                        if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
                            error!("[Router] Failed to send switch reply: {}", e);
                        }
                        true
                    }
                    Err(e) => {
                        error!("[Router] Failed to switch agent: {}", e);
                        let reply = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("❌ Failed to switch: {}", e),
                        );
                        let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                        false
                    }
                };

                // If switch succeeded and there's a trailing task, forward it to the new agent
                if switch_ok {
                    if let Some(task_text) = task {
                        if !task_text.is_empty() {
                            info!("[Router] Forwarding task to agent '{}': {}", id, task_text);
                            let mut task_msg = msg.clone();
                            task_msg.text = task_text.clone();
                            let ctx = self.build_context_with_agent(&task_msg, id);
                            if let Err(e) = self.execute_for_context(&ctx).await {
                                error!("[Router] Failed to execute forwarded task: {}", e);
                            }
                        }
                    }
                }

                Some(Ok(()))
            }
            _ => None,
        }
    }
}
