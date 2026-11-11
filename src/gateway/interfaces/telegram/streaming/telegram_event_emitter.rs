use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
use crate::gateway::reply_emitter::ReplyEmitter;
use crate::sync_primitives::{Arc, Ordering};
use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};

use super::StreamOrchestrator;

/// Owns streaming text delivery and a media-only reply leg for one run.
pub struct TelegramEventEmitter {
    event_tx: mpsc::Sender<StreamEvent>,
    completion: Mutex<Option<tokio::task::JoinHandle<crate::gateway::channel::ChannelResult<()>>>>,
    seq_counter: Arc<crate::sync_primitives::AtomicU64>,
    media: ReplyEmitter,
}

impl TelegramEventEmitter {
    #[must_use]
    pub fn new(
        delivery: TelegramDelivery,
        route: crate::gateway::inbound_context::ReplyRoute,
        media: ReplyEmitter,
    ) -> Self {
        let streaming = delivery.config.streaming.clone();
        let (orchestrator, event_tx) =
            StreamOrchestrator::new(delivery, streaming, media.is_side_answer());
        let inbound_message_id = route
            .inbound_message_id
            .as_ref()
            .and_then(|id| id.as_str().parse().ok());
        let completion = tokio::spawn(orchestrator.run(inbound_message_id));
        Self {
            event_tx,
            completion: Mutex::new(Some(completion)),
            seq_counter: Arc::new(crate::sync_primitives::AtomicU64::new(0)),
            media,
        }
    }
}

