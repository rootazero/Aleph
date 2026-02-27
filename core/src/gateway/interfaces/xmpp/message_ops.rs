//! XMPP Stanza Operations
//!
//! Low-level XMPP stanza building and parsing using simple string operations.
//! Handles only the minimal subset of XMPP XML required for messaging:
//!
//! - `<stream:stream>` — connection setup
//! - `<auth>` — SASL PLAIN authentication
//! - `<presence>` — online status + MUC join
//! - `<message>` — send/receive messages
//! - `<iq>` — info queries (ping)
//!
//! Uses raw `format!()` macros for XML building and simple string searching
//! for XML extraction. This is intentionally NOT a general XML parser.

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use chrono::Utc;
use std::time::Duration;

use super::config::XmppConfig;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Maximum message length for XMPP messages.
pub(crate) const XMPP_MSG_LIMIT: usize = 65535;

/// Parsed JID (Jabber ID) components.
///
/// A full JID has the form: `local@domain/resource`
/// - `local@domain` is the bare JID
/// - `/resource` is optional
#[derive(Debug, Clone, PartialEq)]
pub struct JidParts {
    /// User/local part (before @)
    pub local: String,
    /// Server domain (between @ and /)
    pub domain: String,
    /// Optional resource (after /)
    pub resource: Option<String>,
}

impl JidParts {
    /// Reconstruct the bare JID (without resource)
    pub fn bare(&self) -> String {
        format!("{}@{}", self.local, self.domain)
    }
}

/// Parsed XMPP message stanza.
#[derive(Debug, Clone, PartialEq)]
pub struct XmppMessage {
    /// Sender JID
    pub from: String,
    /// Message body text
    pub body: String,
    /// Message type: "chat" or "groupchat"
    pub msg_type: String,
    /// Optional thread ID
    pub thread: Option<String>,
    /// Optional message ID
    pub id: Option<String>,
}

// ==================== JID Parsing ====================

/// Parse a JID string into its components.
///
/// Supports formats:
/// - `local@domain`
/// - `local@domain/resource`
///
/// Returns `None` if the JID doesn't contain `@`.
pub fn parse_jid(jid: &str) -> Option<JidParts> {
    let at_pos = jid.find('@')?;
    let local = jid[..at_pos].to_string();
    let remainder = &jid[at_pos + 1..];

    let (domain, resource) = if let Some(slash_pos) = remainder.find('/') {
        (
            remainder[..slash_pos].to_string(),
            Some(remainder[slash_pos + 1..].to_string()),
        )
    } else {
        (remainder.to_string(), None)
    };

    if local.is_empty() || domain.is_empty() {
        return None;
    }

    Some(JidParts {
        local,
        domain,
        resource,
    })
}

// ==================== XML Helpers ====================

/// Escape special characters for XML content.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Extract the content between opening and closing XML tags.
///
/// For `<body>Hello world</body>`, returns `Some("Hello world")`.
/// Handles self-closing tags by returning `None`.
fn extract_tag_content<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open_tag = format!("<{}", tag);
    let close_tag = format!("</{}>", tag);

    let start_pos = xml.find(&open_tag)?;
    // Find the end of the opening tag
    let tag_end = xml[start_pos..].find('>')?;
    let content_start = start_pos + tag_end + 1;

    // Check for self-closing tag
    if xml[start_pos..start_pos + tag_end + 1].ends_with("/>") {
        return None;
    }

    let content_end = xml[content_start..].find(&close_tag)?;

    Some(&xml[content_start..content_start + content_end])
}

