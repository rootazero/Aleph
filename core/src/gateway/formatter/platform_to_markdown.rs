//! Platform -> Markdown format conversions (normalize).

use super::helpers::*;

/// Telegram HTML -> Markdown.
pub(super) fn telegram_html_to_markdown(html: &str) -> String {
    let mut s = html.to_string();

    // <b>text</b> -> **text**
    s = replace_html_tag(&s, "b", "**", "**");

    // <strong>text</strong> -> **text**
    s = replace_html_tag(&s, "strong", "**", "**");

    // <i>text</i> -> *text*
    s = replace_html_tag(&s, "i", "*", "*");

    // <em>text</em> -> *text*
    s = replace_html_tag(&s, "em", "*", "*");

    // <code>text</code> -> `text`
    s = replace_html_tag(&s, "code", "`", "`");

    // <pre><code>text</code></pre> -> ```\ntext\n```
    // Also handles <pre><code class="language-xxx">
    s = replace_pre_code_blocks(&s);

    // <a href="url">text</a> -> [text](url)
    s = replace_html_links(&s);

    s
}

/// Slack mrkdwn -> Markdown.
pub(super) fn slack_mrkdwn_to_markdown(text: &str) -> String {
    let mut s = text.to_string();

    // *bold* -> **bold** (Slack bold uses single *)
    // Cannot use replace_paired_marker here because marker="*" and open="**"
    // would cause an infinite loop (the output contains the marker).
    s = replace_paired_marker_positional(&s, "*", "**", "**");

    // <url|text> -> [text](url)
    s = replace_slack_links(&s);

    s
}

/// IRC formatting codes -> Markdown.
pub(super) fn irc_to_markdown(text: &str) -> String {
    let mut s = text.to_string();

    // \x02text\x02 -> **text**
    s = replace_paired_marker(&s, "\x02", "**", "**");

    // \x1Dtext\x1D -> *text*
    s = replace_paired_marker(&s, "\x1D", "*", "*");

    s
}
