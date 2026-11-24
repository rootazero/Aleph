//! Email Operations
//!
//! Low-level functions for IMAP polling and SMTP sending.
//! Separated from the channel struct for testability.

use crate::gateway::channel::{
    ChannelError, ChannelId, ChannelResult, ConversationId, InboundMessage, InboundMessageSender,
    MessageId, SendResult, UserId,
};
use chrono::Utc;
use std::collections::HashMap;
use std::time::Duration;

use super::config::EmailConfig;

const INITIAL_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
const MAX_IMAP_BATCH_MESSAGES: usize = 32;
const MAX_IMAP_MESSAGE_BYTES: u32 = 1024 * 1024;
/// Upper bound on conversations whose inbound subject is remembered.
const MAX_TRACKED_SUBJECTS: usize = 512;

/// The last inbound mail seen from one conversation address.
///
/// Enough to answer inside the sender's mail thread: the subject line and the
/// RFC 5322 `Message-ID` the reply points back at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundThread {
    /// Subject line of the inbound mail (may be empty).
    pub subject: String,
    /// RFC 5322 `Message-ID` of the inbound mail.
    pub message_id: String,
}

/// Per-conversation inbound mail identity, keyed by sender address.
///
/// Written by the IMAP poll loop and read by `EmailChannel::send`, so a reply
/// leaves as `Re: <subject>` with `In-Reply-To`/`References` pointing at the
/// mail it answers. Best effort: past [`MAX_TRACKED_SUBJECTS`] an arbitrary
/// entry is evicted — losing one only loses threading, never a message.
pub type InboundThreadStore = std::sync::Arc<std::sync::Mutex<HashMap<String, InboundThread>>>;

/// Authenticated IMAP session over TLS.
type ImapSession = async_imap::Session<
    async_native_tls::TlsStream<tokio_util::compat::Compat<tokio::net::TcpStream>>,
>;

/// Create an empty inbound thread store.
#[must_use]
pub fn new_inbound_thread_store() -> InboundThreadStore {
    std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))
}

/// Remember the last inbound mail from `conversation`.
pub fn remember_inbound_thread(
    store: &InboundThreadStore,
    conversation: &str,
    subject: &str,
    message_id: &str,
) {
    let message_id = message_id.trim();
    if message_id.is_empty() {
        return;
    }
    let mut map = match store.lock() {
        Ok(map) => map,
        Err(poisoned) => poisoned.into_inner(),
    };
    if map.len() >= MAX_TRACKED_SUBJECTS && !map.contains_key(conversation) {
        if let Some(stale) = map.keys().next().cloned() {
            map.remove(&stale);
        }
    }
    map.insert(
        conversation.to_string(),
        InboundThread {
            subject: subject.trim().to_string(),
            message_id: message_id.to_string(),
        },
    );
}

/// Subject to use when replying into `conversation`.
///
/// Returns `None` when no inbound mail from that conversation carried a
/// subject; otherwise the remembered subject, prefixed with `Re: ` unless it
/// already carries a reply prefix.
#[must_use]
pub fn reply_subject_for(store: &InboundThreadStore, conversation: &str) -> Option<String> {
    let map = match store.lock() {
        Ok(map) => map,
        Err(poisoned) => poisoned.into_inner(),
    };
    let thread = map.get(conversation)?;
    if thread.subject.is_empty() {
        return None;
    }
    Some(if has_reply_prefix(&thread.subject) {
        thread.subject.clone()
    } else {
        format!("Re: {}", thread.subject)
    })
}

/// `Message-ID` to answer when replying into `conversation`.
///
/// Returns `None` when no inbound mail from that conversation has been seen
/// yet; otherwise the remembered identifier, so the reply threads under it.
#[must_use]
pub fn reply_message_id_for(store: &InboundThreadStore, conversation: &str) -> Option<String> {
    let map = match store.lock() {
        Ok(map) => map,
        Err(poisoned) => poisoned.into_inner(),
    };
    map.get(conversation)
        .map(|thread| thread.message_id.clone())
}

fn has_reply_prefix(subject: &str) -> bool {
    subject
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("re:"))
}