/// Extract an XML attribute value from a tag.
///
/// Supports both single and double quoted attributes:
/// - `from="user@example.com"` -> `Some("user@example.com")`
/// - `from='user@example.com'` -> `Some("user@example.com")`
fn extract_attribute<'a>(xml: &'a str, attr: &str) -> Option<&'a str> {
    // Try double quotes first: attr="value"
    let dq_pattern = format!("{}=\"", attr);
    if let Some(start) = xml.find(&dq_pattern) {
        let value_start = start + dq_pattern.len();
        if let Some(value_end) = xml[value_start..].find('"') {
            return Some(&xml[value_start..value_start + value_end]);
        }
    }

    // Try single quotes: attr='value'
    let sq_pattern = format!("{}='", attr);
    if let Some(start) = xml.find(&sq_pattern) {
        let value_start = start + sq_pattern.len();
        if let Some(value_end) = xml[value_start..].find('\'') {
            return Some(&xml[value_start..value_start + value_end]);
        }
    }

    None
}

/// Unescape XML entities in text content.
fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

// ==================== Stanza Building ====================

/// Build the opening `<stream:stream>` XML header.
///
/// This is the first thing sent after TCP connection.
pub fn build_stream_header(domain: &str) -> String {
    format!(
        "<?xml version='1.0'?>\
         <stream:stream \
         to='{}' \
         xmlns='jabber:client' \
         xmlns:stream='http://etherx.jabber.org/streams' \
         version='1.0'>",
        xml_escape(domain)
    )
}

/// Build a SASL PLAIN auth stanza.
///
/// SASL PLAIN format: base64(\0user\0password)
/// The authzid is empty (first \0), authcid is the JID local part.
pub fn build_auth_stanza(jid: &str, password: &str) -> String {
    let local = jid.split('@').next().unwrap_or(jid);
    let plain = format!("\0{}\0{}", local, password);
    let encoded = base64_encode(plain.as_bytes());

    format!(
        "<auth xmlns='urn:ietf:params:xml:ns:xmpp-sasl' mechanism='PLAIN'>{}</auth>",
        encoded
    )
}

/// Build a presence stanza for going online.
pub fn build_presence_stanza() -> String {
    "<presence/>".to_string()
}

/// Build a MUC join presence stanza.
///
/// Sends presence to `room_jid/nick` with MUC extension element.
pub fn build_muc_join_stanza(room_jid: &str, nick: &str) -> String {
    format!(
        "<presence to='{}/{}'>\
         <x xmlns='http://jabber.org/protocol/muc'/>\
         </presence>",
        xml_escape(room_jid),
        xml_escape(nick)
    )
}

/// Build a message stanza.
///
/// - `msg_type` should be "chat" for 1-on-1 or "groupchat" for MUC.
pub fn build_message_stanza(to: &str, body: &str, msg_type: &str) -> String {
    let id = format!("msg-{}", Utc::now().timestamp_millis());
    format!(
        "<message type='{}' to='{}' id='{}'>\
         <body>{}</body>\
         </message>",
        xml_escape(msg_type),
        xml_escape(to),
        xml_escape(&id),
        xml_escape(body)
    )
}

/// Build a resource bind IQ stanza.
///
/// Sent after successful SASL auth to bind a resource.
pub fn build_bind_stanza(resource: &str) -> String {
    format!(
        "<iq type='set' id='bind-1'>\
         <bind xmlns='urn:ietf:params:xml:ns:xmpp-bind'>\
         <resource>{}</resource>\
         </bind>\
         </iq>",
        xml_escape(resource)
    )
}

/// Build a session establishment IQ stanza.
pub fn build_session_stanza() -> String {
    "<iq type='set' id='session-1'>\
     <session xmlns='urn:ietf:params:xml:ns:xmpp-session'/>\
     </iq>"
        .to_string()
}

/// Build a stream close tag.
pub fn build_stream_close() -> String {
    "</stream:stream>".to_string()
}

// ==================== Stanza Parsing ====================

