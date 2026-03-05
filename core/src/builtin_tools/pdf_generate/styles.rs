//! Embedded CSS stylesheets for browser-based PDF rendering.

/// GitHub-flavored Markdown CSS for PDF output.
pub fn github_markdown_css() -> &'static str {
    r#"
    * { margin: 0; padding: 0; box-sizing: border-box; }

    body {
        font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial,
                     "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans CJK SC",
                     sans-serif;
        font-size: 14px;
        line-height: 1.6;
        color: #24292e;
        background-color: #ffffff;
        padding: 2em;
        max-width: 100%;
        word-wrap: break-word;
        overflow-wrap: break-word;
    }

    h1, h2, h3, h4, h5, h6 {
        margin-top: 1.5em; margin-bottom: 0.5em; font-weight: 600; line-height: 1.25;
    }
    h1 { font-size: 2em; border-bottom: 1px solid #eaecef; padding-bottom: 0.3em; }
    h2 { font-size: 1.5em; border-bottom: 1px solid #eaecef; padding-bottom: 0.3em; }
    h3 { font-size: 1.25em; }
    h4 { font-size: 1em; }
    h5 { font-size: 0.875em; }
    h6 { font-size: 0.85em; color: #6a737d; }

    p { margin-bottom: 1em; }
    a { color: #0366d6; text-decoration: none; }

    ul, ol { padding-left: 2em; margin-bottom: 1em; }
    li { margin-bottom: 0.25em; }
    li > ul, li > ol { margin-bottom: 0; }

    code {
        font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, "Courier New", monospace;
        font-size: 0.85em;
        background-color: rgba(27, 31, 35, 0.05);
        border-radius: 3px;
        padding: 0.2em 0.4em;
    }
    pre {
        background-color: #f6f8fa; border-radius: 6px; padding: 16px;
        margin-bottom: 1em; line-height: 1.45;
        white-space: pre-wrap;
        word-wrap: break-word;
        overflow-wrap: break-word;
    }
    pre code { background-color: transparent; padding: 0; font-size: 0.85em; }

    table { border-collapse: collapse; width: 100%; margin-bottom: 1em; }
    th, td { border: 1px solid #dfe2e5; padding: 6px 13px; text-align: left; }
    th { font-weight: 600; background-color: #f6f8fa; }
    tr:nth-child(even) { background-color: #f6f8fa; }

    blockquote {
        border-left: 4px solid #dfe2e5; color: #6a737d; padding: 0 1em; margin-bottom: 1em;
    }

    hr { border: none; border-top: 1px solid #eaecef; margin: 1.5em 0; }
    img { max-width: 100%; height: auto; }

    @media print {
        body { padding: 0; background-color: #ffffff; }
        h1, h2, h3 { break-after: avoid; }
        pre, blockquote, table { break-inside: avoid; }
        pre { white-space: pre-wrap; word-wrap: break-word; }
        p { orphans: 3; widows: 3; word-wrap: break-word; overflow-wrap: break-word; }
    }
    "#
}

/// Wrap HTML content with full document structure and CSS.
pub fn wrap_html_with_styles(html_body: &str, title: Option<&str>) -> String {
    let css = github_markdown_css();
    let title_str = title.unwrap_or("Document");
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title_str}</title>
    <style>{css}</style>
</head>
<body>
{html_body}
</body>
</html>"#
    )
}
