use crate::gateway::channel::{ChannelResult, InboundMessage};
use crate::gateway::event_emitter::StreamEvent;
use crate::gateway::interfaces::telegram::config_v2::StreamingOptions;
use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
use crate::sync_primitives::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing;

use super::lane_handle::LaneHandle;
use super::lane_tracker::{LaneDeliveryTracker, LaneId};
use super::reasoning_extractor::ReasoningExtractor;
use super::status_controller::StatusReactionController;

/// Orchestrates multiple streaming lanes for a single Telegram conversation.
pub struct StreamOrchestrator {
    delivery: TelegramDelivery,
    tracker: Arc<Mutex<LaneDeliveryTracker>>,
    status_controller: StatusReactionController,
    config: StreamingOptions,
    event_rx: mpsc::Receiver<StreamEvent>,
    reasoning_extractor: Option<ReasoningExtractor>,
    /// This run answers a `/btw` side question, so the answer lane's settled
    /// text carries the side-answer marker.
    ///
    /// The orchestrator owns text delivery on this channel — no `StreamEvent`
    /// is ever forwarded to the `ReplyEmitter` that rides along for media — so
    /// this face cannot inherit the marker from the base emitter's outbound
    /// chokepoint and has to apply it at its own settle point. The value is
    /// read off that same `ReplyEmitter` at construction, so there is still one
    /// `ReplyEmitterConfig` per run answering the question once.
    side_answer: bool,
}

impl StreamOrchestrator {
    #[must_use]
    pub fn new(
        delivery: TelegramDelivery,
        config: StreamingOptions,
        side_answer: bool,
    ) -> (Self, mpsc::Sender<StreamEvent>) {
        let (event_tx, event_rx) = mpsc::channel(config.buffer_size);
        // `conversation_id` is built internally from numeric Telegram chat ids,
        // so a parse failure is not expected — but a spawned orchestrator task
        // must never panic on it (P7 defensive design). Fall back to a null chat
        // id and log: the downstream `sendMessage`/`editMessageText` will fail
        // and be handled by the delivery error path rather than killing the task.
        let (chat_id, thread_id) =
            crate::gateway::interfaces::telegram::delivery::parse_conversation_id(
                &delivery.conversation_id,
            )
            .unwrap_or_else(|_| {
                tracing::error!(
                    conversation_id = %delivery.conversation_id,
                    "StreamOrchestrator: unparseable conversation_id; streaming degraded for this run"
                );
                (teloxide::types::ChatId(0), None)
            });
        let tracker = Arc::new(Mutex::new(LaneDeliveryTracker::new(
            chat_id.0,
            thread_id.map(i64::from),
        )));
        let status_controller =
            StatusReactionController::new(delivery.clone(), config.status_reactions.clone());
        let reasoning_extractor = if config.reasoning_lane_enabled {
            Some(ReasoningExtractor::new())
        } else {
            None
        };

        (
            Self {
                delivery,
                tracker,
                status_controller,
                config,
                event_rx,
                reasoning_extractor,
                side_answer,
            },
            event_tx,
        )
    }

    #[must_use]
    pub fn get_lane(&self, lane_id: LaneId) -> LaneHandle {
        LaneHandle::new(lane_id, self.tracker.clone(), self.delivery.clone())
    }

