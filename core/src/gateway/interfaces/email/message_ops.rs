//! Email Operations
//!
//! Low-level functions for IMAP polling and SMTP sending.
//! Separated from the channel struct for testability.

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, SendResult, UserId,
};
use chrono::Utc;
use std::time::Duration;

use super::config::EmailConfig;

const INITIAL_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Email message operations helper.
///
/// Provides methods for IMAP polling and SMTP sending.
pub struct EmailMessageOps;

impl EmailMessageOps {
    /// Extract agent name from email subject brackets.
    ///
    /// Pattern: `[agent-name] subject text` -> `Some("agent-name")`
    ///
    /// # Examples
    /// ```ignore
    /// assert_eq!(EmailMessageOps::extract_agent_from_subject("[coder] Fix the bug"), Some("coder".to_string()));
    /// assert_eq!(EmailMessageOps::extract_agent_from_subject("No brackets"), None);
    /// ```
    pub fn extract_agent_from_subject(subject: &str) -> Option<String> {
        let subject = subject.trim();
        if subject.starts_with('[') {
            if let Some(end) = subject.find(']') {
                let agent = &subject[1..end];
                if !agent.is_empty() {
                    return Some(agent.to_string());
                }
            }
        }
        None
    }

    /// Strip the agent tag from a subject line.
    ///
    /// `[agent-name] Hello` -> `"Hello"`
    /// `No brackets` -> `"No brackets"`
    pub fn strip_agent_tag(subject: &str) -> String {
        let subject = subject.trim();
        if subject.starts_with('[') {
            if let Some(end) = subject.find(']') {
                return subject[end + 1..].trim().to_string();
            }
        }
        subject.to_string()
    }

    /// Parse email body text from raw email bytes.
    ///
    /// Prefers plain text part over HTML. Falls back to extracting
    /// text from HTML if no plain text part is available.
    #[cfg(feature = "email")]
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
    #[cfg(feature = "email")]
    pub fn extract_sender(raw_email: &[u8]) -> Option<String> {
        use mail_parser::MessageParser;

        let message = MessageParser::default().parse(raw_email)?;
        let from = message.from()?;
        from.first().and_then(|addr| addr.address()).map(|s| s.to_string())
    }

    /// Extract subject from raw email bytes.
    #[cfg(feature = "email")]
    pub fn extract_subject(raw_email: &[u8]) -> Option<String> {
        use mail_parser::MessageParser;

        let message = MessageParser::default().parse(raw_email)?;
        message.subject().map(|s| s.to_string())
    }

    /// Extract Message-ID from raw email bytes.
    #[cfg(feature = "email")]
    pub fn extract_message_id(raw_email: &[u8]) -> Option<String> {
        use mail_parser::MessageParser;

        let message = MessageParser::default().parse(raw_email)?;
        message.message_id().map(|s| s.to_string())
    }

    /// Build a simple HTML email body from Markdown text.
    ///
    /// Wraps the Markdown content in a minimal HTML template.
    pub fn markdown_to_html_email(markdown: &str) -> String {
        // Simple Markdown-to-HTML conversion for email
        let mut html = markdown.to_string();

        // Convert code blocks first (before inline conversions).
        // Use a placeholder to protect code blocks from newline conversion.
        let mut code_blocks: Vec<String> = Vec::new();
        html = Self::extract_code_blocks(&html, &mut code_blocks);

        // Bold: **text** -> <strong>text</strong>
        html = Self::convert_paired_marker(&html, "**", "<strong>", "</strong>");

        // Italic: *text* -> <em>text</em>
        html = Self::convert_single_asterisk(&html, "<em>", "</em>");

        // Inline code: `text` -> <code>text</code>
        html = Self::convert_paired_marker(&html, "`", "<code>", "</code>");

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
    #[cfg(feature = "email")]
    pub async fn send_email(
        config: &EmailConfig,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<SendResult, ChannelError> {
        use lettre::message::header::ContentType;
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

        let html_body = Self::markdown_to_html_email(body);

        let email = Message::builder()
            .from(
                config
                    .from_address
                    .parse()
                    .map_err(|e| ChannelError::ConfigError(format!("Invalid from address: {e}")))?,
            )
            .to(to
                .parse()
                .map_err(|e| ChannelError::SendFailed(format!("Invalid recipient address: {e}")))?)
            .subject(subject)
            .header(ContentType::TEXT_HTML)
            .body(html_body)
            .map_err(|e| ChannelError::SendFailed(format!("Failed to build email: {e}")))?;

        let creds = Credentials::new(config.username.clone(), config.password.clone());

        let mailer = if config.use_tls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)
                .map_err(|e| {
                    ChannelError::SendFailed(format!("Failed to create SMTP transport: {e}"))
                })?
                .port(config.smtp_port)
                .credentials(creds)
                .build()
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host)
                .port(config.smtp_port)
                .credentials(creds)
                .build()
        };

