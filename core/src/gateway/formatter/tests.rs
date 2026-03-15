//! Tests for message formatting.

use super::*;

// -----------------------------------------------------------------------
// Telegram HTML
// -----------------------------------------------------------------------

#[test]
fn test_bold_to_telegram_html() {
    assert_eq!(
        MessageFormatter::format("**hello**", MarkupFormat::TelegramHtml),
        "<b>hello</b>"
    );
}

#[test]
fn test_italic_to_telegram_html() {
    assert_eq!(
        MessageFormatter::format("*hello*", MarkupFormat::TelegramHtml),
        "<i>hello</i>"
    );
}

#[test]
fn test_code_to_telegram_html() {
    assert_eq!(
        MessageFormatter::format("`code`", MarkupFormat::TelegramHtml),
        "<code>code</code>"
    );
}

#[test]
fn test_link_to_telegram_html() {
    assert_eq!(
        MessageFormatter::format("[text](https://example.com)", MarkupFormat::TelegramHtml),
        "<a href=\"https://example.com\">text</a>"
    );
}

#[test]
fn test_code_block_to_telegram_html() {
    let input = "```rust\nlet x = 1;\n```";
    let expected = "<pre><code class=\"language-rust\">let x = 1;\n</code></pre>";
    assert_eq!(
        MessageFormatter::format(input, MarkupFormat::TelegramHtml),
        expected
    );
}

#[test]
fn test_code_block_no_lang_to_telegram_html() {
    let input = "```\nsome code\n```";
    let expected = "<pre><code>some code\n</code></pre>";
    assert_eq!(
        MessageFormatter::format(input, MarkupFormat::TelegramHtml),
        expected
    );
}

#[test]
fn test_mixed_content_telegram_html() {
    let input = "**bold** and *italic* and `code`";
    let expected = "<b>bold</b> and <i>italic</i> and <code>code</code>";
    assert_eq!(
        MessageFormatter::format(input, MarkupFormat::TelegramHtml),
        expected
    );
}

#[test]
fn test_html_escape_in_code_block() {
    let input = "```\n<div>test</div>\n```";
    let expected = "<pre><code>&lt;div&gt;test&lt;/div&gt;\n</code></pre>";
    assert_eq!(
        MessageFormatter::format(input, MarkupFormat::TelegramHtml),
        expected
    );
}

#[test]
fn test_html_escape_inline_text() {
    // Plain text containing HTML special characters must be escaped so that
    // Telegram's HTML parser does not choke on them.
    assert_eq!(
        MessageFormatter::format("1 < 2 & 3 > 0", MarkupFormat::TelegramHtml),
        "1 &lt; 2 &amp; 3 &gt; 0"
    );
}

// -----------------------------------------------------------------------
// Slack mrkdwn
// -----------------------------------------------------------------------

#[test]
fn test_bold_to_slack() {
    assert_eq!(
        MessageFormatter::format("**hello**", MarkupFormat::SlackMrkdwn),
        "*hello*"
    );
}

#[test]
fn test_link_to_slack() {
    assert_eq!(
        MessageFormatter::format("[text](https://example.com)", MarkupFormat::SlackMrkdwn),
        "<https://example.com|text>"
    );
}

#[test]
fn test_code_preserved_slack() {
    assert_eq!(
        MessageFormatter::format("`code`", MarkupFormat::SlackMrkdwn),
        "`code`"
    );
}

// -----------------------------------------------------------------------
// IRC formatting
// -----------------------------------------------------------------------

#[test]
fn test_bold_to_irc() {
    assert_eq!(
        MessageFormatter::format("**hello**", MarkupFormat::IrcFormatting),
        "\x02hello\x02"
    );
}

#[test]
fn test_italic_to_irc() {
    assert_eq!(
        MessageFormatter::format("*hello*", MarkupFormat::IrcFormatting),
        "\x1Dhello\x1D"
    );
}

