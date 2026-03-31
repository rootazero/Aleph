//! XMPP message operations (convert, send, connection loop).

use std::time::Duration;

use chrono::Utc;

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};

use super::super::config::XmppConfig;
use super::stanza::{
    build_auth_stanza, build_bind_stanza, build_message_stanza, build_muc_join_stanza,
    build_pong_stanza, build_presence_stanza, build_session_stanza, build_stream_close,
    build_stream_header, extract_ping, extract_stanza, is_auth_failure, is_auth_success,
    is_stream_features, parse_message_stanza,
};
use super::types::parse_jid;
use super::XMPP_MSG_LIMIT;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// XMPP message operations helper.
///
/// Provides methods for sending messages and running the XMPP connection loop.
pub struct XmppMessageOps;

impl XmppMessageOps {
    /// Convert an XMPP message to an `InboundMessage`.
    ///
    /// Returns `None` if:
    /// - The message is from the bot itself
    /// - The message body is empty
    /// - The sender JID is empty
    pub fn convert_message(
        msg: &super::types::XmppMessage,
        channel_id: &ChannelId,
        own_jid: &str,
    ) -> Option<InboundMessage> {
        if msg.body.is_empty() || msg.from.is_empty() {
            return None;
        }

        // Parse sender JID
        let sender_jid = parse_jid(&msg.from)?;

        // Parse our own JID for comparison
        let own_parts = parse_jid(own_jid);

        // Determine if this is a groupchat (MUC) message
        let is_group = msg.msg_type == "groupchat";

        if is_group {
            // In MUC, the from JID is "room@conference/nick"
            // The resource part is the sender's nick in the room
            let nick = sender_jid.resource.as_deref().unwrap_or("");

            // Skip our own messages (compare nick against our MUC nick)
            if let Some(ref own) = own_parts {
                if nick == own.local {
                    return None;
                }
            }

            // For MUC, conversation_id is the room bare JID
            let conversation_id = sender_jid.bare();

            Some(InboundMessage {
                id: MessageId::new(
                    msg.id
                        .clone()
                        .unwrap_or_else(|| format!("xmpp-{}", Utc::now().timestamp_millis())),
                ),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(conversation_id),
                sender_id: UserId::new(msg.from.clone()),
                sender_name: Some(nick.to_string()),
                text: msg.body.clone(),
                attachments: Vec::new(),
                timestamp: Utc::now(),
                reply_to: None,
                is_group: true,
                raw: None,
            })
        } else {
            // 1-on-1 chat: from is the sender's full JID
            // Skip our own messages
            if let Some(ref own) = own_parts {
                if sender_jid.local == own.local && sender_jid.domain == own.domain {
                    return None;
                }
            }

            let conversation_id = sender_jid.bare();
            let sender_name = sender_jid.local.clone();

            Some(InboundMessage {
                id: MessageId::new(
                    msg.id
                        .clone()
                        .unwrap_or_else(|| format!("xmpp-{}", Utc::now().timestamp_millis())),
                ),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(conversation_id),
                sender_id: UserId::new(msg.from.clone()),
                sender_name: Some(sender_name),
                text: msg.body.clone(),
                attachments: Vec::new(),
                timestamp: Utc::now(),
                reply_to: None,
                is_group: false,
                raw: None,
            })
        }
    }

    /// Format and send a message stanza through the write channel.
    ///
    /// Formats text as plain text and splits long messages.
    pub async fn send_message(
        write_tx: &tokio::sync::mpsc::Sender<String>,
        to: &str,
        text: &str,
        msg_type: &str,
    ) -> Result<SendResult, ChannelError> {
        let formatted = MessageFormatter::format(text, MarkupFormat::PlainText);
        let chunks = MessageFormatter::split(&formatted, XMPP_MSG_LIMIT);

        for chunk in &chunks {
            let stanza = build_message_stanza(to, chunk, msg_type);
            write_tx
                .send(stanza)
                .await
                .map_err(|e| ChannelError::SendFailed(format!("XMPP write channel closed: {e}")))?;
        }

        Ok(SendResult {
            message_id: MessageId::new(format!("xmpp-sent-{}", Utc::now().timestamp_millis())),
            timestamp: Utc::now(),
        })
    }

