//! IRC Protocol Operations
//!
//! Low-level IRC protocol parsing and event loop implementation.
//! Handles RFC 2812 message format: `[:prefix] COMMAND [params] [:trailing]`
//!
//! Uses `tokio::net::TcpStream` with buffered I/O for raw TCP connections.
//! No external IRC dependencies required.

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, InboundMessageSender, MessageId,
    SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tokio::sync::oneshot;

use super::config::IrcConfig;
use crate::gateway::restart_backoff::RestartBackoff;

/// Maximum length for a single PRIVMSG payload, accounting for the
/// `:nick!user@host PRIVMSG #channel :` prefix overhead (~80 chars conservative).
pub(crate) const MAX_PRIVMSG_PAYLOAD: usize = 400;

/// Maximum length of a single inbound IRC frame.
///
/// RFC 2812 caps a message at 512 bytes; servers routinely send longer frames
/// with extensions, so the limit stays generous but finite: a peer that never
/// terminates a line must not be able to grow the read buffer without bound.
const MAX_IRC_LINE: usize = 8192;

/// Monotonic counter behind the inbound message ID.
///
/// The millisecond timestamp alone is not unique: two messages from the same
/// sender handled within the same millisecond collide, and the Core dedup
/// (`channel_id:message_id`) then drops the second one.
static INBOUND_SEQ: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Queued write completion
// ---------------------------------------------------------------------------

/// Separator between the completion token and the raw command inside a queued
/// write payload. Payloads are internal: the write worker strips the envelope
/// before the command reaches the socket.
const WRITE_TOKEN_SEP: char = '\u{1f}';

/// Completion slots for queued writes: token -> waiter.
type WriteCompletions = HashMap<u64, oneshot::Sender<Result<(), String>>>;

static WRITE_COMPLETIONS: OnceLock<Mutex<WriteCompletions>> = OnceLock::new();
static WRITE_TOKEN_SEQ: AtomicU64 = AtomicU64::new(0);

fn write_completions() -> MutexGuard<'static, WriteCompletions> {
    WRITE_COMPLETIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Drops the completion slot of a write whose waiter went away (cancelled
/// send), so the registry does not grow without bound.
struct WriteCompletionGuard(u64);

impl Drop for WriteCompletionGuard {
    fn drop(&mut self) {
        write_completions().remove(&self.0);
    }
}

/// Register a completion slot for one queued write.
fn register_write() -> (u64, oneshot::Receiver<Result<(), String>>) {
    let token = WRITE_TOKEN_SEQ.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    write_completions().insert(token, tx);
    (token, rx)
}

/// Hand the local write result back to the sender that queued the command.
fn complete_write(token: u64, result: Result<(), String>) {
    if let Some(tx) = write_completions().remove(&token) {
        let _ = tx.send(result);
    }
}

/// Wrap a raw command into the internal queued-write envelope.
fn encode_write(token: u64, raw: &str) -> String {
    format!("{token}{WRITE_TOKEN_SEP}{raw}")
}

/// Split a queued payload back into its completion token and raw command. A
/// payload without an envelope is treated as a raw command.
fn decode_write(payload: &str) -> (Option<u64>, &str) {
    match payload.split_once(WRITE_TOKEN_SEP) {
        Some((token, raw)) => (token.parse().ok(), raw),
        None => (None, payload),
    }
}

/// Fail every queued write that is still waiting for its completion.
///
/// The writer stops on a broken connection, so the callers waiting on those
/// writes must learn about it instead of waiting for a completion that can no
/// longer arrive.
fn fail_pending_writes(reason: &str) {
    let mut completions = write_completions();
    for (_, tx) in completions.drain() {
        let _ = tx.send(Err(reason.to_string()));
    }
}

/// Read inbound frames from the IRC socket.
///
/// The server is a remote peer: a frame without a terminating newline would
/// grow the read buffer without bound, so the buffer is capped and an
/// oversized frame is dropped instead of being accumulated.
async fn read_bounded_lines(
    reader: tokio::net::tcp::OwnedReadHalf,
    tx: tokio::sync::mpsc::Sender<String>,
) {
    use tokio::io::AsyncReadExt;

    let mut reader = tokio::io::BufReader::new(reader);
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 2048];
    let mut overflow = false;

    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("IRC read error: {e}");
                break;
            }
        };
        for &byte in &chunk[..read] {
            if byte == b'\n' {
                if overflow {
                    tracing::warn!("IRC frame longer than {MAX_IRC_LINE} bytes dropped");
                } else {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches('\r')
                        .to_string();
                    if tx.send(line).await.is_err() {
                        return;
                    }
                }
                buf.clear();
                overflow = false;
            } else if buf.len() < MAX_IRC_LINE {
                buf.push(byte);
            } else {
                overflow = true;
            }
        }
    }
}

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
#[must_use]
pub fn nick_from_prefix(prefix: &str) -> &str {
    prefix.split('!').next().unwrap_or(prefix)
}

