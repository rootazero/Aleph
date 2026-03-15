//! Unified cross-platform message formatting.
//!
//! Converts standard Markdown to/from platform-specific markup formats.
//! This is Phase 0 infrastructure used by all social bot channel implementations.
//!
//! # Supported formats
//!
//! - **Markdown** (passthrough, canonical internal format)
//! - **TelegramHtml**: `<b>`, `<i>`, `<code>`, `<pre><code>`, `<a href="">`
//! - **SlackMrkdwn**: `*bold*`, `_italic_`, `` `code` ``, `<url|text>`
//! - **DiscordMarkdown**: Discord-flavored Markdown (close to standard)
//! - **IrcFormatting**: mIRC control codes (`\x02` bold, `\x1D` italic)
//! - **PlainText**: all formatting stripped
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::gateway::formatter::{MessageFormatter, MarkupFormat};
//!
//! let html = MessageFormatter::format("**hello**", MarkupFormat::TelegramHtml);
//! assert_eq!(html, "<b>hello</b>");
//!
//! let chunks = MessageFormatter::split("long message...", 4096);
//! let md = MessageFormatter::normalize("<b>hello</b>", MarkupFormat::TelegramHtml);
//! ```

mod helpers;
mod markdown_to_platform;
mod platform_to_markdown;
mod splitting;

#[cfg(test)]
mod tests;

use std::fmt;

use markdown_to_platform::*;
use platform_to_markdown::*;
use splitting::split_message;

// ---------------------------------------------------------------------------
// MarkupFormat enum
// ---------------------------------------------------------------------------

/// Target/source markup format for message conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkupFormat {
    /// Standard Markdown (canonical internal format).
    Markdown,
    /// Telegram Bot API HTML subset.
    TelegramHtml,
    /// Slack mrkdwn format.
    SlackMrkdwn,
    /// Discord-flavored Markdown.
    DiscordMarkdown,
    /// IRC mIRC formatting codes.
    IrcFormatting,
    /// Plain text with all formatting stripped.
    PlainText,
}

impl fmt::Display for MarkupFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Markdown => write!(f, "markdown"),
            Self::TelegramHtml => write!(f, "telegram_html"),
            Self::SlackMrkdwn => write!(f, "slack_mrkdwn"),
            Self::DiscordMarkdown => write!(f, "discord_markdown"),
            Self::IrcFormatting => write!(f, "irc_formatting"),
            Self::PlainText => write!(f, "plain_text"),
        }
    }
}

// ---------------------------------------------------------------------------
// MessageFormatter
// ---------------------------------------------------------------------------

/// Unified cross-platform message formatter.
///
/// All methods are stateless and exposed as associated functions.
pub struct MessageFormatter;

impl MessageFormatter {
    /// Convert standard Markdown to the given target format.
    pub fn format(markdown: &str, target: MarkupFormat) -> String {
        match target {
            MarkupFormat::Markdown => markdown.to_string(),
            MarkupFormat::TelegramHtml => markdown_to_telegram_html(markdown),
            MarkupFormat::SlackMrkdwn => markdown_to_slack_mrkdwn(markdown),
            MarkupFormat::DiscordMarkdown => markdown_to_discord(markdown),
            MarkupFormat::IrcFormatting => markdown_to_irc(markdown),
            MarkupFormat::PlainText => markdown_to_plain(markdown),
        }
    }

    /// Smart message splitting that respects paragraph and code block boundaries.
    ///
    /// Guarantees:
    /// - Each chunk is at most `max_len` bytes.
    /// - Code blocks (triple-backtick fences) are never split mid-block
    ///   (unless a single code block exceeds `max_len`).
    /// - Splits prefer paragraph boundaries (`\n\n`), then line boundaries (`\n`).
    pub fn split(text: &str, max_len: usize) -> Vec<String> {
        if text.len() <= max_len {
            return vec![text.to_string()];
        }
        split_message(text, max_len)
    }

    /// Normalize platform-specific markup back to standard Markdown (inbound direction).
    pub fn normalize(platform_text: &str, source: MarkupFormat) -> String {
        match source {
            MarkupFormat::Markdown | MarkupFormat::DiscordMarkdown => {
                platform_text.to_string()
            }
            MarkupFormat::TelegramHtml => telegram_html_to_markdown(platform_text),
            MarkupFormat::SlackMrkdwn => slack_mrkdwn_to_markdown(platform_text),
            MarkupFormat::IrcFormatting => irc_to_markdown(platform_text),
            MarkupFormat::PlainText => platform_text.to_string(),
        }
    }
}