    /// Run the XMPP connection loop with automatic reconnection.
    ///
    /// This function:
    /// 1. Connects TCP to server:port
    /// 2. Sends stream header
    /// 3. Authenticates with SASL PLAIN
    /// 4. Sends new stream header (post-auth)
    /// 5. Binds resource + starts session
    /// 6. Sends initial presence
    /// 7. Joins configured MUC rooms
    /// 8. Reads stanzas, handles pings, forwards messages
    /// 9. Reconnects with exponential backoff on disconnection
    pub async fn run_xmpp_loop(
        config: XmppConfig,
        channel_id: ChannelId,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        mut write_cmd_rx: tokio::sync::mpsc::Receiver<String>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let mut backoff = INITIAL_BACKOFF;
        let addr = config.addr();

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            tracing::info!("Connecting to XMPP server at {addr}...");

            let stream = match TcpStream::connect(&addr).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("XMPP connection failed: {e}, retrying in {backoff:?}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            backoff = INITIAL_BACKOFF;
            tracing::info!("XMPP connected to {addr}");

            let (mut reader, mut writer) = stream.into_split();
            let mut buffer = String::new();
            let mut read_buf = [0u8; 4096];

            // Phase 1: Send stream header
            let domain = config.server_host().to_string();
            let header = build_stream_header(&domain);
            if let Err(e) = writer.write_all(header.as_bytes()).await {
                tracing::warn!("XMPP stream header send failed: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }

            let mut authenticated = false;
            let mut bound = false;
            let own_jid = config.jid.clone();

            // Main connection loop
            let should_reconnect = 'inner: loop {
                tokio::select! {
                    read_result = reader.read(&mut read_buf) => {
                        let n = match read_result {
                            Ok(0) => {
                                tracing::info!("XMPP connection closed");
                                break 'inner true;
                            }
                            Ok(n) => n,
                            Err(e) => {
                                tracing::warn!("XMPP read error: {e}");
                                break 'inner true;
                            }
                        };

                        // Append received data to buffer
                        if let Ok(text) = std::str::from_utf8(&read_buf[..n]) {
                            buffer.push_str(text);
                        } else {
                            tracing::warn!("XMPP: non-UTF8 data received, skipping");
                            continue;
                        }

                        // Process all complete stanzas in the buffer
                        while let Some((stanza, remaining)) = extract_stanza(&buffer) {
                            buffer = remaining;

                            tracing::debug!("XMPP < {}", stanza.get(..200).unwrap_or(&stanza));

                            // Handle based on connection phase
                            if !authenticated {
                                // Pre-auth phase
                                if is_stream_features(&stanza) {
                                    // Send auth
                                    let auth = build_auth_stanza(&config.jid, &config.password);
                                    if let Err(e) = writer.write_all(auth.as_bytes()).await {
                                        tracing::warn!("XMPP auth send failed: {e}");
                                        break 'inner true;
                                    }
                                } else if is_auth_success(&stanza) {
                                    tracing::info!("XMPP SASL authentication successful");
                                    authenticated = true;

                                    // Send new stream header (required after auth)
                                    let header = build_stream_header(&domain);
                                    if let Err(e) = writer.write_all(header.as_bytes()).await {
                                        tracing::warn!("XMPP post-auth stream header failed: {e}");
                                        break 'inner true;
                                    }
                                } else if is_auth_failure(&stanza) {
                                    tracing::error!("XMPP SASL authentication failed");
                                    break 'inner false; // Don't reconnect on auth failure
                                }
                            } else if !bound {
                                // Post-auth, pre-bind phase
                                if is_stream_features(&stanza) {
                                    // Send resource bind
                                    let bind = build_bind_stanza("aleph");
                                    if let Err(e) = writer.write_all(bind.as_bytes()).await {
                                        tracing::warn!("XMPP bind send failed: {e}");
                                        break 'inner true;
                                    }
                                } else if (stanza.contains("<iq") && stanza.contains("type='result'") || stanza.contains("type=\"result\"")) && stanza.contains("bind") {
                                        tracing::info!("XMPP resource bound");

                                        // Start session
                                        let session = build_session_stanza();
                                        if let Err(e) = writer.write_all(session.as_bytes()).await {
                                            tracing::warn!("XMPP session start failed: {e}");
                                            break 'inner true;
                                        }

                                        bound = true;

                                        // Send initial presence
                                        let presence = build_presence_stanza();
                                        if let Err(e) = writer.write_all(presence.as_bytes()).await {
                                            tracing::warn!("XMPP presence send failed: {e}");
                                            break 'inner true;
                                        }
                                        tracing::info!("XMPP online presence sent");

                                        // Join MUC rooms
                                        for room in &config.muc_rooms {
                                            let muc_presence = build_muc_join_stanza(room, &config.nick);
                                            if let Err(e) = writer.write_all(muc_presence.as_bytes()).await {
                                                tracing::warn!("XMPP MUC join failed for {room}: {e}");
                                                break 'inner true;
                                            }
                                            tracing::info!("XMPP joining MUC room {room}");
                                        }
                                }
                            } else {
                                // Fully connected — handle messages and pings
                                if let Some(msg) = parse_message_stanza(&stanza) {
                                    if let Some(inbound) = Self::convert_message(
                                        &msg,
                                        &channel_id,
                                        &own_jid,
                                    ) {
                                        tracing::debug!(
                                            "XMPP message from {}: {}",
                                            inbound.sender_id.as_str(),
                                            inbound.text.get(..50).unwrap_or(&inbound.text)
                                        );
                                        if inbound_tx.send(inbound).await.is_err() {
                                            tracing::error!("XMPP: inbound channel closed");
                                            return;
                                        }
                                    }
                                } else if let Some((id, from)) = extract_ping(&stanza) {
                                    let pong = build_pong_stanza(&id, &from, &own_jid);
                                    if let Err(e) = writer.write_all(pong.as_bytes()).await {
                                        tracing::warn!("XMPP pong send failed: {e}");
                                        break 'inner true;
                                    }
                                }
                            }
                        }
                    }

                    // Outbound message requests from send()
                    Some(raw_stanza) = write_cmd_rx.recv() => {
                        if let Err(e) = writer.write_all(raw_stanza.as_bytes()).await {
                            tracing::warn!("XMPP write failed: {e}");
                            break 'inner true;
                        }
                    }

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!("XMPP adapter shutting down");
                            let close = build_stream_close();
                            let _ = writer.write_all(close.as_bytes()).await;
                            return;
                        }
                    }
                }
            };

            if !should_reconnect || *shutdown_rx.borrow() {
                break;
            }

            tracing::warn!("XMPP: reconnecting in {backoff:?}");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }

        tracing::info!("XMPP connection loop stopped");
    }
}