/// Wrap a message identifier in the angle brackets RFC 5322 requires.
///
/// Mail parsers hand back the identifier without them often enough that
/// callers must not assume either shape.
fn rfc_message_id(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        trimmed.to_string()
    } else {
        format!("<{trimmed}>")
    }
}

/// Email message operations helper.
///
/// Provides methods for IMAP polling and SMTP sending.
pub struct EmailMessageOps;

impl EmailMessageOps {
    /// Parse email body text from raw email bytes.
    ///
    /// Prefers plain text part over HTML. Falls back to extracting
    /// text from HTML if no plain text part is available.
    #[must_use]
    pub fn extract_body_text(raw_email: &[u8]) -> String {
        use mail_parser::MessageParser;

        let message = match MessageParser::default().parse(raw_email) {
            Some(msg) => msg,
            None => return String::new(),
        };

        // Prefer plain text body
        if let Some(text) = message.body_text(0) {
            return text.to_string();
        }

        // Fall back to HTML body, stripped of tags
        if let Some(html) = message.body_html(0) {
            return Self::strip_html_tags(&html);
        }

        String::new()
    }

    /// Extract sender email address from raw email bytes.
    #[must_use]
    pub fn extract_sender(raw_email: &[u8]) -> Option<String> {
        use mail_parser::MessageParser;

        let message = MessageParser::default().parse(raw_email)?;
        let from = message.from()?;
        from.first()
            .and_then(|addr| addr.address())
            .map(|s| s.to_string())
    }

    /// Extract subject from raw email bytes.
    #[must_use]
    pub fn extract_subject(raw_email: &[u8]) -> Option<String> {
        use mail_parser::MessageParser;

        let message = MessageParser::default().parse(raw_email)?;
        message.subject().map(|s| s.to_string())
    }

    /// Extract Message-ID from raw email bytes.
    #[must_use]
    pub fn extract_message_id(raw_email: &[u8]) -> Option<String> {
        use mail_parser::MessageParser;

        let message = MessageParser::default().parse(raw_email)?;
        message.message_id().map(|s| s.to_string())
    }

    /// Count MIME parts the parser classifies as attachments.
    ///
    /// The channel has no attachment delivery path, so the count is used to
    /// tell the model that something was attached instead of dropping it
    /// silently.
    #[must_use]
    pub fn extract_attachment_count(raw_email: &[u8]) -> usize {
        use mail_parser::MessageParser;

        MessageParser::default()
            .parse(raw_email)
            .map_or(0, |message| message.attachments().count())
    }

    /// Build a simple HTML email body from Markdown text.
    ///
    /// Wraps the Markdown content in a minimal HTML template.
    #[must_use]
    pub fn markdown_to_html_email(markdown: &str) -> String {
        // Simple Markdown-to-HTML conversion for email
        // Escape untrusted text and destinations before inserting our fixed tags.
        let mut html = Self::escape_html(markdown);

        // Convert code blocks first (before inline conversions).
        // Use a placeholder to protect code blocks from newline conversion.
        let mut code_blocks: Vec<String> = Vec::new();
        html = Self::extract_code_blocks(&html, &mut code_blocks);
        html = Self::extract_inline_code(&html, &mut code_blocks);

        // Bold: **text** -> <strong>text</strong>
        html = Self::convert_paired_marker(&html, "**", "<strong>", "</strong>");

        // Italic: *text* -> <em>text</em>
        html = Self::convert_single_asterisk(&html, "<em>", "</em>");

        // Links: [text](url) -> <a href="url">text</a>
        html = Self::convert_links(&html);

        // Line breaks (only outside code blocks, which are now placeholders)
        html = html.replace("\n\n", "</p><p>");
        html = html.replace('\n', "<br>");

        // Restore code blocks from placeholders
        for (i, block) in code_blocks.iter().enumerate() {
            html = html.replace(&format!("\x00CODE_BLOCK_{i}\x00"), block);
        }
        format!(
            r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"></head>
<body style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; line-height: 1.6; color: #333; max-width: 600px; margin: 0 auto; padding: 20px;">
<p>{html}</p>
</body>
</html>"#
        )
    }

