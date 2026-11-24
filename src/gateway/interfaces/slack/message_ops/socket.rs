use super::{
    Arc, ChannelId, Duration, InboundMessage, InboundMessageSender, RwLock, SlackConfig,
    SlackDebouncer, SlackMessageOps, UserDirectory, INITIAL_BACKOFF, MAX_BACKOFF,
};

async fn acknowledge_envelope<S>(sink: &mut S, envelope_id: &str) -> Result<(), S::Error>
where
    S: futures_util::Sink<tokio_tungstenite::tungstenite::Message> + Unpin,
{
    use futures_util::SinkExt;

    let ack = serde_json::json!({ "envelope_id": envelope_id });
    sink.send(tokio_tungstenite::tungstenite::Message::Text(
        ack.to_string().into(),
    ))
    .await
}

impl SlackMessageOps {
    pub async fn run_socket_mode_loop(
        client: reqwest::Client,
        app_token: String,
        bot_user_id: Arc<RwLock<Option<String>>>,
        channel_id: ChannelId,
        config: SlackConfig,
        inbound_tx: InboundMessageSender,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
        user_directory: Option<Arc<UserDirectory>>,
    ) {
        use futures_util::StreamExt;

        let mut backoff = INITIAL_BACKOFF;
        let mut debouncer = SlackDebouncer::new(config.debounce_ms, inbound_tx);

        loop {
            if *shutdown_rx.borrow() || shutdown_rx.has_changed().is_err() {
                break;
            }

            // Get a fresh WebSocket URL
            let ws_url = match Self::get_socket_mode_url(&client, &app_token).await {
                Ok(url) => url,
                Err(e) => {
                    tracing::warn!(
                        "Slack: failed to get WebSocket URL: {e}, retrying in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            tracing::info!("Connecting to Slack Socket Mode...");

            let ws_result = tokio_tungstenite::connect_async(&ws_url).await;
            let ws_stream = match ws_result {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::warn!(
                        "Slack WebSocket connection failed: {e}, retrying in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            // Reset backoff on successful connection
            backoff = INITIAL_BACKOFF;
            tracing::info!("Slack Socket Mode connected");

            let (mut ws_tx, mut ws_rx) = ws_stream.split();

            let mut flush_interval = tokio::time::interval(Duration::from_millis(50));

            let should_reconnect = 'inner: loop {
                tokio::select! {
                    _ = flush_interval.tick() => {
                        if debouncer.flush_expired().await {
                            break 'inner false;
                        }
                        continue;
                    }
                    msg = ws_rx.next() => {
                        if debouncer.flush_expired().await {
                            break 'inner false;
                        }

                        let msg = match msg {
                            Some(Ok(m)) => m,
                            Some(Err(e)) => {
                                tracing::warn!("Slack WebSocket error: {e}");
                                break 'inner true;
                            }
                            None => {
                                tracing::info!("Slack WebSocket closed");
                                break 'inner true;
                            }
                        };

                        let text = match msg {
                            tokio_tungstenite::tungstenite::Message::Text(t) => t,
                            tokio_tungstenite::tungstenite::Message::Close(_) => {
                                tracing::info!("Slack Socket Mode closed by server");
                                break 'inner true;
                            }
                            _ => continue,
                        };

                        let payload: serde_json::Value = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("Slack: failed to parse message: {e}");
                                continue;
                            }
                        };

                        let envelope_type = payload["type"].as_str().unwrap_or("");

                        match envelope_type {
                            "hello" => {
                                tracing::debug!("Slack Socket Mode hello received");
                            }

                            "events_api" => {
                                let envelope_id = payload["envelope_id"].as_str().unwrap_or("");
                                if !envelope_id.is_empty() {
                                    if let Err(e) = acknowledge_envelope(&mut ws_tx, envelope_id).await {
                                        tracing::error!("Slack: failed to send ack: {e}");
                                        break 'inner true;
                                    }
                                }

                                let event = &payload["payload"]["event"];
                                let bot_id_guard = bot_user_id.read().await;
                                let bot_id_str = bot_id_guard.as_deref().unwrap_or("");
                                let event_type = event["type"].as_str().unwrap_or("");

                                let inbound = match event_type {
                                    "message" => Self::convert_event_to_inbound(
                                        event,
                                        &channel_id,
                                        bot_id_str,
                                        &config,
                                    ),
                                    "app_mention" => Self::convert_app_mention_to_inbound(
                                        event,
                                        &channel_id,
                                        bot_id_str,
                                        &config,
                                    ),
                                    _ => None,
                                };

                                if let Some(mut inbound) = inbound {
                                    // Extract + download file attachments. Slack
                                    // `url_private` needs the bot token in an auth
                                    // header, which the generic media pipeline
                                    // cannot supply, so the limb fetches its own
                                    // files here (see `files` module).
                                    if config.media_max_mb > 0 {
                                        let slack_files = super::files::parse_files(event);
                                        if !slack_files.is_empty() {
                                            let max_bytes =
                                                config.media_max_mb.saturating_mul(1024 * 1024);
                                            inbound.attachments = super::files::fetch_attachments(
                                                &client,
                                                &config.bot_token,
                                                slack_files,
                                                max_bytes,
                                            )
                                            .await;
                                        }
                                    }

                                    let resolved_inbound = if config.resolve_user_names {
                                        if let Some(ref dir) = user_directory {
                                            if let Some(name) =
                                                dir.resolve(inbound.sender_id.as_str()).await
                                            {
                                                InboundMessage {
                                                    sender_name: Some(name),
                                                    ..inbound
                                                }
                                            } else {
                                                inbound
                                            }
                                        } else {
                                            inbound
                                        }
                                    } else {
                                        inbound
                                    };

                                    tracing::debug!(
                                        "Slack {} from {}: {}",
                                        event_type,
                                        resolved_inbound.sender_id.as_str(),
                                        resolved_inbound.text.chars().take(50).collect::<String>()
                                    );
                                    let reply_to = resolved_inbound.reply_to.clone();
                                    let thread_ts = reply_to.as_ref().map(|id| id.as_str());
                                    if debouncer.enqueue(resolved_inbound, thread_ts).await {
                                        tracing::error!("Slack: inbound channel closed");
                                        break 'inner false;
                                    }
                                }
                            }

                            "disconnect" => {
                                let reason = payload["reason"].as_str().unwrap_or("unknown");
                                tracing::info!("Slack disconnect request: {reason}");
                                break 'inner true;
                            }

                            _ => {
                                tracing::debug!("Slack envelope type: {envelope_type}");
                            }
                        }
                    }
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break 'inner false;
                        }
                    }
                };
            };

            if !should_reconnect || *shutdown_rx.borrow() {
                break;
            }

            tracing::warn!("Slack: reconnecting in {backoff:?}");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }

        if debouncer.drain().await {
            tracing::error!("Slack: shutdown drain could not forward all accepted messages");
        }
        tracing::info!("Slack Socket Mode loop stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn events_api_ack_preserves_envelope_id_and_reports_write_failure() {
        use std::convert::Infallible;
        use tokio_tungstenite::tungstenite::Message;

        let mut frames = Vec::new();
        {
            let sink = futures_util::sink::unfold(&mut frames, |frames, msg| {
                frames.push(msg);
                std::future::ready(Ok::<_, Infallible>(frames))
            });
            let mut sink = std::pin::pin!(sink);
            acknowledge_envelope(&mut sink, "envelope-42")
                .await
                .unwrap();
        }
        assert_eq!(frames.len(), 1);
        let Message::Text(text) = &frames[0] else {
            panic!("ACK must be a text frame");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(text).unwrap(),
            serde_json::json!({"envelope_id": "envelope-42"})
        );

        let sink = futures_util::sink::unfold((), |(), _: Message| {
            std::future::ready(Err::<(), _>("closed"))
        });
        let mut sink = std::pin::pin!(sink);
        assert_eq!(
            acknowledge_envelope(&mut sink, "envelope-42").await,
            Err("closed")
        );
    }
}