        let response = mailer.send(email).await.map_err(|e| {
            ChannelError::SendFailed(format!("SMTP send failed: {e}"))
        })?;

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
    /// messages, converts them to InboundMessages, and marks them as seen.
    #[cfg(feature = "email")]
    pub async fn run_imap_poll_loop(
        config: EmailConfig,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        channel_id: ChannelId,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let poll_interval = Duration::from_secs(config.poll_interval_secs);
        let mut backoff = INITIAL_BACKOFF;

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            match Self::poll_imap_once(&config, &inbound_tx, &channel_id).await {
                Ok(count) => {
                    if count > 0 {
                        tracing::info!("Email: processed {count} new messages");
                    }
                    backoff = INITIAL_BACKOFF; // Reset backoff on success
                }
                Err(e) => {
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
    }

    /// Perform a single IMAP poll cycle.
    ///
    /// Connects to the IMAP server, searches for unseen messages in each
    /// configured folder, fetches and parses them, then marks them as seen.
    ///
    /// Uses `tokio_util::compat` to bridge tokio's `AsyncRead/AsyncWrite` with
    /// the `futures::AsyncRead/AsyncWrite` that `async-imap` expects.
    #[cfg(feature = "email")]
    async fn poll_imap_once(
        config: &EmailConfig,
        inbound_tx: &tokio::sync::mpsc::Sender<InboundMessage>,
        channel_id: &ChannelId,
    ) -> Result<usize, ChannelError> {
        use tokio_util::compat::TokioAsyncReadCompatExt;

        let tcp_stream = tokio::net::TcpStream::connect((config.imap_host.as_str(), config.imap_port))
            .await
            .map_err(|e| ChannelError::Internal(format!("IMAP TCP connect failed: {e}")))?;

        // Wrap tokio TcpStream with compat layer so it implements futures::AsyncRead/Write
        let compat_stream = tcp_stream.compat();

        let tls_stream = if config.use_tls {
            let tls = async_native_tls::TlsConnector::new();
            tls.connect(&config.imap_host, compat_stream)
                .await
                .map_err(|e| ChannelError::Internal(format!("IMAP TLS connect failed: {e}")))?
        } else {
            return Err(ChannelError::ConfigError(
                "Non-TLS IMAP connections are not supported for security reasons".to_string(),
            ));
        };

        let client = async_imap::Client::new(tls_stream);
        let mut session = client
            .login(&config.username, &config.password)
            .await
            .map_err(|(e, _)| ChannelError::AuthFailed(format!("IMAP login failed: {e}")))?;

        let mut total_processed = 0;

        for folder in &config.folders {
            // Select the folder
            session.select(folder).await.map_err(|e| {
                ChannelError::Internal(format!("IMAP SELECT {folder} failed: {e}"))
            })?;

            // Search for unseen messages
            let search_result = session
                .search("UNSEEN")
                .await
                .map_err(|e| ChannelError::Internal(format!("IMAP SEARCH failed: {e}")))?;

            if search_result.is_empty() {
                continue;
            }

            // Build sequence set from UIDs
            let uids: Vec<String> = search_result.iter().map(|uid: &u32| uid.to_string()).collect();
            let uid_set = uids.join(",");

            // Fetch messages
            let messages_stream = session
                .fetch(&uid_set, "(RFC822 UID)")
                .await
                .map_err(|e| ChannelError::Internal(format!("IMAP FETCH failed: {e}")))?;

            // Collect fetches into a Vec to release the borrow on session
            use futures::StreamExt;
            let fetches: Vec<_> = messages_stream
                .filter_map(|r| async { r.ok() })
                .collect()
                .await;

            for fetch in &fetches {
                let body = match fetch.body() {
                    Some(b) => b,
                    None => continue,
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
                let text = Self::extract_body_text(body);
                let message_id = Self::extract_message_id(body)
                    .unwrap_or_else(|| format!("email-{}", uuid::Uuid::new_v4()));

                if text.is_empty() {
                    continue;
                }

                let inbound = InboundMessage {
                    id: MessageId::new(&message_id),
                    channel_id: channel_id.clone(),
                    conversation_id: ConversationId::new(sender.clone()),
                    sender_id: UserId::new(sender.clone()),
                    sender_name: Some(sender.clone()),
                    text,
                    attachments: Vec::new(), // TODO: extract email attachments
                    timestamp: Utc::now(),
                    reply_to: None,
                    is_group: false,
                    raw: Some(serde_json::json!({
                        "subject": subject,
                        "from": sender,
                        "message_id": message_id,
                        "folder": folder,
                    })),
                };

                if inbound_tx.send(inbound).await.is_err() {
                    tracing::error!("Email: inbound channel closed");
                    break;
                }

                total_processed += 1;
            }

            // Mark fetched messages as seen
            if !uid_set.is_empty() {
                if let Ok(mut store_stream) = session
                    .store(&uid_set, "+FLAGS (\\Seen)")
                    .await
                {
                    // Consume the stream to apply the flag changes
                    while let Some(_) = store_stream.next().await {}
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

    /// Extract fenced code blocks, replace with placeholders, store HTML in `blocks`.
    fn extract_code_blocks(text: &str, blocks: &mut Vec<String>) -> String {
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

        loop {
            if let Some(start) = result.find(marker) {
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
                        let replacement =
                            format!("<a href=\"{url}\">{link_text}</a>");
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
    // Agent extraction from subject
    // ------------------------------------------------------------------

    #[test]
    fn test_extract_agent_basic() {
        assert_eq!(
            EmailMessageOps::extract_agent_from_subject("[coder] Fix the bug"),
            Some("coder".to_string())
        );
    }

    #[test]
    fn test_extract_agent_with_spaces() {
        assert_eq!(
            EmailMessageOps::extract_agent_from_subject("  [researcher] Find papers  "),
            Some("researcher".to_string())
        );
    }

    #[test]
    fn test_extract_agent_no_brackets() {
        assert_eq!(
            EmailMessageOps::extract_agent_from_subject("No brackets here"),
            None
        );
    }

    #[test]
    fn test_extract_agent_empty_brackets() {
        assert_eq!(
            EmailMessageOps::extract_agent_from_subject("[] Empty brackets"),
            None
        );
    }

    #[test]
    fn test_extract_agent_brackets_not_at_start() {
        assert_eq!(
            EmailMessageOps::extract_agent_from_subject("Re: [coder] Fix bug"),
            None
        );
    }

    // ------------------------------------------------------------------
    // Subject tag stripping
    // ------------------------------------------------------------------

    #[test]
    fn test_strip_agent_tag_present() {
        assert_eq!(
            EmailMessageOps::strip_agent_tag("[coder] Fix the bug"),
            "Fix the bug"
        );
    }

    #[test]
    fn test_strip_agent_tag_absent() {
        assert_eq!(
            EmailMessageOps::strip_agent_tag("No brackets"),
            "No brackets"
        );
    }

    #[test]
    fn test_strip_agent_tag_empty_subject() {
        assert_eq!(EmailMessageOps::strip_agent_tag(""), "");
    }

    #[test]
    fn test_strip_agent_tag_only_brackets() {
        assert_eq!(EmailMessageOps::strip_agent_tag("[agent]"), "");
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
        let result = EmailMessageOps::convert_links(
            "[a](https://a.com) and [b](https://b.com)"
        );
        assert!(result.contains("<a href=\"https://a.com\">a</a>"));
        assert!(result.contains("<a href=\"https://b.com\">b</a>"));
    }
}
