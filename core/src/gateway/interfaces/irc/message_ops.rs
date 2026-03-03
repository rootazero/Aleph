//! IRC Protocol Operations
//!
//! Low-level IRC protocol parsing and event loop implementation.
//! Handles RFC 2812 message format: `[:prefix] COMMAND [params] [:trailing]`
//!
//! Uses `tokio::net::TcpStream` with buffered I/O for raw TCP connections.
//! No external IRC dependencies required.

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use chrono::Utc;
use std::time::Duration;

use super::config::IrcConfig;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Maximum length for a single PRIVMSG payload, accounting for the
/// `:nick!user@host PRIVMSG #channel :` prefix overhead (~80 chars conservative).
pub(crate) const MAX_PRIVMSG_PAYLOAD: usize = 400;

/// Parsed IRC protocol line.
///
/// Represents the components of an IRC message per RFC 2812:
/// `[:prefix] COMMAND [params...] [:trailing]`
#[derive(Debug)]
pub struct IrcLine {
    /// Optional prefix (e.g., "nick!user@host" — without the leading colon)
    pub prefix: Option<String>,
    /// The IRC command (e.g., "PRIVMSG", "PING", "001")
    pub command: String,
    /// Parameters following the command
    pub params: Vec<String>,
    /// Trailing parameter (after " :" in the line)
    pub trailing: Option<String>,
}

/// Parse a raw IRC line into structured components.
///
/// IRC line format per RFC 2812: `[:prefix] COMMAND [params...] [:trailing]`
///
/// Returns `None` for empty or whitespace-only lines.
pub fn parse_irc_line(line: &str) -> Option<IrcLine> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let mut remaining = line;

    // Parse optional prefix (starts with ':')
    let prefix = if remaining.starts_with(':') {
        let space = remaining.find(' ')?;
        let pfx = remaining[1..space].to_string();
        remaining = &remaining[space + 1..];
        Some(pfx)
    } else {
        None
    };

    // Split off the trailing parameter (after " :")
    let (main_part, trailing) = if let Some(idx) = remaining.find(" :") {
        (
            remaining[..idx].to_string(),
            Some(remaining[idx + 2..].to_string()),
        )
    } else {
        (remaining.to_string(), None)
    };

    let mut parts = main_part.split_whitespace();
    let command = parts.next()?.to_string();
    let params: Vec<String> = parts.map(String::from).collect();

    Some(IrcLine {
        prefix,
        command,
        params,
        trailing,
    })
}

/// Extract the nickname from an IRC prefix like "nick!user@host".
///
/// Returns the part before '!', or the entire string if no '!' is present.
pub fn nick_from_prefix(prefix: &str) -> &str {
    prefix.split('!').next().unwrap_or(prefix)
}

/// Convert an IRC PRIVMSG to an `InboundMessage`.
///
/// Returns `None` if:
/// - The command is not PRIVMSG
/// - The message is from the bot itself (case-insensitive comparison)
/// - The message body is empty
/// - No prefix (sender info) is present
pub fn convert_privmsg(
    parsed: &IrcLine,
    channel_id: &ChannelId,
    own_nick: &str,
    _config: &IrcConfig,
) -> Option<InboundMessage> {
    if parsed.command != "PRIVMSG" {
        return None;
    }

    let prefix = parsed.prefix.as_deref()?;
    let sender_nick = nick_from_prefix(prefix);

    // Skip messages from the bot itself (case-insensitive)
    if sender_nick.eq_ignore_ascii_case(own_nick) {
        return None;
    }

    let target = parsed.params.first()?;
    let text = parsed.trailing.as_deref().unwrap_or("");
    if text.is_empty() {
        return None;
    }

    // Determine if this is a channel message (group) or a DM
    let is_group = target.starts_with('#') || target.starts_with('&');

    // For group messages, conversation_id is the channel name.
    // For DMs, conversation_id is the sender's nick (so replies go back to them).
    let conversation_id = if is_group {
        target.to_string()
    } else {
        sender_nick.to_string()
    };

    Some(InboundMessage {
        id: MessageId::new(format!(
            "irc-{}-{}",
            sender_nick,
            Utc::now().timestamp_millis()
        )),
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(conversation_id),
        sender_id: UserId::new(sender_nick.to_string()),
        sender_name: Some(sender_nick.to_string()),
        text: text.to_string(),
        attachments: Vec::new(),
        timestamp: Utc::now(),
        reply_to: None,
        is_group,
        raw: None, // IRC has no structured raw data
    })
}