    /// Main event loop. Receives `StreamEvents` and routes them to lanes.
    pub async fn run(mut self, initial_inbound: InboundMessage) -> ChannelResult<()> {
        let answer_lane = self.get_lane(LaneId::Answer);
        let inbound_message_id: Option<i64> = initial_inbound.id.as_str().parse().ok();

        while let Some(event) = self.event_rx.recv().await {
            tracing::debug!("StreamOrchestrator received event: {:?}", event);

            if let Some(msg_id) = inbound_message_id {
                if let Err(e) = self.status_controller.handle_event(&event, msg_id).await {
                    tracing::debug!("Status reaction update failed (non-critical): {}", e);
                }
            }

            match &event {
                StreamEvent::ResponseChunk { delta, .. } => {
                    if let Some(ref mut extractor) = self.reasoning_extractor {
                        let (reasoning, answer, _) = extractor.process_chunk(delta);
                        if let Some(r) = reasoning {
                            let reasoning_lane = self.get_lane(LaneId::Reasoning);
                            if let Err(e) = reasoning_lane.write_chunk(&r).await {
                                tracing::warn!("Failed to write reasoning chunk: {}", e);
                            }
                        }
                        if let Some(a) = answer {
                            if let Err(e) = answer_lane.write_chunk(&a).await {
                                tracing::warn!("Failed to write answer chunk: {}", e);
                            }
                        }
                    } else if let Err(e) = answer_lane.write_chunk(delta).await {
                        tracing::warn!("Failed to write answer chunk: {}", e);
                    }
                }
                StreamEvent::ReasoningBlock { content, .. } => {
                    if self.config.reasoning_lane_enabled {
                        let reasoning_lane = self.get_lane(LaneId::Reasoning);
                        if let Err(e) = reasoning_lane.write_chunk(content).await {
                            tracing::warn!("Failed to write reasoning chunk: {}", e);
                        }
                    }
                }
                StreamEvent::ToolStart { tool_name, .. } => {
                    tracing::info!(tool = %tool_name, "Tool started");
                }
                StreamEvent::ToolEnd { .. } => {
                    tracing::info!("Tool ended");
                }
                StreamEvent::RunComplete { summary, .. } => {
                    // Finalize the answer lane with the *deliverable* text: strip
                    // raw `<think>`/completion markers the model embedded in
                    // `final_response` via the same single-source sanitizer the
                    // inbound `ReplyEmitter` uses, so the final Telegram edit
                    // matches the reasoning-split streamed preview rather than
                    // leaking the raw assistant text.
                    //
                    // `None` means the sanitizer found nothing deliverable — a
                    // pure-reasoning turn. This used to fall back to the RAW text
                    // in that case, i.e. it posted the chain-of-thought precisely
                    // when sanitisation had said not to. Settle on what was
                    // already streamed instead: it is clean, and the promise that
                    // a pure-reasoning turn never blanks an existing message is
                    // kept without delivering what the sanitiser refused.
                    //
                    // The side-answer marker goes on AFTER that sanitizer and
                    // only in the `Some` arm — this is an edit-based lane, so
                    // the badge arrives atomically in the settling edit rather
                    // than on each streamed chunk. `finalize_streamed` settles
                    // on bytes already on screen and has no text to rewrite; a
                    // pure-reasoning side question therefore settles unmarked,
                    // which is the same thing that arm already promises for
                    // every other rewrite.
                    let raw = summary.final_response.as_deref().unwrap_or("");
                    let outcome = match crate::gateway::reply_emitter::sanitize_final_response(raw)
                    {
                        Some(text) if self.side_answer => {
                            answer_lane
                                .finalize(&crate::gateway::btw::format_side_answer(&text))
                                .await
                        }
                        Some(text) => answer_lane.finalize(&text).await,
                        None => answer_lane.finalize_streamed().await,
                    };
                    if let Err(e) = outcome {
                        tracing::warn!("Failed to finalize answer lane: {}", e);
                    }
                    break;
                }
                StreamEvent::RunError { error, .. } => {
                    tracing::warn!("Run error: {}", error);
                    let error_msg = format!("Error: {error}");
                    let _ = answer_lane.write_chunk(&error_msg).await;
                    let _ = answer_lane.finalize(&error_msg).await;
                    break;
                }
                _ => {
                    tracing::debug!("Ignoring event in orchestrator: {:?}", event);
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::telegram::config_resolver::ResolvedConfig;
    use crate::gateway::interfaces::telegram::error_cooldown::ErrorCooldown;

    #[tokio::test]
    async fn test_orchestrator_creation() {
        let delivery = TelegramDelivery::new(
            teloxide::Bot::new("test"),
            ResolvedConfig {
                account_id: "test".to_string(),
                bot_token: "test".to_string(),
                bot_username: None,
                default_agent: None,
                dm_policy: Default::default(),
                group_policy: Default::default(),
                send_typing: false,
                allowed_users: vec![],
                allowed_groups: vec![],
                streaming: Default::default(),
                error_policy: Default::default(),
                max_retries: 0,
                html_fallback: true,
                link_preview:
                    crate::gateway::interfaces::telegram::config_v2::LinkPreviewMode::Enabled,
            },
            Arc::new(ErrorCooldown::new()),
            "123",
        );
        let config = StreamingOptions::default();
        let (orchestrator, _tx) = StreamOrchestrator::new(delivery, config, false);
        let handle = orchestrator.get_lane(LaneId::Answer);
        assert!(matches!(handle, LaneHandle { .. }));
    }

    #[tokio::test]
    async fn test_event_routing_to_lanes() {
        let delivery = TelegramDelivery::new(
            teloxide::Bot::new("test"),
            ResolvedConfig {
                account_id: "test".to_string(),
                bot_token: "test".to_string(),
                bot_username: None,
                default_agent: None,
                dm_policy: Default::default(),
                group_policy: Default::default(),
                send_typing: false,
                allowed_users: vec![],
                allowed_groups: vec![],
                streaming: Default::default(),
                error_policy: Default::default(),
                max_retries: 0,
                html_fallback: true,
                link_preview:
                    crate::gateway::interfaces::telegram::config_v2::LinkPreviewMode::Enabled,
            },
            Arc::new(ErrorCooldown::new()),
            "123",
        );
        let config = StreamingOptions {
            reasoning_lane_enabled: true,
            ..Default::default()
        };
        let (orchestrator, tx) = StreamOrchestrator::new(delivery, config, false);

        let inbound = InboundMessage {
            id: crate::gateway::channel::MessageId::new("1"),
            conversation_id: crate::gateway::channel::ConversationId::new("123"),
            channel_id: crate::gateway::channel::ChannelId::new("telegram"),
            sender_id: crate::gateway::channel::UserId::new("user"),
            sender_name: None,
            text: "hello".to_string(),
            timestamp: chrono::Utc::now(),
            attachments: vec![],
            metadata: Default::default(),
            reply_to: None,
            is_group: false,
            raw: None,
        };

        tokio::spawn(orchestrator.run(inbound));

        tx.send(StreamEvent::ResponseChunk {
            run_id: "r1".to_string(),
            seq: 1,
            delta: "hi".to_string(),
            full_text: "hi".to_string(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        })
        .await
        .unwrap();

        tx.send(StreamEvent::RunComplete {
            run_id: "r1".to_string(),
            seq: 2,
            summary: crate::gateway::event_emitter::RunSummary {
                total_tokens: 1,
                tool_calls: 0,
                loops: 1,
                final_response: Some("hi".to_string()),
                ..Default::default()
            },
            total_duration_ms: 100,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_reasoning_extraction_in_orchestrator() {
        let delivery = TelegramDelivery::new(
            teloxide::Bot::new("test"),
            ResolvedConfig {
                account_id: "test".to_string(),
                bot_token: "test".to_string(),
                bot_username: None,
                default_agent: None,
                dm_policy: Default::default(),
                group_policy: Default::default(),
                send_typing: false,
                allowed_users: vec![],
                allowed_groups: vec![],
                streaming: Default::default(),
                error_policy: Default::default(),
                max_retries: 0,
                html_fallback: true,
                link_preview:
                    crate::gateway::interfaces::telegram::config_v2::LinkPreviewMode::Enabled,
            },
            Arc::new(ErrorCooldown::new()),
            "123",
        );
        let config = StreamingOptions {
            reasoning_lane_enabled: true,
            ..Default::default()
        };
        let (orchestrator, tx) = StreamOrchestrator::new(delivery, config, false);

        let inbound = InboundMessage {
            id: crate::gateway::channel::MessageId::new("1"),
            conversation_id: crate::gateway::channel::ConversationId::new("123"),
            channel_id: crate::gateway::channel::ChannelId::new("telegram"),
            sender_id: crate::gateway::channel::UserId::new("user"),
            sender_name: None,
            text: "hello".to_string(),
            timestamp: chrono::Utc::now(),
            attachments: vec![],
            metadata: Default::default(),
            reply_to: None,
            is_group: false,
            raw: None,
        };

        tokio::spawn(orchestrator.run(inbound));

        tx.send(StreamEvent::ResponseChunk {
            run_id: "r1".to_string(),
            seq: 1,
            delta: "<think>step 1</think> answer".to_string(),
            full_text: "<think>step 1</think> answer".to_string(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        })
        .await
        .unwrap();

        tx.send(StreamEvent::RunComplete {
            run_id: "r1".to_string(),
            seq: 2,
            summary: crate::gateway::event_emitter::RunSummary {
                total_tokens: 1,
                tool_calls: 0,
                loops: 1,
                final_response: Some("<think>step 1</think> answer".to_string()),
                ..Default::default()
            },
            total_duration_ms: 100,
        })
        .await
        .unwrap();
    }
}
