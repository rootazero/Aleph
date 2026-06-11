use super::{SlackMessageOps, Arc, RwLock, ChannelId, SlackConfig, InboundMessageSender, UserDirectory, INITIAL_BACKOFF, SlackDebouncer, MAX_BACKOFF, Duration, InboundMessage};

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
        use futures_util::{SinkExt, StreamExt};

        let mut backoff = INITIAL_BACKOFF;
        let mut debouncer = SlackDebouncer::new(config.debounce_ms, inbound_tx);

        loop {
            if *shutdown_rx.borrow() {
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
                        debouncer.flush_expired().await;
                        continue;
                    }
                    msg = ws_rx.next() => {
                        debouncer.flush_expired().await;

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
                                    let ack = serde_json::json!({ "envelope_id": envelope_id });
                                    if let Err(e) = ws_tx
                                        .send(tokio_tungstenite::tungstenite::Message::Text(
                                            serde_json::to_string(&ack).unwrap().into(),
                                        ))
                                        .await
                                    {
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
                                        return;
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
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            let _ = ws_tx.close().await;
                            return;
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

        tracing::info!("Slack Socket Mode loop stopped");
    }
}