/// IRC message operations helper.
///
/// Provides methods for sending messages and running the IRC connection loop.
pub struct IrcMessageOps;

impl IrcMessageOps {
    /// Format and send a PRIVMSG through the write channel.
    ///
    /// Formats text using IRC formatting codes and splits long messages.
    pub async fn send_message(
        write_tx: &tokio::sync::mpsc::Sender<String>,
        target: &str,
        text: &str,
    ) -> Result<SendResult, ChannelError> {
        let formatted = MessageFormatter::format(text, MarkupFormat::IrcFormatting);
        let chunks = MessageFormatter::split(&formatted, MAX_PRIVMSG_PAYLOAD);

        for chunk in &chunks {
            let raw = format!("PRIVMSG {} :{}\r\n", target, chunk);
            write_tx.send(raw).await.map_err(|e| {
                ChannelError::SendFailed(format!("IRC write channel closed: {e}"))
            })?;
        }

        Ok(SendResult {
            message_id: MessageId::new(format!(
                "irc-sent-{}",
                Utc::now().timestamp_millis()
            )),
            timestamp: Utc::now(),
        })
    }

    /// Run the IRC connection loop with automatic reconnection.
    ///
    /// This function:
    /// 1. Connects TCP to server:port
    /// 2. Sends NICK + USER registration
    /// 3. Optionally sends NickServ IDENTIFY
    /// 4. Reads lines from the server
    /// 5. Handles PING/PONG, 001 (RPL_WELCOME), 433 (nick in use), PRIVMSG
    /// 6. Processes outbound messages from the write channel
    /// 7. Reconnects with exponential backoff on disconnection
    /// 8. Sends QUIT on shutdown
    pub async fn run_irc_loop(
        config: IrcConfig,
        channel_id: ChannelId,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        mut write_cmd_rx: tokio::sync::mpsc::Receiver<String>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpStream;

        let mut backoff = INITIAL_BACKOFF;
        let addr = config.addr();

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            tracing::info!("Connecting to IRC server at {addr}...");

            let stream = match TcpStream::connect(&addr).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("IRC connection failed: {e}, retrying in {backoff:?}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            backoff = INITIAL_BACKOFF;
            tracing::info!("IRC connected to {addr}");

            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();

            // Send registration: NICK + USER
            let mut registration = String::new();
            registration.push_str(&format!("NICK {}\r\n", config.nick));
            registration.push_str(&format!(
                "USER {} 0 * :{}\r\n",
                config.nick, config.realname
            ));

            if let Err(e) = writer.write_all(registration.as_bytes()).await {
                tracing::warn!("IRC registration send failed: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }

            let nick = config.nick.clone();
            let channels_to_join = config.channels.clone();
            let mut joined = false;

            // Inner message loop - returns true if we should reconnect
            let should_reconnect = 'inner: loop {
                tokio::select! {
                    line_result = lines.next_line() => {
                        let line = match line_result {
                            Ok(Some(l)) => l,
                            Ok(None) => {
                                tracing::info!("IRC connection closed");
                                break 'inner true;
                            }
                            Err(e) => {
                                tracing::warn!("IRC read error: {e}");
                                break 'inner true;
                            }
                        };

                        tracing::debug!("IRC < {line}");

                        let parsed = match parse_irc_line(&line) {
                            Some(p) => p,
                            None => continue,
                        };

                        match parsed.command.as_str() {
                            // PING/PONG keepalive
                            "PING" => {
                                let pong_param = parsed
                                    .trailing
                                    .as_deref()
                                    .or(parsed.params.first().map(|s| s.as_str()))
                                    .unwrap_or("");
                                let pong = format!("PONG :{pong_param}\r\n");
                                if let Err(e) = writer.write_all(pong.as_bytes()).await {
                                    tracing::warn!("IRC PONG send failed: {e}");
                                    break 'inner true;
                                }
                            }

                            // RPL_WELCOME (001) - registration complete, join channels
                            "001" => {
                                if !joined {
                                    tracing::info!("IRC registered as {nick}");

                                    // Send NickServ IDENTIFY if password is configured
                                    if let Some(ref password) = config.password {
                                        let identify = format!(
                                            "PRIVMSG NickServ :IDENTIFY {}\r\n",
                                            password
                                        );
                                        if let Err(e) =
                                            writer.write_all(identify.as_bytes()).await
                                        {
                                            tracing::warn!(
                                                "IRC NickServ IDENTIFY failed: {e}"
                                            );
                                            break 'inner true;
                                        }
                                    }

                                    for ch in &channels_to_join {
                                        let join_cmd = format!("JOIN {ch}\r\n");
                                        if let Err(e) =
                                            writer.write_all(join_cmd.as_bytes()).await
                                        {
                                            tracing::warn!("IRC JOIN send failed: {e}");
                                            break 'inner true;
                                        }
                                        tracing::info!("IRC joining {ch}");
                                    }
                                    joined = true;
                                }
                            }

                            // ERR_NICKNAMEINUSE (433) - nickname taken, try alternative
                            "433" => {
                                tracing::warn!("IRC: nickname '{nick}' is already in use");
                                let alt_nick = format!("{nick}_");
                                let cmd = format!("NICK {alt_nick}\r\n");
                                let _ = writer.write_all(cmd.as_bytes()).await;
                            }

                            // PRIVMSG - incoming message
                            "PRIVMSG" => {
                                if let Some(msg) =
                                    convert_privmsg(&parsed, &channel_id, &nick, &config)
                                {
                                    tracing::debug!(
                                        "IRC message from {}: {}",
                                        msg.sender_id.as_str(),
                                        &msg.text[..msg.text.len().min(50)]
                                    );
                                    if inbound_tx.send(msg).await.is_err() {
                                        tracing::error!("IRC: inbound channel closed");
                                        return;
                                    }
                                }
                            }

                            // JOIN confirmation
                            "JOIN" => {
                                if let Some(ref prefix) = parsed.prefix {
                                    let joiner = nick_from_prefix(prefix);
                                    let channel = parsed
                                        .trailing
                                        .as_deref()
                                        .or(parsed.params.first().map(|s| s.as_str()))
                                        .unwrap_or("?");
                                    if joiner.eq_ignore_ascii_case(&nick) {
                                        tracing::info!("IRC joined {channel}");
                                    }
                                }
                            }

                            _ => {
                                // Ignore other commands (NOTICE, MODE, etc.)
                            }
                        }
                    }

                    // Outbound message requests from send()
                    Some(raw_cmd) = write_cmd_rx.recv() => {
                        if let Err(e) = writer.write_all(raw_cmd.as_bytes()).await {
                            tracing::warn!("IRC write failed: {e}");
                            break 'inner true;
                        }
                    }

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!("IRC adapter shutting down");
                            let _ = writer
                                .write_all(b"QUIT :Aleph shutting down\r\n")
                                .await;
                            return;
                        }
                    }
                }
            };

            if !should_reconnect || *shutdown_rx.borrow() {
                break;
            }

            tracing::warn!("IRC: reconnecting in {backoff:?}");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }

        tracing::info!("IRC connection loop stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== IRC Line Parser Tests ====================

    #[test]
    fn test_parse_simple_privmsg() {
        let line =
            parse_irc_line(":nick!user@host PRIVMSG #channel :Hello world").unwrap();
        assert_eq!(line.prefix.as_deref(), Some("nick!user@host"));
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.params, vec!["#channel"]);
        assert_eq!(line.trailing.as_deref(), Some("Hello world"));
    }

    #[test]
    fn test_parse_ping() {
        let line = parse_irc_line("PING :server.example.com").unwrap();
        assert!(line.prefix.is_none());
        assert_eq!(line.command, "PING");
        assert!(line.params.is_empty());
        assert_eq!(line.trailing.as_deref(), Some("server.example.com"));
    }

    #[test]
    fn test_parse_welcome() {
        let line =
            parse_irc_line(":server 001 nick :Welcome to IRC").unwrap();
        assert_eq!(line.prefix.as_deref(), Some("server"));
        assert_eq!(line.command, "001");
        assert_eq!(line.params, vec!["nick"]);
        assert_eq!(line.trailing.as_deref(), Some("Welcome to IRC"));
    }

    #[test]
    fn test_parse_nick_in_use() {
        let line = parse_irc_line(
            ":server 433 * nick :Nickname is already in use",
        )
        .unwrap();
        assert_eq!(line.prefix.as_deref(), Some("server"));
        assert_eq!(line.command, "433");
        assert_eq!(line.params, vec!["*", "nick"]);
        assert_eq!(
            line.trailing.as_deref(),
            Some("Nickname is already in use")
        );
    }

