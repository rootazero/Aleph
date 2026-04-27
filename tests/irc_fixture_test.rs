use alephcore::gateway::channel::ChannelId;
use alephcore::gateway::interfaces::irc::message_ops::{
    convert_privmsg, nick_from_prefix, parse_irc_line,
};
use alephcore::gateway::interfaces::irc::IrcConfig;

fn test_irc_config() -> IrcConfig {
    IrcConfig {
        server: "irc.test.com".to_string(),
        port: 6667,
        nick: "testbot".to_string(),
        password: None,
        channels: vec!["#test".to_string()],
        use_tls: false,
        realname: "Test Bot".to_string(),
    }
}

#[test]
fn test_irc_privmsg_parsing() {
    let json_str = include_str!("fixtures/irc/privmsg.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["command"], "PRIVMSG");
    assert_eq!(data["params"][0], "#aleph");
    assert_eq!(data["trailing"], "Hello bot, how are you?");
}

#[test]
fn test_irc_ping_parsing() {
    let json_str = include_str!("fixtures/irc/ping.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["command"], "PING");
    assert_eq!(data["trailing"], "irc.example.com");
}

#[test]
fn test_irc_welcome_parsing() {
    let json_str = include_str!("fixtures/irc/rpl_welcome.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["command"], "001");
    assert_eq!(data["params"][0], "alephbot");
}

#[test]
fn test_parse_irc_line_with_prefix() {
    let line = ":user1!~user1@irc.example.com PRIVMSG #aleph :Hello bot\r\n";
    let parsed = parse_irc_line(line);
    assert!(parsed.is_some());
    let p = parsed.unwrap();
    assert_eq!(p.prefix, Some("user1!~user1@irc.example.com".to_string()));
    assert_eq!(p.command, "PRIVMSG");
    assert_eq!(p.params, vec!["#aleph"]);
    assert_eq!(p.trailing, Some("Hello bot".to_string()));
}

#[test]
fn test_parse_irc_line_without_prefix() {
    let line = "PING :irc.example.com\r\n";
    let parsed = parse_irc_line(line);
    assert!(parsed.is_some());
    let p = parsed.unwrap();
    assert_eq!(p.prefix, None);
    assert_eq!(p.command, "PING");
    assert_eq!(p.params, Vec::<String>::new());
    assert_eq!(p.trailing, Some("irc.example.com".to_string()));
}

#[test]
fn test_parse_irc_line_empty() {
    assert!(parse_irc_line("").is_none());
    assert!(parse_irc_line("   ").is_none());
}

#[test]
fn test_nick_from_prefix() {
    assert_eq!(nick_from_prefix("user1!~user1@irc.com"), "user1");
    assert_eq!(nick_from_prefix("user1"), "user1");
}

#[test]
fn test_convert_privmsg_success() {
    let line = ":user1!~user1@irc.example.com PRIVMSG #aleph :Hello bot\r\n";
    let parsed = parse_irc_line(line).unwrap();
    let channel_id = ChannelId::new("irc");
    let config = test_irc_config();

    let inbound = convert_privmsg(&parsed, &channel_id, "testbot", &config);

    assert!(inbound.is_some());
    let msg = inbound.unwrap();
    assert_eq!(msg.text, "Hello bot");
    assert_eq!(msg.sender_id.as_str(), "user1");
    assert_eq!(msg.conversation_id.as_str(), "#aleph");
    assert!(msg.is_group, "Channel messages should be group messages");
}

#[test]
fn test_convert_privmsg_filters_own_messages() {
    let line = ":testbot!~test@irc.example.com PRIVMSG #test :Hello\r\n";
    let parsed = parse_irc_line(line).unwrap();
    let channel_id = ChannelId::new("irc");
    let config = test_irc_config();

    let inbound = convert_privmsg(&parsed, &channel_id, "testbot", &config);

    assert!(inbound.is_none(), "Bot's own messages should be filtered");
}

#[test]
fn test_convert_privmsg_non_privmsg() {
    let line = ":irc.example.com 001 testbot :Welcome\r\n";
    let parsed = parse_irc_line(line).unwrap();
    let channel_id = ChannelId::new("irc");
    let config = test_irc_config();

    let inbound = convert_privmsg(&parsed, &channel_id, "testbot", &config);

    assert!(inbound.is_none(), "Non-PRIVMSG commands should be ignored");
}
