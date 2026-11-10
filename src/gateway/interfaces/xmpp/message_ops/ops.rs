//! XMPP message operations (convert, send, connection loop).

use std::time::Duration;

use chrono::Utc;

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, InboundMessageSender, MessageId,
    SendResult, UserId,
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
const MAX_STANZA_SIZE: usize = 1_048_576;

pub type WriteCommand = (String, tokio::sync::oneshot::Sender<Result<(), String>>);

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
    #[must_use]
    pub fn convert_message(
        msg: &super::types::XmppMessage,
        channel_id: &ChannelId,
        own_jid: &str,
        muc_nick: Option<&str>,
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

            // Skip our own messages. In a MUC the sender is identified by
            // the NICK it joined with (`config.nick`), not by the bare JID —
            // comparing the nick against the bare JID never matched, so the
            // bot's own room messages were ingested as inbound traffic.
            if let Some(ref own) = own_parts {
                if nick == own.bare() {
                    return None;
                }
            }
            if let Some(joined_nick) = muc_nick {
                if !joined_nick.is_empty() && nick == joined_nick {
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
                metadata: vec![],
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
                metadata: vec![],
            })
        }
    }

    /// Format and send a message stanza through the write channel.
    ///
    /// Formats text as plain text and splits long messages.
    pub async fn send_message(
        write_tx: &tokio::sync::mpsc::Sender<WriteCommand>,
        to: &str,
        text: &str,
        msg_type: &str,
    ) -> Result<SendResult, ChannelError> {
        let formatted = MessageFormatter::format(text, MarkupFormat::PlainText);
        let chunks = MessageFormatter::split(&formatted, XMPP_MSG_LIMIT);

        // Acknowledge local socket writes, not remote delivery.
        let mut last_stanza_id: Option<String> = None;

        for chunk in &chunks {
            let stanza = build_message_stanza(to, chunk, msg_type);

            // Extract the stanza id embedded by build_message_stanza so the
            // returned SendResult carries the real on-wire ID, not a
            // fabricated timestamp.  The id attribute is inserted as
            // id='<value>' by build_message_stanza; extract it heuristically.
            if let Some(id_start) = stanza.find(" id='") {
                let after = &stanza[id_start + 5..];
                if let Some(id_end) = after.find('\'') {
                    last_stanza_id = Some(after[..id_end].to_owned());
                }
            }

            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            write_tx
                .send((stanza, ack_tx))
                .await
                .map_err(|e| ChannelError::SendFailed(format!("XMPP write channel closed: {e}")))?;
            ack_rx
                .await
                .map_err(|_| {
                    ChannelError::SendFailed("XMPP writer stopped; delivery uncertain".into())
                })?
                .map_err(ChannelError::SendFailed)?;
        }

        // All local writes completed; this does not acknowledge remote delivery.
        let id_str = last_stanza_id
            .unwrap_or_else(|| format!("xmpp-sent-{}", Utc::now().timestamp_millis()));

        Ok(SendResult {
            message_id: MessageId::new(id_str),
            timestamp: Utc::now(),
        })
    }

    /// Run the XMPP connection loop with automatic reconnection.
    ///
    /// This function:
    /// 1. Connects TCP to server:port
    /// 2. Upgrades to TLS if use_tls is enabled
    /// 3. Sends stream header
    /// 4. Authenticates with SASL PLAIN
    /// 5. Sends new stream header (post-auth)
    /// 6. Binds resource + starts session
    /// 7. Sends initial presence
    /// 8. Joins configured MUC rooms
    /// 9. Reads stanzas, handles pings, forwards messages
    /// 10. Reconnects with exponential backoff on disconnection
    pub async fn run_xmpp_loop(
        config: XmppConfig,
        channel_id: ChannelId,
        inbound_tx: InboundMessageSender,
        mut write_cmd_rx: tokio::sync::mpsc::Receiver<WriteCommand>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        const MAX_STANZA_SIZE: usize = 1024 * 1024;

        if config.use_tls {
            tracing::error!("XMPP TLS is not implemented; refusing to connect with use_tls=true");
            return;
        }

        let mut backoff = INITIAL_BACKOFF;
        let addr = config.addr();

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            tracing::info!("Connecting to XMPP server at {addr}...");
            let tcp_stream = match TcpStream::connect(&addr).await {
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
            let (mut reader, mut writer) = tcp_stream.into_split();
            let mut buffer = String::new();
            let mut pending: Vec<u8> = Vec::new();
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

                                        // Append received data to buffer. A multi-byte UTF-8
                                        // sequence may be split across reads, so accumulate raw
                                        // bytes and only move the valid prefix into the string.
                                        if buffer.len() + pending.len() + n > MAX_STANZA_SIZE {
                                            tracing::error!("XMPP stanza buffer exceeds {MAX_STANZA_SIZE} bytes");
                                            break 'inner true;
                                        }
                                        pending.extend_from_slice(&read_buf[..n]);
                                        loop {
                                            match std::str::from_utf8(&pending) {
                                                Ok(text) => {
                                                    buffer.push_str(text);
                                                    pending.clear();
                                                    break;
                                                }
                                                Err(e) => {
                                                    let valid = e.valid_up_to();
                                                    if let Ok(text) = std::str::from_utf8(&pending[..valid]) {
                                                        buffer.push_str(text);
                                                    }
                                                    match e.error_len() {
                                                        Some(bad) => {
                                                            tracing::warn!("XMPP: skipping {bad} invalid UTF-8 byte(s)");
                                                            pending.drain(..valid + bad);
                                                        }
                                                        None => {
                                                            // Incomplete trailing sequence — keep it for the next read
                                                            pending.drain(..valid);
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
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
                                                } else if stanza.contains("<iq") && (stanza.contains("type='result'") || stanza.contains("type=\"result\"")) && stanza.contains("bind") {
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
                                        Some(&config.nick),
                                    ) {
                                        tracing::debug!(
                                            "XMPP message from {}: {}",
                                            inbound.sender_id.as_str(),
                                            inbound.text.get(..50).unwrap_or(&inbound.text)
                                        );
                                        if inbound_tx.send(inbound).is_err() {
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
                    Some((raw_stanza, ack)) = write_cmd_rx.recv(), if authenticated && bound => {
                        match writer.write_all(raw_stanza.as_bytes()).await {
                            Ok(()) => { let _ = ack.send(Ok(())); }
                            Err(e) => {
                                let _ = ack.send(Err(format!("XMPP write failed; delivery uncertain: {e}")));
                                break 'inner true;
                            }
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

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[tokio::test]
    async fn send_reports_writer_failure() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<WriteCommand>(1);
        let writer = tokio::spawn(async move {
            let (stanza, ack) = rx.recv().await.unwrap();
            assert!(stanza.contains(" id='"));
            ack.send(Err("partial write; delivery uncertain".into()))
                .unwrap();
        });
        assert!(
            XmppMessageOps::send_message(&tx, "a@example.org", "hello", "chat")
                .await
                .is_err()
        );
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_incomplete_stanza_closes_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = XmppConfig {
            server: Some("127.0.0.1".into()),
            port: listener.local_addr().unwrap().port(),
            jid: "bot@example.org".into(),
            password: "secret".into(),
            use_tls: false,
            ..Default::default()
        };
        let inbound = crate::gateway::channel::ChannelState::new(1).sender();
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        let (_shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(XmppMessageOps::run_xmpp_loop(
            config,
            ChannelId::new("xmpp"),
            inbound,
            rx,
            shutdown_rx,
        ));
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = [0; 1024];
        socket.read(&mut header).await.unwrap();
        let payload = format!("<message><body>{}", "a".repeat(MAX_STANZA_SIZE));
        let _ = socket.write_all(payload.as_bytes()).await;
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut header)).await;
        task.abort();
        assert!(matches!(closed, Ok(Ok(0)) | Ok(Err(_))));
    }
}