    #[test]
    fn test_parse_no_prefix() {
        let line = parse_irc_line("PING :data").unwrap();
        assert!(line.prefix.is_none());
        assert_eq!(line.command, "PING");
        assert!(line.params.is_empty());
        assert_eq!(line.trailing.as_deref(), Some("data"));
    }

    #[test]
    fn test_parse_no_trailing() {
        let line = parse_irc_line("MODE #channel +o nick").unwrap();
        assert!(line.prefix.is_none());
        assert_eq!(line.command, "MODE");
        assert_eq!(line.params, vec!["#channel", "+o", "nick"]);
        assert!(line.trailing.is_none());
    }

    #[test]
    fn test_parse_empty_line() {
        assert!(parse_irc_line("").is_none());
        assert!(parse_irc_line("   ").is_none());
        assert!(parse_irc_line("\t\n").is_none());
    }

    #[test]
    fn test_parse_join_with_trailing() {
        let line =
            parse_irc_line(":alice!alice@host JOIN :#channel").unwrap();
        assert_eq!(line.prefix.as_deref(), Some("alice!alice@host"));
        assert_eq!(line.command, "JOIN");
        assert!(line.params.is_empty());
        assert_eq!(line.trailing.as_deref(), Some("#channel"));
    }

    #[test]
    fn test_parse_join_without_trailing() {
        let line =
            parse_irc_line(":alice!alice@host JOIN #channel").unwrap();
        assert_eq!(line.command, "JOIN");
        assert_eq!(line.params, vec!["#channel"]);
        assert!(line.trailing.is_none());
    }

    #[test]
    fn test_parse_notice() {
        let line = parse_irc_line(
            ":NickServ!NickServ@services NOTICE bot :This nickname is registered",
        )
        .unwrap();
        assert_eq!(
            line.prefix.as_deref(),
            Some("NickServ!NickServ@services")
        );
        assert_eq!(line.command, "NOTICE");
        assert_eq!(line.params, vec!["bot"]);
        assert_eq!(
            line.trailing.as_deref(),
            Some("This nickname is registered")
        );
    }

    #[test]
    fn test_parse_quit() {
        let line =
            parse_irc_line(":alice!alice@host QUIT :Leaving").unwrap();
        assert_eq!(line.command, "QUIT");
        assert!(line.params.is_empty());
        assert_eq!(line.trailing.as_deref(), Some("Leaving"));
    }

    #[test]
    fn test_parse_privmsg_with_colon_in_trailing() {
        // The trailing may contain additional colons
        let line = parse_irc_line(
            ":nick!user@host PRIVMSG #channel :Hello: how are you? http://example.com",
        )
        .unwrap();
        assert_eq!(
            line.trailing.as_deref(),
            Some("Hello: how are you? http://example.com")
        );
    }

    #[test]
    fn test_parse_line_with_crlf() {
        // Lines may have trailing \r\n which should be trimmed
        let line = parse_irc_line("PING :server\r\n").unwrap();
        assert_eq!(line.command, "PING");
        assert_eq!(line.trailing.as_deref(), Some("server"));
    }

    #[test]
    fn test_parse_multiple_params() {
        let line = parse_irc_line(
            ":server 353 bot = #channel :alice bob charlie",
        )
        .unwrap();
        assert_eq!(line.command, "353");
        assert_eq!(line.params, vec!["bot", "=", "#channel"]);
        assert_eq!(
            line.trailing.as_deref(),
            Some("alice bob charlie")
        );
    }

    #[test]
    fn test_parse_prefix_only_command() {
        // Minimal valid line: just a command
        let line = parse_irc_line("QUIT").unwrap();
        assert!(line.prefix.is_none());
        assert_eq!(line.command, "QUIT");
        assert!(line.params.is_empty());
        assert!(line.trailing.is_none());
    }