/// Whether a message text addresses the bot by its nick.
///
/// The text must start with the bot's nick followed by a separator (`:`, `,`
/// or whitespace), so `alephbot: hi` addresses the bot while `alephbotica`
/// does not.
#[must_use]
fn addresses_bot(text: &str, own_nick: &str) -> bool {
    let text = text.trim_start();
    let Some(prefix) = text.get(..own_nick.len()) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case(own_nick) {
        return false;
    }
    let rest = &text[own_nick.len()..];
    rest.starts_with(':') || rest.starts_with(',') || rest.starts_with(char::is_whitespace)
}

/// Convert an IRC PRIVMSG to an `InboundMessage`.
///
/// Returns `None` if:
/// - The command is not PRIVMSG
/// - The message is from the bot itself (case-insensitive comparison)
/// - The message body is empty
/// - No prefix (sender info) is present
/// - The sender is outside `IrcConfig::allowed_users`
/// - A channel message does not address the bot and `require_mention` is set
#[must_use]
pub fn convert_privmsg(
    parsed: &IrcLine,
    channel_id: &ChannelId,
    own_nick: &str,
    config: &IrcConfig,
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

    // Senders outside the allowlist never reach the bot (an empty allowlist
    // allows everyone, which is the pre-existing behaviour).
    if !config.is_user_allowed(sender_nick) {
        return None;
    }

    let target = parsed.params.first()?;
    let text = parsed.trailing.as_deref().unwrap_or("");
    if text.is_empty() {
        return None;
    }

    // Determine if this is a channel message (group) or a DM
    let is_group = target.starts_with('#') || target.starts_with('&');

    // In a channel, only messages that address the bot are accepted when a
    // mention is required — a direct message is addressed by construction.
    if is_group && config.require_mention && !addresses_bot(text, own_nick) {
        return None;
    }

    // For group messages, conversation_id is the channel name.
    // For DMs, conversation_id is the sender's nick (so replies go back to them).
    let conversation_id = if is_group {
        target.to_string()
    } else {
        sender_nick.to_string()
    };

    Some(InboundMessage {
        id: MessageId::new(format!(
            "irc-{}-{}-{}",
            sender_nick,
            Utc::now().timestamp_millis(),
            INBOUND_SEQ.fetch_add(1, Ordering::Relaxed)
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
        metadata: vec![],
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

        // The target sits in the middle of the raw command line: embedded
        // CR/LF would terminate the line and inject a new command, and a
        // space would inject extra parameters (same injection class as the
        // trailing text handled below). Legit IRC targets never contain
        // control characters or spaces.
        let target: String = target
            .chars()
            .filter(|c| !c.is_control() && *c != ' ')
            .collect();
        if target.is_empty() {
            return Err(ChannelError::SendFailed(
                "IRC target is empty after sanitization".to_string(),
            ));
        }

        for chunk in &chunks {
            // IRC messages are strictly single-line: a raw CR/LF inside the
            // trailing parameter terminates the line and lets the remainder be
            // parsed by the server as a new command (line/command injection),
            // and silently corrupts any multi-line reply. Emit one PRIVMSG per
            // line with embedded control characters removed.
            for line in chunk.split(['\r', '\n']) {
                if line.is_empty() {
                    continue;
                }
                let raw = format!("PRIVMSG {target} :{line}\r\n");
                let (token, completion) = register_write();
                // The guard drops the slot when this call is cancelled before
                // the completion arrives.
                let _guard = WriteCompletionGuard(token);
                write_tx
                    .send(encode_write(token, &raw))
                    .await
                    .map_err(|e| {
                        ChannelError::SendFailed(format!("IRC write channel closed: {e}"))
                    })?;
                match completion.await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        return Err(ChannelError::SendFailed(format!("IRC write failed: {e}")));
                    }
                    Err(_) => {
                        return Err(ChannelError::SendFailed(
                            "IRC writer stopped before the command was written".to_string(),
                        ));
                    }
                }
            }
        }

        Ok(SendResult {
            message_id: MessageId::new(format!("irc-sent-{}", Utc::now().timestamp_millis())),
            timestamp: Utc::now(),
        })
    }

    /// Run the IRC connection loop with automatic reconnection.
    ///
    /// This function:
    /// 1. Connects TCP to server:port
    /// 2. Sends NICK + USER registration
    /// 3. Optionally sends `NickServ` IDENTIFY
    /// 4. Reads lines from the server
    /// 5. Handles PING/PONG, 001 (`RPL_WELCOME`), 433 (nick in use), PRIVMSG
    /// 6. Processes outbound messages from the write channel
    /// 7. Reconnects with exponential backoff on disconnection
    /// 8. Sends QUIT on shutdown
    pub async fn run_irc_loop(
        config: IrcConfig,
        channel_id: ChannelId,
        inbound_tx: InboundMessageSender,
        mut write_cmd_rx: tokio::sync::mpsc::Receiver<String>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpStream;

        let mut backoff = RestartBackoff::with_defaults();
        let addr = config.addr();

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            tracing::info!("Connecting to IRC server at {addr}...");

            let stream = match TcpStream::connect(&addr).await {
                Ok(s) => s,
                Err(e) => {
                    if let Some(delay) = backoff.next_delay() {
                        tracing::warn!("IRC connection failed: {e}, retrying in {delay:?}");
                        tokio::time::sleep(delay).await;
                    }
                    continue;
                }
            };

            backoff.reset();
            tracing::info!("IRC connected to {addr}");

            let (reader, mut writer) = stream.into_split();
            let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<String>(64);
            let reader_task = tokio::spawn(async move {
                read_bounded_lines(reader, line_tx).await;
            });

            // Send registration: NICK + USER
            let mut registration = String::new();
            registration.push_str(&format!("NICK {}\r\n", config.nick));
            registration.push_str(&format!(
                "USER {} 0 * :{}\r\n",
                config.nick, config.realname
            ));

            if let Err(e) = writer.write_all(registration.as_bytes()).await {
                if let Some(delay) = backoff.next_delay() {
                    tracing::warn!("IRC registration send failed: {e}, retrying in {delay:?}");
                    tokio::time::sleep(delay).await;
                }
                continue;
            }

            let mut nick = config.nick.clone();
            let channels_to_join = config.channels.clone();
            let mut joined = false;

            // Inner message loop - returns true if we should reconnect
            let should_reconnect = 'inner: loop {
                tokio::select! {
                    line_result = line_rx.recv() => {
                        let line = match line_result {
                            Some(l) => l,
                            None => {
                                tracing::info!("IRC connection closed");
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
                                            "PRIVMSG NickServ :IDENTIFY {password}\r\n"
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
                                // The nickname is taken: move to the next
                                // candidate and keep it as the bot's identity,
                                // otherwise the self-filter and the mention
                                // check keep using the stale name.
                                let alt_nick = format!("{nick}_");
                                tracing::warn!(
                                    "IRC: nickname '{nick}' is already in use, trying '{alt_nick}'"
                                );
                                let cmd = format!("NICK {alt_nick}\r\n");
                                if let Err(e) = writer.write_all(cmd.as_bytes()).await {
                                    tracing::warn!("IRC nickname change failed: {e}");
                                    break 'inner true;
                                }
                                nick = alt_nick;
                            }

                            // PRIVMSG - incoming message
                            "PRIVMSG" => {
                                if let Some(msg) =
                                    convert_privmsg(&parsed, &channel_id, &nick, &config)
                                {
                                    tracing::debug!(
                                        "IRC message from {}: {}",
                                        msg.sender_id.as_str(),
                                        msg.text.chars().take(50).collect::<String>()
                                    );
                                    if inbound_tx.send(msg).is_err() {
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
                    Some(payload) = write_cmd_rx.recv() => {
                        let (token, raw_cmd) = decode_write(&payload);
                        if let Err(e) = writer.write_all(raw_cmd.as_bytes()).await {
                            tracing::warn!("IRC write failed: {e}");
                            if let Some(token) = token {
                                complete_write(token, Err(e.to_string()));
                            }
                            fail_pending_writes("IRC write failed");
                            break 'inner true;
                        }
                        if let Some(token) = token {
                            complete_write(token, Ok(()));
                        }
                    }

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!("IRC adapter shutting down");
                            let _ = writer
                                .write_all(b"QUIT :Aleph shutting down\r\n")
                                .await;
                            fail_pending_writes("IRC adapter shutting down");
                            reader_task.abort();
                            return;
                        }
                    }
                }
            };

            // The reader task owns the socket half; it must not outlive the
            // connection it reads from.
            reader_task.abort();

            if !should_reconnect || *shutdown_rx.borrow() {
                break;
            }

            if let Some(delay) = backoff.next_delay() {
                tracing::warn!("IRC: reconnecting in {delay:?}");
                tokio::time::sleep(delay).await;
            }
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
        let line = parse_irc_line(":nick!user@host PRIVMSG #channel :Hello world").unwrap();
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
        let line = parse_irc_line(":server 001 nick :Welcome to IRC").unwrap();
        assert_eq!(line.prefix.as_deref(), Some("server"));
        assert_eq!(line.command, "001");
        assert_eq!(line.params, vec!["nick"]);
        assert_eq!(line.trailing.as_deref(), Some("Welcome to IRC"));
    }

    #[test]
    fn test_parse_nick_in_use() {
        let line = parse_irc_line(":server 433 * nick :Nickname is already in use").unwrap();
        assert_eq!(line.prefix.as_deref(), Some("server"));
        assert_eq!(line.command, "433");
        assert_eq!(line.params, vec!["*", "nick"]);
        assert_eq!(line.trailing.as_deref(), Some("Nickname is already in use"));
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
        let line = parse_irc_line(":alice!alice@host JOIN :#channel").unwrap();
        assert_eq!(line.prefix.as_deref(), Some("alice!alice@host"));
        assert_eq!(line.command, "JOIN");
        assert!(line.params.is_empty());
        assert_eq!(line.trailing.as_deref(), Some("#channel"));
    }

    #[test]
    fn test_parse_join_without_trailing() {
        let line = parse_irc_line(":alice!alice@host JOIN #channel").unwrap();
        assert_eq!(line.command, "JOIN");
        assert_eq!(line.params, vec!["#channel"]);
        assert!(line.trailing.is_none());
    }

    #[test]
    fn test_parse_notice() {
        let line =
            parse_irc_line(":NickServ!NickServ@services NOTICE bot :This nickname is registered")
                .unwrap();
        assert_eq!(line.prefix.as_deref(), Some("NickServ!NickServ@services"));
        assert_eq!(line.command, "NOTICE");
        assert_eq!(line.params, vec!["bot"]);
        assert_eq!(
            line.trailing.as_deref(),
            Some("This nickname is registered")
        );
    }

    #[test]
    fn test_parse_quit() {
        let line = parse_irc_line(":alice!alice@host QUIT :Leaving").unwrap();
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
        let line = parse_irc_line(":server 353 bot = #channel :alice bob charlie").unwrap();
        assert_eq!(line.command, "353");
        assert_eq!(line.params, vec!["bot", "=", "#channel"]);
        assert_eq!(line.trailing.as_deref(), Some("alice bob charlie"));
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
        let line = parse_irc_line(":alice!alice@host PRIVMSG bot :Hey, private message").unwrap();
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.params, vec!["bot"]);
        assert_eq!(line.trailing.as_deref(), Some("Hey, private message"));
    }

    #[test]
    fn test_parse_mode_with_prefix() {
        let line = parse_irc_line(":ChanServ!ChanServ@services MODE #channel +o nick").unwrap();
        assert_eq!(line.prefix.as_deref(), Some("ChanServ!ChanServ@services"));
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
        assert_eq!(nick_from_prefix("alice!alice@host.example.com"), "alice");
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

        assert_eq!(msg.text, "Check this: http://example.com:8080/path");
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

    // ==================== Allowlist and Mention Tests ====================

    fn privmsg(sender: &str, target: &str, text: &str) -> IrcLine {
        IrcLine {
            prefix: Some(format!("{sender}!{sender}@host")),
            command: "PRIVMSG".to_string(),
            params: vec![target.to_string()],
            trailing: Some(text.to_string()),
        }
    }

    #[test]
    fn test_sender_outside_the_allowlist_is_dropped() {
        let parsed = privmsg("mallory", "#test", "hello");
        let channel_id = ChannelId::new("irc");
        let config = IrcConfig {
            allowed_users: vec!["alice".to_string()],
            ..make_config()
        };

        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_sender_inside_the_allowlist_is_accepted() {
        let parsed = privmsg("alice", "#test", "hello");
        let channel_id = ChannelId::new("irc");
        let config = IrcConfig {
            allowed_users: vec!["Alice".to_string()],
            ..make_config()
        };

        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config).unwrap();
        assert_eq!(msg.sender_id.as_str(), "alice");
    }

    #[test]
    fn test_a_dm_from_an_unknown_nick_is_dropped() {
        let parsed = privmsg("mallory", "alephbot", "hello");
        let channel_id = ChannelId::new("irc");
        let config = IrcConfig {
            allowed_users: vec!["alice".to_string()],
            ..make_config()
        };

        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_a_channel_message_without_a_mention_is_dropped() {
        let parsed = privmsg("alice", "#test", "just chatting");
        let channel_id = ChannelId::new("irc");
        let config = IrcConfig {
            require_mention: true,
            ..make_config()
        };

        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_a_channel_message_addressing_the_bot_is_accepted() {
        let parsed = privmsg("alice", "#test", "alephbot: hello there");
        let channel_id = ChannelId::new("irc");
        let config = IrcConfig {
            require_mention: true,
            ..make_config()
        };

        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config).unwrap();
        assert_eq!(msg.text, "alephbot: hello there");
    }

    #[test]
    fn test_a_nick_prefix_is_not_a_mention() {
        let parsed = privmsg("alice", "#test", "alephbotica hello");
        let channel_id = ChannelId::new("irc");
        let config = IrcConfig {
            require_mention: true,
            ..make_config()
        };

        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_a_direct_message_needs_no_mention() {
        let parsed = privmsg("alice", "alephbot", "hello");
        let channel_id = ChannelId::new("irc");
        let config = IrcConfig {
            require_mention: true,
            ..make_config()
        };

        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config).unwrap();
        assert!(!msg.is_group);
    }

    #[test]
    fn test_an_empty_allowlist_allows_every_sender() {
        let parsed = privmsg("mallory", "#test", "hello");
        let channel_id = ChannelId::new("irc");
        let config = make_config();

        let msg = convert_privmsg(&parsed, &channel_id, "alephbot", &config).unwrap();
        assert_eq!(msg.sender_id.as_str(), "mallory");
    }
}