#[async_trait]
impl EventEmitter for TelegramEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        let run_ended = matches!(event, StreamEvent::RunComplete { .. });
        let terminal = run_ended || matches!(event, StreamEvent::RunError { .. });
        let forwarded = self
            .event_tx
            .send(event)
            .await
            .map_err(|_| EventEmitError::ChannelClosed);
        if !terminal {
            return forwarded;
        }
        let task = self
            .completion
            .lock()
            .await
            .take()
            .ok_or(EventEmitError::ChannelClosed)?;
        let completion = match task.await {
            Ok(result) => result.map_err(|error| {
                EventEmitError::EventBus(format!("Telegram delivery failed: {error}"))
            }),
            Err(error) => Err(EventEmitError::EventBus(format!(
                "Telegram delivery task failed: {error}"
            ))),
        };
        // Wait for final text before the single media drain, even on text failure.
        // No event is forwarded to the media leg, so it cannot duplicate text.
        if run_ended {
            self.media.deliver_run_media().await;
        }
        completion.and(forwarded)
    }
    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::RunSummary;
    use crate::gateway::interfaces::telegram::config_resolver::ResolvedConfig;
    use crate::gateway::interfaces::telegram::config_v2::{
        DmPolicy, ErrorPolicy, GroupPolicy, LinkPreviewMode, StreamingOptions,
    };
    use crate::gateway::interfaces::telegram::error_cooldown::ErrorCooldown;
    use crate::gateway::media::PendingMedia;
    use serde_json::json;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn route() -> crate::gateway::inbound_context::ReplyRoute {
        crate::gateway::inbound_context::ReplyRoute::new(
            crate::gateway::channel::ChannelId::new("telegram"),
            crate::gateway::channel::ConversationId::new("123"),
        )
    }

    fn config() -> ResolvedConfig {
        ResolvedConfig {
            account_id: "acct".to_string(),
            bot_token: "123:test".to_string(),
            bot_username: None,
            default_agent: None,
            dm_policy: DmPolicy::default(),
            group_policy: GroupPolicy::default(),
            send_typing: false,
            allowed_users: Vec::new(),
            allowed_groups: Vec::new(),
            streaming: StreamingOptions::default(),
            error_policy: ErrorPolicy::default(),
            max_retries: 0,
            html_fallback: true,
            link_preview: LinkPreviewMode::Disabled,
        }
    }

    /// Telegram's `sendMessage` success envelope.
    fn message_response() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": {"message_id": 1001, "date": 1_700_000_000,
                "chat": {"id": 123, "type": "private"}}
        }))
    }

    fn complete_with_text(text: &str) -> StreamEvent {
        StreamEvent::RunComplete {
            run_id: "run".to_string(),
            seq: 2,
            summary: RunSummary {
                final_response: Some(text.to_string()),
                ..RunSummary::default()
            },
            total_duration_ms: 1,
        }
    }

    fn emitter_with_bot(
        bot: teloxide::Bot,
        config: ResolvedConfig,
        route: crate::gateway::inbound_context::ReplyRoute,
        pending: PendingMedia,
    ) -> TelegramEventEmitter {
        let media = ReplyEmitter::new(
            Arc::new(crate::gateway::channel_registry::ChannelRegistry::new()),
            route.clone(),
            "run".to_string(),
            pending,
        );
        TelegramEventEmitter::new(
            TelegramDelivery::new(bot, config, Arc::new(ErrorCooldown::new()), "123"),
            route,
            media,
        )
    }

    fn emitter_with(_run_id: &str, pending: PendingMedia) -> TelegramEventEmitter {
        emitter_with_bot(teloxide::Bot::new("test"), config(), route(), pending)
    }

    fn run_complete(run_id: &str) -> StreamEvent {
        StreamEvent::RunComplete {
            run_id: run_id.to_string(),
            seq: 2,
            summary: RunSummary::default(),
            total_duration_ms: 1,
        }
    }
    #[tokio::test]
    async fn run_complete_drains_the_runs_media_buffer() {
        let pending: PendingMedia = Arc::new(Mutex::new(vec![
            crate::gateway::media::resolved_test_attachment(),
        ]));
        let emitter = emitter_with("run", pending.clone());
        emitter.emit(run_complete("run")).await.unwrap();
        assert!(pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn run_complete_without_media_is_a_no_op() {
        let emitter = emitter_with("run", PendingMedia::default());
        emitter.emit(run_complete("run")).await.unwrap();
    }

    #[tokio::test]
    async fn run_error_drains_pending_media_before_terminating() {
        let pending: PendingMedia = Arc::new(Mutex::new(vec![
            crate::gateway::media::resolved_test_attachment(),
        ]));
        let emitter = emitter_with("run", pending.clone());
        emitter
            .emit(StreamEvent::RunError {
                run_id: "run".into(),
                seq: 3,
                error: "boom".into(),
                error_code: None,
                session_key: None,
            })
            .await
            .unwrap();
        assert!(pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_telegram_event_emitter_creation() {
        let emitter = emitter_with("run", PendingMedia::default());
        emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: "run".into(),
                seq: 1,
                delta: "hi".into(),
                full_text: "hi".into(),
                chunk_index: 0,
                is_final: false,
                is_intermediate: false,
            })
            .await
            .unwrap();
        assert_eq!(emitter.next_seq(), 0);
        assert_eq!(emitter.next_seq(), 1);
    }

    #[tokio::test]
    async fn terminal_delivery_error_reaches_the_emitter_caller() {
        let server = MockServer::start().await;
        Mock::given(path("/bot123:test/SendMessage"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "ok": false, "error_code": 400, "description": "Bad Request: rejected"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let bot = teloxide::Bot::new("123:test").set_api_url(server.uri().parse().unwrap());
        let emitter = emitter_with_bot(bot, config(), route(), PendingMedia::default());
        let result = emitter.emit(complete_with_text("answer")).await;
        assert!(
            matches!(result, Err(EventEmitError::EventBus(message)) if message.contains("Telegram delivery failed"))
        );
    }

    #[tokio::test]
    async fn final_text_settles_before_media_and_keeps_disabled_link_previews() {
        let server = MockServer::start().await;
        Mock::given(path("/bot123:test/SendMessage"))
            .respond_with(message_response().set_delay(std::time::Duration::from_millis(200)))
            .expect(1)
            .mount(&server)
            .await;
        let pending: PendingMedia = Arc::new(Mutex::new(vec![
            crate::gateway::media::resolved_test_attachment(),
        ]));
        let bot = teloxide::Bot::new("123:test").set_api_url(server.uri().parse().unwrap());
        let emitter = emitter_with_bot(bot, config(), route(), pending.clone());
        let task = tokio::spawn(async move {
            emitter
                .emit(complete_with_text("https://example.com"))
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !server.received_requests().await.unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !task.is_finished(),
            "emit must wait for final delivery, not just enqueue"
        );
        assert_eq!(
            pending.lock().await.len(),
            1,
            "media cannot overtake the final text"
        );
        task.await.unwrap().unwrap();
        assert!(pending.lock().await.is_empty());
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["link_preview_options"]["is_disabled"], true);
    }

    #[tokio::test]
    async fn reactions_use_real_inbound_id_and_skip_absent_ids() {
        for inbound_id in [Some("987"), None] {
            let server = MockServer::start().await;
            Mock::given(path("/bot123:test/SetMessageReaction"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": true})),
                )
                .expect(if inbound_id.is_some() { 1 } else { 0 })
                .mount(&server)
                .await;
            let mut config = config();
            config.streaming.status_reactions.complete = Some("ok".into());
            let mut route = route();
            route.inbound_message_id = inbound_id.map(crate::gateway::channel::MessageId::new);
            let bot = teloxide::Bot::new("123:test").set_api_url(server.uri().parse().unwrap());
            let emitter = emitter_with_bot(bot, config, route, PendingMedia::default());
            emitter.emit(run_complete("run")).await.unwrap();
            if inbound_id.is_some() {
                let requests = server.received_requests().await.unwrap();
                let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
                assert_eq!(body["message_id"], 987);
            }
        }
    }

    #[tokio::test]
    async fn short_reasoning_tail_is_flushed_only_when_opted_in() {
        for enabled in [true, false] {
            let server = MockServer::start().await;
            Mock::given(path("/bot123:test/SendMessage"))
                .respond_with(message_response())
                .expect(if enabled { 2 } else { 1 })
                .mount(&server)
                .await;
            let mut config = config();
            config.streaming.reasoning_lane_enabled = enabled;
            let bot = teloxide::Bot::new("123:test").set_api_url(server.uri().parse().unwrap());
            let emitter = emitter_with_bot(bot, config, route(), PendingMedia::default());
            emitter
                .emit(StreamEvent::Reasoning {
                    run_id: "run".into(),
                    seq: 1,
                    content: "tiny".into(),
                    is_complete: false,
                })
                .await
                .unwrap();
            emitter
                .emit(complete_with_text("<think>secret</think>answer"))
                .await
                .unwrap();
            let requests = server.received_requests().await.unwrap();
            let texts: Vec<String> = requests
                .iter()
                .map(|request| {
                    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                    body["text"].as_str().unwrap().to_string()
                })
                .collect();
            assert!(!texts.iter().any(|text| text.contains("secret")));
            assert_eq!(
                texts.iter().filter(|text| text.contains("tiny")).count(),
                usize::from(enabled)
            );
            if enabled {
                assert!(texts[0].starts_with("🧠 "));
            }
        }
    }
}