    #[test]
    fn test_parse_privmsg_dm() {
        let line = parse_irc_line(
            ":alice!alice@host PRIVMSG bot :Hey, private message",
        )
        .unwrap();
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.params, vec!["bot"]);
        assert_eq!(
            line.trailing.as_deref(),
            Some("Hey, private message")
        );
    }

    #[test]
    fn test_parse_mode_with_prefix() {
        let line = parse_irc_line(
            ":ChanServ!ChanServ@services MODE #channel +o nick",
        )
        .unwrap();
        assert_eq!(
            line.prefix.as_deref(),
            Some("ChanServ!ChanServ@services")
        );
        assert_eq!(line.command, "MODE");
        assert_eq!(line.params, vec!["#channel", "+o", "nick"]);
        assert!(line.trailing.is_none());
    }

    #[test]
    fn test_parse_empty_trailing() {
        let line = parse_irc_line(":nick!user@host PRIVMSG #channel :").unwrap();
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.trailing.as_deref(), Some(""));
    }

    // ==================== nick_from_prefix Tests ====================

    #[test]
    fn test_nick_from_prefix() {
        assert_eq!(nick_from_prefix("nick!user@host"), "nick");
    }

    #[test]
    fn test_nick_from_prefix_no_user() {
        assert_eq!(nick_from_prefix("nick"), "nick");
    }

    #[test]
    fn test_nick_from_prefix_complex() {
        assert_eq!(
            nick_from_prefix("alice!alice@host.example.com"),
            "alice"
        );
    }

    #[test]
    fn test_nick_from_prefix_with_tilde() {
        assert_eq!(nick_from_prefix("bot!~bot@192.168.1.1"), "bot");
    }

    // ==================== convert_privmsg Tests ====================

    fn make_config() -> IrcConfig {
        IrcConfig {
            server: "irc.test.com".to_string(),
            nick: "alephbot".to_string(),
            channels: vec!["#test".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn test_convert_channel_message() {
        let parsed = IrcLine {
            prefix: Some("alice!alice@host".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["#aleph".to_string()],
            trailing: Some("Hello from IRC!".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config).unwrap();

        assert_eq!(msg.channel_id.as_str(), "irc");
        assert_eq!(msg.conversation_id.as_str(), "#aleph");
        assert_eq!(msg.sender_id.as_str(), "alice");
        assert_eq!(msg.sender_name.as_deref(), Some("alice"));
        assert_eq!(msg.text, "Hello from IRC!");
        assert!(msg.is_group);
        assert!(msg.reply_to.is_none());
        assert!(msg.attachments.is_empty());
    }

    #[test]
    fn test_convert_dm_message() {
        let parsed = IrcLine {
            prefix: Some("bob!bob@host".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["alephbot".to_string()],
            trailing: Some("Private message".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config).unwrap();

        assert!(!msg.is_group);
        assert_eq!(msg.conversation_id.as_str(), "bob"); // DM replies go to sender
        assert_eq!(msg.sender_id.as_str(), "bob");
    }

    #[test]
    fn test_skip_own_message() {
        let parsed = IrcLine {
            prefix: Some("alephbot!bot@host".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["#aleph".to_string()],
            trailing: Some("My own message".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_skip_own_message_case_insensitive() {
        let parsed = IrcLine {
            prefix: Some("AlephBot!bot@host".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["#aleph".to_string()],
            trailing: Some("My own message".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_skip_empty_message() {
        let parsed = IrcLine {
            prefix: Some("alice!alice@host".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["#aleph".to_string()],
            trailing: Some("".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_skip_no_trailing() {
        let parsed = IrcLine {
            prefix: Some("alice!alice@host".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["#aleph".to_string()],
            trailing: None,
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_skip_non_privmsg() {
        let parsed = IrcLine {
            prefix: Some("alice!alice@host".to_string()),
            command: "NOTICE".to_string(),
            params: vec!["#aleph".to_string()],
            trailing: Some("This is a notice".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_skip_no_prefix() {
        let parsed = IrcLine {
            prefix: None,
            command: "PRIVMSG".to_string(),
            params: vec!["#aleph".to_string()],
            trailing: Some("No sender".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_ampersand_channel() {
        // '&' channels are local IRC channels
        let parsed = IrcLine {
            prefix: Some("alice!alice@host".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["&local".to_string()],
            trailing: Some("Local channel message".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config).unwrap();

        assert!(msg.is_group);
        assert_eq!(msg.conversation_id.as_str(), "&local");
    }

    #[test]
    fn test_convert_message_with_colons() {
        let parsed = IrcLine {
            prefix: Some("alice!alice@host".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec!["#test".to_string()],
            trailing: Some("Check this: http://example.com:8080/path".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config).unwrap();

        assert_eq!(
            msg.text,
            "Check this: http://example.com:8080/path"
        );
    }

    #[test]
    fn test_convert_no_params() {
        let parsed = IrcLine {
            prefix: Some("alice!alice@host".to_string()),
            command: "PRIVMSG".to_string(),
            params: vec![],
            trailing: Some("Hello".to_string()),
        };

        let channel_id = ChannelId::new("irc");
        let config = make_config();
        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none()); // No target means no valid message
    }
}