/// Parse a received stanza for message content.
///
/// Returns `Some(XmppMessage)` if the stanza is a `<message>` with a `<body>`.
/// Returns `None` for non-message stanzas or messages without body.
pub fn parse_message_stanza(stanza: &str) -> Option<XmppMessage> {
    // Must be a message stanza
    if !stanza.contains("<message") {
        return None;
    }

    // Extract body content
    let body = extract_tag_content(stanza, "body")?;
    if body.is_empty() {
        return None;
    }

    let from = extract_attribute(stanza, "from")
        .unwrap_or("")
        .to_string();
    let msg_type = extract_attribute(stanza, "type")
        .unwrap_or("chat")
        .to_string();
    let id = extract_attribute(stanza, "id").map(|s| s.to_string());
    let thread = extract_tag_content(stanza, "thread").map(|s| s.to_string());

    Some(XmppMessage {
        from,
        body: xml_unescape(body),
        msg_type,
        thread,
        id,
    })
}

/// Check if a stanza indicates successful SASL authentication.
pub fn is_auth_success(stanza: &str) -> bool {
    stanza.contains("<success") && stanza.contains("urn:ietf:params:xml:ns:xmpp-sasl")
}

/// Check if a stanza indicates SASL authentication failure.
pub fn is_auth_failure(stanza: &str) -> bool {
    stanza.contains("<failure") && stanza.contains("urn:ietf:params:xml:ns:xmpp-sasl")
}

/// Check if a stanza is a stream features element.
pub fn is_stream_features(stanza: &str) -> bool {
    stanza.contains("<stream:features")
}

/// Check if a stanza is a ping IQ that needs a pong response.
///
/// Returns the IQ `id` and `from` if it's a ping.
pub fn extract_ping(stanza: &str) -> Option<(String, String)> {
    if !stanza.contains("urn:xmpp:ping") || !stanza.contains("type='get'") && !stanza.contains("type=\"get\"") {
        return None;
    }

    let id = extract_attribute(stanza, "id")?.to_string();
    let from = extract_attribute(stanza, "from")
        .unwrap_or("")
        .to_string();

    Some((id, from))
}

/// Build a pong response to a ping IQ.
pub fn build_pong_stanza(id: &str, to: &str, from: &str) -> String {
    if to.is_empty() {
        format!(
            "<iq type='result' id='{}' from='{}'/>",
            xml_escape(id),
            xml_escape(from)
        )
    } else {
        format!(
            "<iq type='result' id='{}' to='{}' from='{}'/>",
            xml_escape(id),
            xml_escape(to),
            xml_escape(from)
        )
    }
}

// ==================== Buffer Parsing ====================

/// Try to extract a complete stanza from the buffer.
///
/// Returns `Some((stanza, remaining))` if a complete stanza is found.
/// Returns `None` if more data is needed.
///
/// Handles:
/// - Self-closing tags: `<presence/>`
/// - Stream headers: `<stream:stream ...>`
/// - Simple paired tags: `<message ...>...</message>`
/// - SASL responses: `<success .../>`, `<failure.../>`
pub fn extract_stanza(buffer: &str) -> Option<(String, String)> {
    let trimmed = buffer.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    // Handle XML declaration (skip it)
    if trimmed.starts_with("<?xml") {
        if let Some(end) = trimmed.find("?>") {
            let remaining = &trimmed[end + 2..];
            // Recursively try to extract from the rest
            if remaining.trim().is_empty() {
                return Some((trimmed[..end + 2].to_string(), String::new()));
            }
            return extract_stanza(remaining);
        }
        return None;
    }

    // Handle stream:stream opening tag (it's never closed in normal flow)
    if trimmed.starts_with("<stream:stream") {
        if let Some(end) = trimmed.find('>') {
            let stanza = trimmed[..end + 1].to_string();
            let remaining = trimmed[end + 1..].to_string();
            return Some((stanza, remaining));
        }
        return None;
    }

    // Handle stream close
    if trimmed.starts_with("</stream:stream") {
        if let Some(end) = trimmed.find('>') {
            let stanza = trimmed[..end + 1].to_string();
            let remaining = trimmed[end + 1..].to_string();
            return Some((stanza, remaining));
        }
        return None;
    }

    // Must start with '<'
    if !trimmed.starts_with('<') {
        // Skip non-XML content
        if let Some(next_tag) = trimmed.find('<') {
            return extract_stanza(&trimmed[next_tag..]);
        }
        return None;
    }

    // Extract the tag name
    let tag_name_end = trimmed[1..]
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .map(|i| i + 1)?;
    let tag_name = &trimmed[1..tag_name_end];

    // Find the end of the opening tag
    let open_tag_end = trimmed.find('>')?;

    // Check for self-closing tag
    if trimmed[..open_tag_end + 1].ends_with("/>") {
        let stanza = trimmed[..open_tag_end + 1].to_string();
        let remaining = trimmed[open_tag_end + 1..].to_string();
        return Some((stanza, remaining));
    }

    // Look for the closing tag
    let close_tag = format!("</{}>", tag_name);
    if let Some(close_pos) = trimmed.find(&close_tag) {
        let end = close_pos + close_tag.len();
        let stanza = trimmed[..end].to_string();
        let remaining = trimmed[end..].to_string();
        return Some((stanza, remaining));
    }

    // Not a complete stanza yet
    None
}