    /// Send email via SMTP using lettre.
    ///
    /// `in_reply_to` is the RFC 5322 `Message-ID` of the inbound mail being
    /// answered. Setting it (plus `References`) is what makes the receiving
    /// mail client thread the answer under the original message.
    pub async fn send_email(
        config: &EmailConfig,
        to: &str,
        subject: &str,
        body: &str,
        in_reply_to: Option<&str>,
    ) -> Result<SendResult, ChannelError> {
        use lettre::message::header::ContentType;
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

        let html_body = Self::markdown_to_html_email(body);

        let mut builder = Message::builder()
            .from(
                config
                    .from_address
                    .parse()
                    .map_err(|e| ChannelError::ConfigError(format!("Invalid from address: {e}")))?,
            )
            .to(to
                .parse()
                .map_err(|e| ChannelError::SendFailed(format!("Invalid recipient address: {e}")))?)
            .subject(subject);

        if let Some(in_reply_to) = in_reply_to.map(rfc_message_id) {
            builder = builder
                .in_reply_to(in_reply_to.clone())
                .references(in_reply_to);
        }

        let email = builder
            .header(ContentType::TEXT_HTML)
            .body(html_body)
            .map_err(|e| ChannelError::SendFailed(format!("Failed to build email: {e}")))?;

        let creds = Credentials::new(config.username.clone(), config.password.clone());

        // `EmailConfig::validate` rejects `use_tls = false`, so STARTTLS is
        // the only transport this channel ever builds.
        let mailer = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)
            .map_err(|e| ChannelError::SendFailed(format!("Failed to create SMTP transport: {e}")))?
            .port(config.smtp_port)
            .credentials(creds)
            .build();

