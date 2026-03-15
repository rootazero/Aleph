//! Tests for XMPP message operations.

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
    assert_eq!(xml_helpers::xml_escape("a<b>c&d\"e'f"), "a&lt;b&gt;c&amp;d&quot;e&apos;f");
}

#[test]
fn test_xml_escape_no_special() {
    assert_eq!(xml_helpers::xml_escape("hello world"), "hello world");
}

#[test]
fn test_xml_unescape() {
    assert_eq!(
        xml_helpers::xml_unescape("a&lt;b&gt;c&amp;d&quot;e&apos;f"),
        "a<b>c&d\"e'f"
    );
}

#[test]
fn test_extract_tag_content_body() {
    let xml = "<message><body>Hello world</body></message>";
    assert_eq!(xml_helpers::extract_tag_content(xml, "body"), Some("Hello world"));
}

#[test]
fn test_extract_tag_content_thread() {
    let xml = "<message><body>Hi</body><thread>t-123</thread></message>";
    assert_eq!(xml_helpers::extract_tag_content(xml, "thread"), Some("t-123"));
}

#[test]
fn test_extract_tag_content_missing() {
    let xml = "<message><body>Hi</body></message>";
    assert_eq!(xml_helpers::extract_tag_content(xml, "thread"), None);
}

#[test]
fn test_extract_tag_content_self_closing() {
    let xml = "<message><body/></message>";
    assert_eq!(xml_helpers::extract_tag_content(xml, "body"), None);
}

#[test]
fn test_extract_attribute_from() {
    let xml = "<message from='alice@example.com' type='chat'><body>Hi</body></message>";
    assert_eq!(xml_helpers::extract_attribute(xml, "from"), Some("alice@example.com"));
}

#[test]
fn test_extract_attribute_type() {
    let xml = "<message from='alice@example.com' type='groupchat'><body>Hi</body></message>";
    assert_eq!(xml_helpers::extract_attribute(xml, "type"), Some("groupchat"));
}

#[test]
fn test_extract_attribute_id() {
    let xml = "<message id='msg-123' from='alice@example.com'><body>Hi</body></message>";
    assert_eq!(xml_helpers::extract_attribute(xml, "id"), Some("msg-123"));
}

#[test]
fn test_extract_attribute_double_quotes() {
    let xml = r#"<message from="alice@example.com" type="chat"><body>Hi</body></message>"#;
    assert_eq!(xml_helpers::extract_attribute(xml, "from"), Some("alice@example.com"));
}

#[test]
fn test_extract_attribute_missing() {
    let xml = "<message><body>Hi</body></message>";
    assert_eq!(xml_helpers::extract_attribute(xml, "from"), None);
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
    let expected = xml_helpers::base64_encode(b"\0bot\0secret");
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
    use crate::gateway::channel::ChannelId;

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
    use crate::gateway::channel::ChannelId;

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
    use crate::gateway::channel::ChannelId;

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
    use crate::gateway::channel::ChannelId;

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
    use crate::gateway::channel::ChannelId;

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
    use crate::gateway::channel::ChannelId;

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
    use crate::gateway::channel::ChannelId;

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