// ==================== Base64 Encoding (minimal) ====================

/// Simple base64 encoding for SASL PLAIN.
fn base64_encode(data: &[u8]) -> String {
    // Use the base64 crate that's already a dependency
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

// ==================== Message Operations ====================

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
        msg: &XmppMessage,
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
            write_tx.send(stanza).await.map_err(|e| {
                ChannelError::SendFailed(format!("XMPP write channel closed: {e}"))
            })?;
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
    #[cfg(feature = "xmpp")]
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

                            tracing::debug!("XMPP < {}", &stanza[..stanza.len().min(200)]);

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
                                } else if stanza.contains("<iq") && stanza.contains("type='result'") || stanza.contains("type=\"result\"") {
                                    if stanza.contains("bind") {
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
                                            &inbound.text[..inbound.text.len().min(50)]
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

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== JID Parsing Tests ====================

    #[test]
    fn test_parse_jid_basic() {
        let jid = parse_jid("user@example.com").unwrap();
        assert_eq!(jid.local, "user");
        assert_eq!(jid.domain, "example.com");
        assert!(jid.resource.is_none());
    }

    #[test]
    fn test_parse_jid_with_resource() {
        let jid = parse_jid("user@example.com/laptop").unwrap();
        assert_eq!(jid.local, "user");
        assert_eq!(jid.domain, "example.com");
        assert_eq!(jid.resource.as_deref(), Some("laptop"));
    }

    #[test]
    fn test_parse_jid_muc_occupant() {
        let jid = parse_jid("room@conference.example.com/alice").unwrap();
        assert_eq!(jid.local, "room");
        assert_eq!(jid.domain, "conference.example.com");
        assert_eq!(jid.resource.as_deref(), Some("alice"));
    }

    #[test]
    fn test_parse_jid_no_at() {
        assert!(parse_jid("nope").is_none());
    }

    #[test]
    fn test_parse_jid_empty_local() {
        assert!(parse_jid("@example.com").is_none());
    }

    #[test]
    fn test_parse_jid_empty_domain() {
        assert!(parse_jid("user@").is_none());
    }

    #[test]
    fn test_parse_jid_bare() {
        let jid = parse_jid("bot@example.com/res").unwrap();
        assert_eq!(jid.bare(), "bot@example.com");
    }

    // ==================== XML Helper Tests ====================

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("a<b>c&d\"e'f"), "a&lt;b&gt;c&amp;d&quot;e&apos;f");
    }

    #[test]
    fn test_xml_escape_no_special() {
        assert_eq!(xml_escape("hello world"), "hello world");
    }

    #[test]
    fn test_xml_unescape() {
        assert_eq!(
            xml_unescape("a&lt;b&gt;c&amp;d&quot;e&apos;f"),
            "a<b>c&d\"e'f"
        );
    }

    #[test]
    fn test_extract_tag_content_body() {
        let xml = "<message><body>Hello world</body></message>";
        assert_eq!(extract_tag_content(xml, "body"), Some("Hello world"));
    }

    #[test]
    fn test_extract_tag_content_thread() {
        let xml = "<message><body>Hi</body><thread>t-123</thread></message>";
        assert_eq!(extract_tag_content(xml, "thread"), Some("t-123"));
    }

    #[test]
    fn test_extract_tag_content_missing() {
        let xml = "<message><body>Hi</body></message>";
        assert_eq!(extract_tag_content(xml, "thread"), None);
    }

    #[test]
    fn test_extract_tag_content_self_closing() {
        let xml = "<message><body/></message>";
        assert_eq!(extract_tag_content(xml, "body"), None);
    }

    #[test]
    fn test_extract_attribute_from() {
        let xml = "<message from='alice@example.com' type='chat'><body>Hi</body></message>";
        assert_eq!(extract_attribute(xml, "from"), Some("alice@example.com"));
    }

    #[test]
    fn test_extract_attribute_type() {
        let xml = "<message from='alice@example.com' type='groupchat'><body>Hi</body></message>";
        assert_eq!(extract_attribute(xml, "type"), Some("groupchat"));
    }

    #[test]
    fn test_extract_attribute_id() {
        let xml = "<message id='msg-123' from='alice@example.com'><body>Hi</body></message>";
        assert_eq!(extract_attribute(xml, "id"), Some("msg-123"));
    }

    #[test]
    fn test_extract_attribute_double_quotes() {
        let xml = r#"<message from="alice@example.com" type="chat"><body>Hi</body></message>"#;
        assert_eq!(extract_attribute(xml, "from"), Some("alice@example.com"));
    }

    #[test]
    fn test_extract_attribute_missing() {
        let xml = "<message><body>Hi</body></message>";
        assert_eq!(extract_attribute(xml, "from"), None);
    }

    // ==================== Stanza Building Tests ====================

    #[test]
    fn test_build_stream_header() {
        let header = build_stream_header("example.com");
        assert!(header.contains("<?xml version='1.0'?>"));
        assert!(header.contains("<stream:stream"));
        assert!(header.contains("to='example.com'"));
        assert!(header.contains("xmlns='jabber:client'"));
        assert!(header.contains("version='1.0'"));
    }

    #[test]
    fn test_build_auth_stanza() {
        let auth = build_auth_stanza("bot@example.com", "secret");
        assert!(auth.contains("<auth xmlns='urn:ietf:params:xml:ns:xmpp-sasl'"));
        assert!(auth.contains("mechanism='PLAIN'"));
        assert!(auth.contains("</auth>"));
        // The base64 content should be encoding of "\0bot\0secret"
        let expected = base64_encode(b"\0bot\0secret");
        assert!(auth.contains(&expected));
    }

    #[test]
    fn test_build_presence_stanza() {
        let presence = build_presence_stanza();
        assert_eq!(presence, "<presence/>");
    }

    #[test]
    fn test_build_muc_join_stanza() {
        let join = build_muc_join_stanza("room@conference.example.com", "aleph");
        assert!(join.contains("<presence to='room@conference.example.com/aleph'>"));
        assert!(join.contains("http://jabber.org/protocol/muc"));
        assert!(join.contains("</presence>"));
    }

    #[test]
    fn test_build_message_stanza_chat() {
        let msg = build_message_stanza("alice@example.com", "Hello!", "chat");
        assert!(msg.contains("type='chat'"));
        assert!(msg.contains("to='alice@example.com'"));
        assert!(msg.contains("<body>Hello!</body>"));
        assert!(msg.contains("</message>"));
        assert!(msg.contains("id='msg-"));
    }

    #[test]
    fn test_build_message_stanza_groupchat() {
        let msg = build_message_stanza("room@conference.example.com", "Hello room!", "groupchat");
        assert!(msg.contains("type='groupchat'"));
        assert!(msg.contains("to='room@conference.example.com'"));
        assert!(msg.contains("<body>Hello room!</body>"));
    }

    #[test]
    fn test_build_message_stanza_escaping() {
        let msg = build_message_stanza("a@b.com", "Hello <world> & \"friends\"", "chat");
        assert!(msg.contains("Hello &lt;world&gt; &amp; &quot;friends&quot;"));
    }

    #[test]
    fn test_build_bind_stanza() {
        let bind = build_bind_stanza("aleph");
        assert!(bind.contains("type='set'"));
        assert!(bind.contains("urn:ietf:params:xml:ns:xmpp-bind"));
        assert!(bind.contains("<resource>aleph</resource>"));
    }

    #[test]
    fn test_build_session_stanza() {
        let session = build_session_stanza();
        assert!(session.contains("type='set'"));
        assert!(session.contains("urn:ietf:params:xml:ns:xmpp-session"));
    }

    #[test]
    fn test_build_stream_close() {
        assert_eq!(build_stream_close(), "</stream:stream>");
    }

    #[test]
    fn test_build_pong_stanza() {
        let pong = build_pong_stanza("ping-1", "server.example.com", "bot@example.com");
        assert!(pong.contains("type='result'"));
        assert!(pong.contains("id='ping-1'"));
        assert!(pong.contains("to='server.example.com'"));
        assert!(pong.contains("from='bot@example.com'"));
    }

    #[test]
    fn test_build_pong_stanza_no_to() {
        let pong = build_pong_stanza("ping-1", "", "bot@example.com");
        assert!(pong.contains("type='result'"));
        assert!(pong.contains("id='ping-1'"));
        assert!(!pong.contains("to="));
        assert!(pong.contains("from='bot@example.com'"));
    }

    // ==================== Stanza Parsing Tests ====================

    #[test]
    fn test_parse_message_chat() {
        let stanza = "<message from='alice@example.com/laptop' type='chat' id='msg-42'>\
                       <body>Hello there!</body></message>";
        let msg = parse_message_stanza(stanza).unwrap();
        assert_eq!(msg.from, "alice@example.com/laptop");
        assert_eq!(msg.body, "Hello there!");
        assert_eq!(msg.msg_type, "chat");
        assert_eq!(msg.id.as_deref(), Some("msg-42"));
        assert!(msg.thread.is_none());
    }

    #[test]
    fn test_parse_message_groupchat() {
        let stanza = "<message from='room@conference.example.com/alice' type='groupchat' id='gc-1'>\
                       <body>Hello room!</body></message>";
        let msg = parse_message_stanza(stanza).unwrap();
        assert_eq!(msg.from, "room@conference.example.com/alice");
        assert_eq!(msg.body, "Hello room!");
        assert_eq!(msg.msg_type, "groupchat");
    }

    #[test]
    fn test_parse_message_with_thread() {
        let stanza = "<message from='alice@example.com' type='chat'>\
                       <body>Threaded message</body>\
                       <thread>thread-abc</thread></message>";
        let msg = parse_message_stanza(stanza).unwrap();
        assert_eq!(msg.body, "Threaded message");
        assert_eq!(msg.thread.as_deref(), Some("thread-abc"));
    }

    #[test]
    fn test_parse_message_no_body() {
        let stanza = "<message from='alice@example.com' type='chat'></message>";
        assert!(parse_message_stanza(stanza).is_none());
    }

    #[test]
    fn test_parse_message_empty_body() {
        let stanza = "<message from='alice@example.com' type='chat'>\
                       <body></body></message>";
        assert!(parse_message_stanza(stanza).is_none());
    }

    #[test]
    fn test_parse_not_a_message() {
        let stanza = "<presence from='alice@example.com'/>";
        assert!(parse_message_stanza(stanza).is_none());
    }

    #[test]
    fn test_parse_message_default_type() {
        // If type is missing, default to "chat"
        let stanza = "<message from='alice@example.com'><body>No type</body></message>";
        let msg = parse_message_stanza(stanza).unwrap();
        assert_eq!(msg.msg_type, "chat");
    }

    #[test]
    fn test_parse_message_with_escaped_content() {
        let stanza = "<message from='alice@example.com' type='chat'>\
                       <body>Hello &amp; welcome &lt;friend&gt;</body></message>";
        let msg = parse_message_stanza(stanza).unwrap();
        assert_eq!(msg.body, "Hello & welcome <friend>");
    }

    // ==================== Auth Detection Tests ====================

    #[test]
    fn test_is_auth_success() {
        let stanza = "<success xmlns='urn:ietf:params:xml:ns:xmpp-sasl'/>";
        assert!(is_auth_success(stanza));
    }

    #[test]
    fn test_is_auth_success_not_success() {
        let stanza = "<failure xmlns='urn:ietf:params:xml:ns:xmpp-sasl'/>";
        assert!(!is_auth_success(stanza));
    }

    #[test]
    fn test_is_auth_failure() {
        let stanza = "<failure xmlns='urn:ietf:params:xml:ns:xmpp-sasl'>\
                       <not-authorized/></failure>";
        assert!(is_auth_failure(stanza));
    }

    #[test]
    fn test_is_auth_failure_not_failure() {
        let stanza = "<success xmlns='urn:ietf:params:xml:ns:xmpp-sasl'/>";
        assert!(!is_auth_failure(stanza));
    }

    #[test]
    fn test_is_stream_features() {
        let stanza = "<stream:features><mechanisms>...</mechanisms></stream:features>";
        assert!(is_stream_features(stanza));
    }

    // ==================== Ping/Pong Tests ====================

    #[test]
    fn test_extract_ping() {
        let stanza = "<iq from='server.example.com' type='get' id='ping-1'>\
                       <ping xmlns='urn:xmpp:ping'/></iq>";
        let (id, from) = extract_ping(stanza).unwrap();
        assert_eq!(id, "ping-1");
        assert_eq!(from, "server.example.com");
    }

    #[test]
    fn test_extract_ping_not_a_ping() {
        let stanza = "<iq type='result' id='bind-1'><bind/></iq>";
        assert!(extract_ping(stanza).is_none());
    }

    // ==================== Buffer Extraction Tests ====================

    #[test]
    fn test_extract_stanza_self_closing() {
        let buffer = "<presence/>";
        let (stanza, remaining) = extract_stanza(buffer).unwrap();
        assert_eq!(stanza, "<presence/>");
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_extract_stanza_message() {
        let buffer = "<message from='a@b.com'><body>Hi</body></message>extra";
        let (stanza, remaining) = extract_stanza(buffer).unwrap();
        assert_eq!(
            stanza,
            "<message from='a@b.com'><body>Hi</body></message>"
        );
        assert_eq!(remaining, "extra");
    }

    #[test]
    fn test_extract_stanza_stream_header() {
        let buffer = "<stream:stream to='example.com' xmlns='jabber:client'>rest";
        let (stanza, remaining) = extract_stanza(buffer).unwrap();
        assert!(stanza.starts_with("<stream:stream"));
        assert!(stanza.ends_with('>'));
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_extract_stanza_incomplete() {
        let buffer = "<message from='a@b.com'><body>Incomplete";
        assert!(extract_stanza(buffer).is_none());
    }

    #[test]
    fn test_extract_stanza_empty() {
        assert!(extract_stanza("").is_none());
        assert!(extract_stanza("   ").is_none());
    }

    #[test]
    fn test_extract_stanza_xml_declaration() {
        let buffer = "<?xml version='1.0'?><stream:stream to='example.com'>rest";
        let (stanza, remaining) = extract_stanza(buffer).unwrap();
        assert!(stanza.starts_with("<stream:stream"));
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_extract_stanza_stream_close() {
        let buffer = "</stream:stream>";
        let (stanza, remaining) = extract_stanza(buffer).unwrap();
        assert_eq!(stanza, "</stream:stream>");
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_extract_stanza_multiple() {
        let buffer = "<presence/><message from='a@b.com'><body>Hi</body></message>";
        let (stanza1, remaining) = extract_stanza(buffer).unwrap();
        assert_eq!(stanza1, "<presence/>");

        let (stanza2, remaining2) = extract_stanza(&remaining).unwrap();
        assert_eq!(
            stanza2,
            "<message from='a@b.com'><body>Hi</body></message>"
        );
        assert_eq!(remaining2, "");
    }

    // ==================== Convert Message Tests ====================

    #[test]
    fn test_convert_chat_message() {
        let msg = XmppMessage {
            from: "alice@example.com/laptop".to_string(),
            body: "Hello!".to_string(),
            msg_type: "chat".to_string(),
            thread: None,
            id: Some("msg-1".to_string()),
        };

        let channel_id = ChannelId::new("xmpp");
        let inbound =
            XmppMessageOps::convert_message(&msg, &channel_id, "bot@example.com").unwrap();

        assert_eq!(inbound.channel_id.as_str(), "xmpp");
        assert_eq!(inbound.conversation_id.as_str(), "alice@example.com");
        assert_eq!(inbound.sender_id.as_str(), "alice@example.com/laptop");
        assert_eq!(inbound.sender_name.as_deref(), Some("alice"));
        assert_eq!(inbound.text, "Hello!");
        assert!(!inbound.is_group);
        assert_eq!(inbound.id.as_str(), "msg-1");
    }

    #[test]
    fn test_convert_groupchat_message() {
        let msg = XmppMessage {
            from: "room@conference.example.com/alice".to_string(),
            body: "Hello room!".to_string(),
            msg_type: "groupchat".to_string(),
            thread: None,
            id: Some("gc-1".to_string()),
        };

        let channel_id = ChannelId::new("xmpp");
        let inbound =
            XmppMessageOps::convert_message(&msg, &channel_id, "bot@example.com").unwrap();

        assert!(inbound.is_group);
        assert_eq!(
            inbound.conversation_id.as_str(),
            "room@conference.example.com"
        );
        assert_eq!(inbound.sender_name.as_deref(), Some("alice"));
    }

    #[test]
    fn test_convert_skips_own_chat_message() {
        let msg = XmppMessage {
            from: "bot@example.com/aleph".to_string(),
            body: "My own message".to_string(),
            msg_type: "chat".to_string(),
            thread: None,
            id: None,
        };

        let channel_id = ChannelId::new("xmpp");
        let result = XmppMessageOps::convert_message(&msg, &channel_id, "bot@example.com");
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_skips_own_muc_message() {
        // In MUC, from="room@conference/nick" and we compare nick against our local JID part
        let msg = XmppMessage {
            from: "room@conference.example.com/bot".to_string(),
            body: "My MUC message".to_string(),
            msg_type: "groupchat".to_string(),
            thread: None,
            id: None,
        };

        let channel_id = ChannelId::new("xmpp");
        let result = XmppMessageOps::convert_message(&msg, &channel_id, "bot@example.com");
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_skips_empty_body() {
        let msg = XmppMessage {
            from: "alice@example.com".to_string(),
            body: String::new(),
            msg_type: "chat".to_string(),
            thread: None,
            id: None,
        };

        let channel_id = ChannelId::new("xmpp");
        let result = XmppMessageOps::convert_message(&msg, &channel_id, "bot@example.com");
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_skips_empty_from() {
        let msg = XmppMessage {
            from: String::new(),
            body: "Hello!".to_string(),
            msg_type: "chat".to_string(),
            thread: None,
            id: None,
        };

        let channel_id = ChannelId::new("xmpp");
        let result = XmppMessageOps::convert_message(&msg, &channel_id, "bot@example.com");
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_generates_id_when_missing() {
        let msg = XmppMessage {
            from: "alice@example.com/laptop".to_string(),
            body: "No ID".to_string(),
            msg_type: "chat".to_string(),
            thread: None,
            id: None,
        };

        let channel_id = ChannelId::new("xmpp");
        let inbound =
            XmppMessageOps::convert_message(&msg, &channel_id, "bot@example.com").unwrap();

        assert!(inbound.id.as_str().starts_with("xmpp-"));
    }
}