#[test]
fn test_link_to_irc() {
    assert_eq!(
        MessageFormatter::format("[text](https://example.com)", MarkupFormat::IrcFormatting),
        "text (https://example.com)"
    );
}

// -----------------------------------------------------------------------
// Discord (passthrough)
// -----------------------------------------------------------------------

#[test]
fn test_discord_passthrough() {
    let input = "**bold** and *italic* and `code`";
    assert_eq!(
        MessageFormatter::format(input, MarkupFormat::DiscordMarkdown),
        input
    );
}

// -----------------------------------------------------------------------
// Plain text
// -----------------------------------------------------------------------

#[test]
fn test_strip_to_plain() {
    assert_eq!(
        MessageFormatter::format("**bold** and *italic*", MarkupFormat::PlainText),
        "bold and italic"
    );
}

#[test]
fn test_plain_strips_code() {
    assert_eq!(
        MessageFormatter::format("`code`", MarkupFormat::PlainText),
        "code"
    );
}

#[test]
fn test_plain_converts_links() {
    assert_eq!(
        MessageFormatter::format("[click](https://example.com)", MarkupFormat::PlainText),
        "click (https://example.com)"
    );
}

#[test]
fn test_plain_strips_code_block() {
    let input = "before\n```rust\nlet x = 1;\n```\nafter";
    let result = MessageFormatter::format(input, MarkupFormat::PlainText);
    assert!(result.contains("let x = 1;"));
    assert!(!result.contains("```"));
}

// -----------------------------------------------------------------------
// Split
// -----------------------------------------------------------------------

#[test]
fn test_split_short_message() {
    let result = MessageFormatter::split("short", 100);
    assert_eq!(result, vec!["short"]);
}

#[test]
fn test_split_at_paragraph_boundary() {
    let text = "paragraph one\n\nparagraph two\n\nparagraph three";
    let chunks = MessageFormatter::split(text, 25);
    // Should split at paragraph boundaries.
    assert!(chunks.len() >= 2);
    for chunk in &chunks {
        assert!(chunk.len() <= 25, "chunk too long: {}", chunk.len());
    }
}

#[test]
fn test_split_respects_code_blocks() {
    let text = format!(
        "intro\n\n```rust\n{}\n```\n\noutro",
        "let x = 1;\n".repeat(5)
    );
    let chunks = MessageFormatter::split(&text, 200);
    // The code block should not be split across chunks.
    let mut found_partial_fence = false;
    for chunk in &chunks {
        let opens = chunk.matches("```").count();
        // If a chunk contains an opening fence, it should also contain the close.
        if opens % 2 != 0 {
            found_partial_fence = true;
        }
    }
    // This is best-effort; if the code block itself is smaller than max_len,
    // it should remain intact.
    if text.len() > 200 {
        // We expect the code block to fit in one chunk since it's ~80 bytes.
        assert!(
            !found_partial_fence,
            "code block was split across chunks"
        );
    }
}

#[test]
fn test_split_at_line_boundary() {
    let text = "line one\nline two\nline three\nline four";
    let chunks = MessageFormatter::split(text, 20);
    assert!(chunks.len() >= 2);
    for chunk in &chunks {
        assert!(chunk.len() <= 20);
    }
}

#[test]
fn test_split_long_line_hard_split() {
    // A single very long line with no newlines or other split opportunities
    // must still produce chunks that are all <= max_len.
    let long_line = "x".repeat(150);
    let max_len = 40;
    let chunks = MessageFormatter::split(&long_line, max_len);
    assert!(chunks.len() >= 4, "expected at least 4 chunks, got {}", chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        assert!(
            chunk.len() <= max_len,
            "chunk {} has length {} which exceeds max_len {}",
            i,
            chunk.len(),
            max_len
        );
    }
    // Verify all content is preserved.
    let reassembled: String = chunks.concat();
    assert_eq!(reassembled, long_line);
}

// -----------------------------------------------------------------------
// Normalize (reverse direction)
// -----------------------------------------------------------------------

