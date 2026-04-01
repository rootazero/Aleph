//! XMPP stanza building and parsing.

use chrono::Utc;

use super::types::XmppMessage;
use super::xml_helpers::{
    base64_encode, extract_attribute, extract_tag_content, xml_escape, xml_unescape,
};

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

    let from = extract_attribute(stanza, "from").unwrap_or("").to_string();
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
    if !stanza.contains("urn:xmpp:ping")
        || !stanza.contains("type='get'") && !stanza.contains("type=\"get\"")
    {
        return None;
    }

    let id = extract_attribute(stanza, "id")?.to_string();
    let from = extract_attribute(stanza, "from").unwrap_or("").to_string();

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