        let response = mailer
            .send(email)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("SMTP send failed: {e}")))?;

        // Use first line of SMTP response as message ID
        let msg_id = response
            .message()
            .flat_map(|s| s.lines())
            .next()
            .unwrap_or("sent")
            .to_string();

        Ok(SendResult {
            message_id: MessageId::new(msg_id),
            timestamp: Utc::now(),
        })
    }

    /// Run the IMAP polling loop.
    ///
    /// Spawned as a tokio task. Connects to the IMAP server, polls for unseen
    /// messages, converts them to `InboundMessages`, and marks them as seen.
    ///
    /// Returns `Ok(())` when shutdown was requested and `Err` when the channel
    /// hit a failure that retrying cannot fix (bad config, rejected login), so
    /// the caller can publish `ChannelStatus::Error` instead of leaving the
    /// channel looking healthy.
    pub async fn run_imap_poll_loop(
        config: EmailConfig,
        inbound_tx: InboundMessageSender,
        channel_id: ChannelId,
        threads: InboundThreadStore,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> ChannelResult<()> {
        let poll_interval = Duration::from_secs(config.poll_interval_secs);
        let mut backoff = INITIAL_BACKOFF;

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            match Self::poll_imap_once(&config, &inbound_tx, &channel_id, &threads).await {
                Ok(count) => {
                    if count > 0 {
                        tracing::info!("Email: processed {count} new messages");
                    }
                    backoff = INITIAL_BACKOFF; // Reset backoff on success
                }
                Err(e) => {
                    if Self::is_fatal_imap_error(&e) {
                        tracing::error!("Email IMAP poll failed fatally: {e}");
                        return Err(e);
                    }
                    tracing::warn!("Email IMAP poll error: {e}, retrying in {backoff:?}");
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {},
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
                        }
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            }

            // Wait for next poll cycle or shutdown
            tokio::select! {
                _ = tokio::time::sleep(poll_interval) => {},
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }

        tracing::info!("Email IMAP poll loop stopped");
        Ok(())
    }

    /// Errors that retrying cannot fix: bad configuration or rejected credentials.
    fn is_fatal_imap_error(error: &ChannelError) -> bool {
        matches!(
            error,
            ChannelError::ConfigError(_) | ChannelError::AuthFailed(_)
        )
    }

    /// Verify IMAP reachability and credentials without polling for mail.
    ///
    /// Called from `EmailChannel::start` so the channel only reports
    /// `Connected` after it has actually reached the server, and a rejected
    /// login surfaces as an error instead of as a warning inside a poll loop
    /// nobody is watching.
    pub async fn check_imap_connection(config: &EmailConfig) -> ChannelResult<()> {
        let mut session = Self::connect_imap(config).await?;
        let _ = session.logout().await;
        Ok(())
    }

    /// Open a TLS IMAP connection and authenticate.
    async fn connect_imap(config: &EmailConfig) -> Result<ImapSession, ChannelError> {
        use tokio_util::compat::TokioAsyncReadCompatExt;

        let tcp_stream =
            tokio::net::TcpStream::connect((config.imap_host.as_str(), config.imap_port))
                .await
                .map_err(|e| ChannelError::Internal(format!("IMAP TCP connect failed: {e}")))?;
        // Wrap tokio TcpStream with compat layer so it implements futures::AsyncRead/Write
        let tls = async_native_tls::TlsConnector::new();
        let tls_stream = tls
            .connect(&config.imap_host, tcp_stream.compat())
            .await
            .map_err(|e| ChannelError::Internal(format!("IMAP TLS connect failed: {e}")))?;

        let client = async_imap::Client::new(tls_stream);
        client
            .login(&config.username, &config.password)
            .await
            .map_err(|(e, _)| ChannelError::AuthFailed(format!("IMAP login failed: {e}")))
    }

    /// Perform a single IMAP poll cycle.
    ///
    /// Connects to the IMAP server, searches for unseen messages in each
    /// configured folder, fetches and parses them, then marks them as seen.
    async fn poll_imap_once(
        config: &EmailConfig,
        inbound_tx: &InboundMessageSender,
        channel_id: &ChannelId,
        threads: &InboundThreadStore,
    ) -> Result<usize, ChannelError> {
        let mut session = Self::connect_imap(config).await?;

        let mut total_processed = 0;

        for folder in &config.folders {
            // Select the folder
            let mailbox = session
                .select(folder)
                .await
                .map_err(|e| ChannelError::Internal(format!("IMAP SELECT {folder} failed: {e}")))?;

            // Bound SEARCH metadata as well as FETCH bodies with sequence windows.
            for first in (1..=mailbox.exists).step_by(MAX_IMAP_BATCH_MESSAGES) {
                let last = first
                    .saturating_add(MAX_IMAP_BATCH_MESSAGES as u32 - 1)
                    .min(mailbox.exists);
                let search_result = session
                    .uid_search(format!(
                        "{first}:{last} UNSEEN SMALLER {}",
                        MAX_IMAP_MESSAGE_BYTES + 1
                    ))
                    .await
                    .map_err(|e| ChannelError::Internal(format!("IMAP SEARCH failed: {e}")))?;
                if search_result.is_empty() {
                    continue;
                }
                let uid_set = Self::imap_uid_batch(search_result);

                // PEEK preserves UNSEEN until publication; partial FETCH bounds
                // allocation even if a message grows after SEARCH.
                let mut messages_stream = session
                    .uid_fetch(
                        &uid_set,
                        format!(
                            "(BODY.PEEK[]<0.{}> UID RFC822.SIZE)",
                            MAX_IMAP_MESSAGE_BYTES + 1
                        ),
                    )
                    .await
                    .map_err(|e| ChannelError::Internal(format!("IMAP FETCH failed: {e}")))?;

                use futures::StreamExt;
                let mut published_uids: Vec<String> = Vec::new();
                let mut fetch_error: Option<String> = None;
                while let Some(result) = messages_stream.next().await {
                    let fetch = match result {
                        Ok(fetch) => fetch,
                        Err(e) => {
                            // A FETCH error means the message could not be read
                            // at all. It stays UNSEEN, so the next poll retries
                            // it — but this batch is incomplete, and the caller
                            // has to learn that: counting a partial batch as a
                            // complete one is how a dropped message becomes a
                            // silent one.
                            tracing::warn!("Email: IMAP fetch error: {e}");
                            fetch_error = Some(e.to_string());
                            break;
                        }
                    };
                    let body = match fetch.body() {
                        Some(body) if Self::is_complete_imap_body(body, fetch.size) => body,
                        _ => continue,
                    };

                    // Parse the email
                    let sender = match Self::extract_sender(body) {
                        Some(s) => s,
                        None => {
                            tracing::warn!("Email: could not extract sender");
                            continue;
                        }
                    };

                    // Check sender allowlist
                    if !config.is_sender_allowed(&sender) {
                        tracing::debug!("Email: sender {sender} not in allowed list, skipping");
                        continue;
                    }

                    let subject = Self::extract_subject(body).unwrap_or_default();
                    let attachment_count = Self::extract_attachment_count(body);
                    let text = Self::extract_body_text(body);
                    let message_id = Self::extract_message_id(body)
                        .unwrap_or_else(|| format!("email-{}", uuid::Uuid::new_v4()));

                    if text.is_empty() && attachment_count == 0 {
                        continue;
                    }

                    // Remember subject and Message-ID so the reply to this
                    // sender threads under the same mail-client conversation.
                    remember_inbound_thread(threads, &sender, &subject, &message_id);

                    let text =
                        crate::gateway::interfaces::with_attachment_note(text, attachment_count);

                    let inbound = InboundMessage {
                        id: MessageId::new(&message_id),
                        channel_id: channel_id.clone(),
                        conversation_id: ConversationId::new(sender.clone()),
                        sender_id: UserId::new(sender.clone()),
                        sender_name: Some(sender.clone()),
                        text,
                        attachments: Vec::new(), // inbound attachments are reported in `text`
                        timestamp: Utc::now(),
                        reply_to: None,
                        is_group: false,
                        raw: Some(serde_json::json!({
                            "subject": subject,
                            "from": sender,
                            "message_id": message_id,
                            "folder": folder,
                        })),
                        metadata: vec![],
                    };

                    if inbound_tx.send(inbound).is_err() {
                        tracing::error!("Email: inbound channel closed");
                        // Drain this bounded command before issuing STORE on the session.
                        // Unpublished bodies remain UNSEEN.
                        while messages_stream.next().await.is_some() {}
                        break;
                    }

                    if let Some(uid) = fetch.uid {
                        published_uids.push(uid.to_string());
                    }

                    total_processed += 1;
                }

                drop(messages_stream);

                // Mark ONLY the messages that were actually published as seen.
                // Marking the whole search set would hide messages that were
                // never delivered (fetch error, allowlist skip, closed
                // receiver) — the next poll only searches UNSEEN, so those would
                // never be seen again.
                if !published_uids.is_empty() {
                    let published_set = published_uids.join(",");
                    if let Ok(mut store_stream) =
                        session.uid_store(&published_set, "+FLAGS (\\Seen)").await
                    {
                        // Consume the stream to apply the flag changes
                        while store_stream.next().await.is_some() {}
                    }
                }

                if let Some(err) = fetch_error {
                    return Err(ChannelError::Internal(format!(
                        "IMAP FETCH failed after {total_processed} published messages: {err}"
                    )));
                }
            }
        }

        // Logout cleanly
        let _ = session.logout().await;

        Ok(total_processed)
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    fn imap_uid_batch(uids: std::collections::HashSet<u32>) -> String {
        let mut uids: Vec<_> = uids.into_iter().collect();
        uids.sort_unstable();
        uids.into_iter()
            .take(MAX_IMAP_BATCH_MESSAGES)
            .map(|uid| uid.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    fn is_complete_imap_body(body: &[u8], size: Option<u32>) -> bool {
        body.len() <= MAX_IMAP_MESSAGE_BYTES as usize
            && size.is_some_and(|size| size as usize == body.len())
    }

    /// Strip HTML tags from a string (simple implementation).
    fn strip_html_tags(html: &str) -> String {
        let mut result = String::with_capacity(html.len());
        let mut in_tag = false;

        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => result.push(ch),
                _ => {}
            }
        }

        // Decode common HTML entities
        result
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&nbsp;", " ")
    }

    fn escape_html(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }

    // Input is already escaped; placeholders keep Markdown inside code literal.
    fn extract_inline_code(text: &str, blocks: &mut Vec<String>) -> String {
        let mut result = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find('`') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('`') else {
                break;
            };
            result.push_str(&rest[..start]);
            let idx = blocks.len();
            blocks.push(format!("<code>{}</code>", &after[..end]));
            result.push_str(&format!("\x00CODE_BLOCK_{idx}\x00"));
            rest = &after[end + 1..];
        }
        result.push_str(rest);
        result
    }

    /// Extract fenced code blocks, replace with placeholders, store HTML in `blocks`.
    fn extract_code_blocks(text: &str, blocks: &mut Vec<String>) -> String {
        let mut result = String::with_capacity(text.len());
        let mut rest = text;

        while let Some(fence_start) = rest.find("```") {
            result.push_str(&rest[..fence_start]);
            let after_fence = &rest[fence_start + 3..];

            // Skip optional language tag
            let code_start = after_fence.find('\n').map_or(0, |n| n + 1);
            let code_body = &after_fence[code_start..];

            if let Some(close) = code_body.find("```") {
                let code = &code_body[..close];
                let html_block = format!(
                    "<pre><code>{}</code></pre>",
                    code.replace('<', "&lt;").replace('>', "&gt;")
                );
                let idx = blocks.len();
                blocks.push(html_block);
                result.push_str(&format!("\x00CODE_BLOCK_{idx}\x00"));
                rest = &code_body[close + 3..];
            } else {
                // Unclosed fence
                let html_block = format!(
                    "<pre><code>{}</code></pre>",
                    code_body.replace('<', "&lt;").replace('>', "&gt;")
                );
                let idx = blocks.len();
                blocks.push(html_block);
                result.push_str(&format!("\x00CODE_BLOCK_{idx}\x00"));
                rest = "";
                break;
            }
        }

        result.push_str(rest);
        result
    }

    /// Convert fenced code blocks to HTML <pre><code>.
    #[cfg(test)]
    fn convert_code_blocks(text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut rest = text;

        while let Some(fence_start) = rest.find("```") {
            result.push_str(&rest[..fence_start]);
            let after_fence = &rest[fence_start + 3..];

            // Skip optional language tag
            let code_start = after_fence.find('\n').map(|n| n + 1).unwrap_or(0);
            let code_body = &after_fence[code_start..];

            if let Some(close) = code_body.find("```") {
                let code = &code_body[..close];
                result.push_str("<pre><code>");
                result.push_str(&code.replace('<', "&lt;").replace('>', "&gt;"));
                result.push_str("</code></pre>");
                rest = &code_body[close + 3..];
            } else {
                // Unclosed fence
                result.push_str("<pre><code>");
                result.push_str(&code_body.replace('<', "&lt;").replace('>', "&gt;"));
                result.push_str("</code></pre>");
                rest = "";
                break;
            }
        }

        result.push_str(rest);
        result
    }

    /// Convert paired markers (like ** or `) to HTML tags.
    fn convert_paired_marker(text: &str, marker: &str, open: &str, close: &str) -> String {
        let mut result = text.to_string();

        while let Some(start) = result.find(marker) {
            let after_start = start + marker.len();
            if after_start >= result.len() {
                break;
            }
            if let Some(rel_end) = result[after_start..].find(marker) {
                let end = after_start + rel_end;
                let inner = &result[after_start..end];
                result = format!(
                    "{}{}{}{}{}",
                    &result[..start],
                    open,
                    inner,
                    close,
                    &result[end + marker.len()..]
                );
            } else {
                break;
            }
        }

        result
    }

    /// Convert single asterisk italic markers to HTML.
    fn convert_single_asterisk(text: &str, open: &str, close: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut in_italic = false;

        for (i, &ch) in chars.iter().enumerate() {
            if ch == '*'
                && (i == 0 || chars[i - 1] != '*')
                && (i + 1 >= chars.len() || chars[i + 1] != '*')
            {
                if in_italic {
                    out.push_str(close);
                } else {
                    out.push_str(open);
                }
                in_italic = !in_italic;
            } else {
                out.push(ch);
            }
        }

        out
    }

    /// Convert Markdown links to HTML <a> tags.
    fn convert_links(text: &str) -> String {
        let mut result = text.to_string();

        loop {
            if let Some(bracket_start) = result.find('[') {
                if let Some(rel_bracket_end) = result[bracket_start..].find("](") {
                    let bracket_end = bracket_start + rel_bracket_end;
                    if let Some(rel_paren_end) = result[bracket_end + 2..].find(')') {
                        let paren_end = bracket_end + 2 + rel_paren_end;
                        let link_text = &result[bracket_start + 1..bracket_end];
                        let url = &result[bracket_end + 2..paren_end];
                        let replacement = format!("<a href=\"{url}\">{link_text}</a>");
                        result = format!(
                            "{}{}{}",
                            &result[..bracket_start],
                            replacement,
                            &result[paren_end + 1..]
                        );
                        continue;
                    }
                }
            }
            break;
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Reply threading
    // ------------------------------------------------------------------

    #[test]
    fn test_inbound_thread_roundtrip_threads_reply() {
        let store = new_inbound_thread_store();
        assert_eq!(reply_subject_for(&store, "a@example.com"), None);
        assert_eq!(reply_message_id_for(&store, "a@example.com"), None);

        remember_inbound_thread(&store, "a@example.com", "  Fix the bug  ", "<id-1@mail>");
        assert_eq!(
            reply_subject_for(&store, "a@example.com"),
            Some("Re: Fix the bug".to_string())
        );
        assert_eq!(
            reply_message_id_for(&store, "a@example.com").as_deref(),
            Some("<id-1@mail>")
        );

        // Already a reply: the prefix is not stacked again.
        remember_inbound_thread(&store, "a@example.com", "Re: Fix the bug", "<id-2@mail>");
        assert_eq!(
            reply_subject_for(&store, "a@example.com"),
            Some("Re: Fix the bug".to_string())
        );

        // A blank subject still keeps the identity, just no subject to repeat.
        remember_inbound_thread(&store, "b@example.com", "   ", "<id-3@mail>");
        assert_eq!(reply_subject_for(&store, "b@example.com"), None);
        assert_eq!(
            reply_message_id_for(&store, "b@example.com").as_deref(),
            Some("<id-3@mail>")
        );
    }

    #[test]
    fn test_rfc_message_id_wraps_once() {
        assert_eq!(rfc_message_id("abc@example.com"), "<abc@example.com>");
        assert_eq!(rfc_message_id(" <abc@example.com> "), "<abc@example.com>");
    }

    // ------------------------------------------------------------------
    // HTML stripping
    // ------------------------------------------------------------------

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(
            EmailMessageOps::strip_html_tags("<p>Hello <b>world</b></p>"),
            "Hello world"
        );
    }

    #[test]
    fn test_strip_html_entities() {
        assert_eq!(
            EmailMessageOps::strip_html_tags("1 &lt; 2 &amp; 3 &gt; 0"),
            "1 < 2 & 3 > 0"
        );
    }

    #[test]
    fn test_strip_html_nested() {
        assert_eq!(
            EmailMessageOps::strip_html_tags("<div><p>Nested <em>content</em></p></div>"),
            "Nested content"
        );
    }

    // ------------------------------------------------------------------
    // Markdown to HTML email
    // ------------------------------------------------------------------

    #[test]
    fn markdown_escapes_text_code_and_link_attributes() {
        let result = EmailMessageOps::markdown_to_html_email(
            "Use List<T> & <img src=x> `**<tag>**`\n\n[link](https://example.com/?a=\"quoted\"&b=1)\n\n```\n<tag>&amp;\nsecond line\n```",
        );
        assert!(result.contains("List&lt;T&gt; &amp; &lt;img src=x&gt;"));
        assert!(result.contains("<code>**&lt;tag&gt;**</code>"));
        assert!(!result.contains("<img"));
        assert!(result.contains("href=\"https://example.com/?a=&quot;quoted&quot;&amp;b=1\""));
        assert!(result.contains("&lt;tag&gt;&amp;amp;\nsecond line\n</code>"));
    }

    #[test]
    fn imap_batch_and_body_limits_exclude_oversize_and_truncated_mail() {
        let batch = EmailMessageOps::imap_uid_batch((1..=100).collect());
        assert_eq!(batch.split(',').count(), MAX_IMAP_BATCH_MESSAGES);
        assert!(batch.ends_with(",32"));
        let body = vec![b'x'; MAX_IMAP_MESSAGE_BYTES as usize];
        assert!(EmailMessageOps::is_complete_imap_body(
            &body,
            Some(MAX_IMAP_MESSAGE_BYTES)
        ));
        assert!(!EmailMessageOps::is_complete_imap_body(
            &body,
            Some(MAX_IMAP_MESSAGE_BYTES + 1)
        ));
        assert!(!EmailMessageOps::is_complete_imap_body(&body, None));
        let oversize = vec![b'x'; MAX_IMAP_MESSAGE_BYTES as usize + 1];
        assert!(!EmailMessageOps::is_complete_imap_body(
            &oversize,
            Some(MAX_IMAP_MESSAGE_BYTES + 1)
        ));
    }

    #[test]
    fn test_markdown_to_html_bold() {
        let result = EmailMessageOps::markdown_to_html_email("**bold**");
        assert!(result.contains("<strong>bold</strong>"));
    }

    #[test]
    fn test_markdown_to_html_italic() {
        let result = EmailMessageOps::markdown_to_html_email("*italic*");
        assert!(result.contains("<em>italic</em>"));
    }

    #[test]
    fn test_markdown_to_html_code() {
        let result = EmailMessageOps::markdown_to_html_email("`code`");
        assert!(result.contains("<code>code</code>"));
    }

    #[test]
    fn test_markdown_to_html_link() {
        let result = EmailMessageOps::markdown_to_html_email("[click](https://example.com)");
        assert!(result.contains("<a href=\"https://example.com\">click</a>"));
    }

    #[test]
    fn test_markdown_to_html_code_block() {
        let result = EmailMessageOps::markdown_to_html_email("```\nlet x = 1;\n```");
        assert!(result.contains("<pre><code>let x = 1;\n</code></pre>"));
    }

    #[test]
    fn test_markdown_to_html_paragraphs() {
        let result = EmailMessageOps::markdown_to_html_email("Para 1\n\nPara 2");
        assert!(result.contains("</p><p>"));
    }

    #[test]
    fn test_markdown_to_html_line_breaks() {
        let result = EmailMessageOps::markdown_to_html_email("Line 1\nLine 2");
        assert!(result.contains("<br>"));
    }

    #[test]
    fn test_markdown_to_html_wraps_in_html() {
        let result = EmailMessageOps::markdown_to_html_email("Hello");
        assert!(result.contains("<!DOCTYPE html>"));
        assert!(result.contains("<html>"));
        assert!(result.contains("</html>"));
        assert!(result.contains("font-family"));
    }

    // ------------------------------------------------------------------
    // Code block conversion
    // ------------------------------------------------------------------

    #[test]
    fn test_convert_code_blocks_with_language() {
        let result =
            EmailMessageOps::convert_code_blocks("before\n```rust\nlet x = 1;\n```\nafter");
        assert!(result.contains("<pre><code>let x = 1;\n</code></pre>"));
        assert!(result.contains("before"));
        assert!(result.contains("after"));
    }

    #[test]
    fn test_convert_code_blocks_html_escape() {
        let result = EmailMessageOps::convert_code_blocks("```\n<div>test</div>\n```");
        assert!(result.contains("&lt;div&gt;test&lt;/div&gt;"));
    }

    // ------------------------------------------------------------------
    // Link conversion
    // ------------------------------------------------------------------

    #[test]
    fn test_convert_links() {
        assert_eq!(
            EmailMessageOps::convert_links("[text](https://example.com)"),
            "<a href=\"https://example.com\">text</a>"
        );
    }

    #[test]
    fn test_convert_links_multiple() {
        let result = EmailMessageOps::convert_links("[a](https://a.com) and [b](https://b.com)");
        assert!(result.contains("<a href=\"https://a.com\">a</a>"));
        assert!(result.contains("<a href=\"https://b.com\">b</a>"));
    }
}