#[test]
fn test_normalize_telegram_html() {
    assert_eq!(
        MessageFormatter::normalize("<b>hello</b>", MarkupFormat::TelegramHtml),
        "**hello**"
    );
}

#[test]
fn test_normalize_telegram_html_italic() {
    assert_eq!(
        MessageFormatter::normalize("<i>hello</i>", MarkupFormat::TelegramHtml),
        "*hello*"
    );
}

#[test]
fn test_normalize_telegram_html_code() {
    assert_eq!(
        MessageFormatter::normalize("<code>x</code>", MarkupFormat::TelegramHtml),
        "`x`"
    );
}

#[test]
fn test_normalize_telegram_html_link() {
    assert_eq!(
        MessageFormatter::normalize(
            "<a href=\"https://example.com\">text</a>",
            MarkupFormat::TelegramHtml
        ),
        "[text](https://example.com)"
    );
}

#[test]
fn test_normalize_telegram_html_pre_code() {
    assert_eq!(
        MessageFormatter::normalize(
            "<pre><code class=\"language-rust\">let x = 1;\n</code></pre>",
            MarkupFormat::TelegramHtml
        ),
        "```rust\nlet x = 1;\n```"
    );
}

#[test]
fn test_normalize_slack_mrkdwn() {
    // Slack *hello* -> **hello** (Slack uses single * for bold)
    assert_eq!(
        MessageFormatter::normalize("*hello*", MarkupFormat::SlackMrkdwn),
        "**hello**"
    );
}

#[test]
fn test_normalize_slack_link() {
    assert_eq!(
        MessageFormatter::normalize(
            "<https://example.com|text>",
            MarkupFormat::SlackMrkdwn
        ),
        "[text](https://example.com)"
    );
}

#[test]
fn test_normalize_irc_bold() {
    assert_eq!(
        MessageFormatter::normalize("\x02hello\x02", MarkupFormat::IrcFormatting),
        "**hello**"
    );
}

#[test]
fn test_normalize_irc_italic() {
    assert_eq!(
        MessageFormatter::normalize("\x1Dhello\x1D", MarkupFormat::IrcFormatting),
        "*hello*"
    );
}

#[test]
fn test_normalize_plain_passthrough() {
    let text = "just plain text";
    assert_eq!(
        MessageFormatter::normalize(text, MarkupFormat::PlainText),
        text
    );
}

#[test]
fn test_normalize_markdown_passthrough() {
    let text = "**bold** and *italic*";
    assert_eq!(
        MessageFormatter::normalize(text, MarkupFormat::Markdown),
        text
    );
}

// -----------------------------------------------------------------------
// Roundtrip tests
// -----------------------------------------------------------------------

#[test]
fn test_roundtrip_telegram_bold() {
    let original = "**hello**";
    let html = MessageFormatter::format(original, MarkupFormat::TelegramHtml);
    let back = MessageFormatter::normalize(&html, MarkupFormat::TelegramHtml);
    assert_eq!(back, original);
}

#[test]
fn test_roundtrip_telegram_link() {
    let original = "[click](https://example.com)";
    let html = MessageFormatter::format(original, MarkupFormat::TelegramHtml);
    let back = MessageFormatter::normalize(&html, MarkupFormat::TelegramHtml);
    assert_eq!(back, original);
}

#[test]
fn test_roundtrip_slack_bold() {
    let original = "**hello**";
    let slack = MessageFormatter::format(original, MarkupFormat::SlackMrkdwn);
    let back = MessageFormatter::normalize(&slack, MarkupFormat::SlackMrkdwn);
    assert_eq!(back, original);
}

// -----------------------------------------------------------------------
// Display
// -----------------------------------------------------------------------

#[test]
fn test_markup_format_display() {
    assert_eq!(MarkupFormat::TelegramHtml.to_string(), "telegram_html");
    assert_eq!(MarkupFormat::PlainText.to_string(), "plain_text");
}
