//! Markdown → ANSI rendering for human-facing assistant output.
//!
//! The Panel renders Markdown client-side (pulldown-cmark + syntect); the CLI
//! previously printed the raw Markdown source. This module closes that gap with
//! a small, dependency-light terminal renderer that reuses the existing colour
//! gate ([`super::theme::use_color`]):
//!
//! - **colour on** (a TTY, or `ALEPH_COLOR=always`): emphasis becomes ANSI SGR,
//!   markers are removed, headings/code/quotes are colourised.
//! - **colour off** (piped, `NO_COLOR`, `ALEPH_COLOR=never`): the same structure
//!   is emitted as clean plain text — list bullets and code fences are kept so
//!   the output stays readable and code stays copy-pasteable.
//!
//! It deliberately covers the common subset of CommonMark/GFM that assistant
//! replies use (headings, emphasis, inline code, fenced code, lists, block
//! quotes, rules, links, tables) rather than every edge of the spec.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::theme::use_color;

const RESET: &str = "\x1b[0m";

/// Render Markdown to a terminal-friendly string (ANSI when colour is enabled).
pub fn render(markdown: &str) -> String {
    render_with_color(markdown, use_color())
}

/// Render Markdown with an explicit colour decision.
///
/// `render` delegates here after consulting the ambient colour gate; tests use
/// this seam to exercise both the coloured (OSC-8 hyperlinks, SGR) and plain
/// (`text (url)`) paths deterministically without depending on a TTY.
pub(crate) fn render_with_color(markdown: &str, color: bool) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);

    let mut renderer = Renderer::new(color);
    for event in Parser::new_ext(markdown, opts) {
        renderer.handle(event);
    }
    renderer.finish()
}

/// One pending table being accumulated until its closing tag.
#[derive(Default)]
struct TableAcc {
    rows: Vec<Vec<String>>,
    current: Vec<String>,
    cell: String,
    header_rows: usize,
}

/// One open `[text](url)` link being accumulated until its closing tag, so the
/// destination can be rendered as an OSC-8 hyperlink (coloured) or a `text
/// (url)` suffix (plain). CommonMark links do not nest, so a single slot
/// suffices.
struct LinkAcc {
    url: String,
    text: String,
}

struct Renderer {
    out: String,
    color: bool,
    /// Active inline SGR codes (e.g. `["1", "3"]`) for nested emphasis.
    inline: Vec<&'static str>,
    /// One entry per open list; `None` = bullet, `Some(n)` = next ordinal.
    lists: Vec<Option<u64>>,
    quote_depth: usize,
    in_code_block: bool,
    table: Option<TableAcc>,
    link: Option<LinkAcc>,
}

impl Renderer {
    fn new(color: bool) -> Self {
        Self {
            out: String::new(),
            color,
            inline: Vec::new(),
            lists: Vec::new(),
            quote_depth: 0,
            in_code_block: false,
            table: None,
            link: None,
        }
    }

    /// Append `s` to the active table cell when inside a table, else to the
    /// main output buffer. Used for already-composed fragments (e.g. a fully
    /// rendered hyperlink) that must not be re-styled.
    fn emit(&mut self, s: &str) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(s);
        } else {
            self.out.push_str(s);
        }
    }

    fn finish(mut self) -> String {
        while self.out.ends_with('\n') {
            self.out.pop();
        }
        self.out
    }

    /// Ensure the buffer is at the start of a fresh line.
    fn ensure_line_start(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    /// Ensure exactly one blank line precedes the next block (no leading blanks).
    fn ensure_blank_line(&mut self) {
        if self.out.is_empty() {
            return;
        }
        self.ensure_line_start();
        if !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn quote_prefix(&self) -> String {
        if self.quote_depth == 0 {
            return String::new();
        }
        let bar = if self.color {
            format!("\x1b[90m{}{}", "│ ".repeat(self.quote_depth), RESET)
        } else {
            "> ".repeat(self.quote_depth)
        };
        bar
    }

    fn current_indent(&self) -> String {
        // Two spaces per nesting level for list items.
        "  ".repeat(self.lists.len().saturating_sub(1))
    }

    /// Emit text with the active inline styles applied (colour only).
    fn push_styled(&mut self, s: &str) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(s);
            return;
        }
        if self.color && !self.inline.is_empty() {
            self.out.push_str("\x1b[");
            self.out.push_str(&self.inline.join(";"));
            self.out.push('m');
            self.out.push_str(s);
            self.out.push_str(RESET);
        } else {
            self.out.push_str(s);
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                // Inline code: bright cyan when coloured; keep backticks when not
                // so it stays distinguishable in plain output.
                let rendered = if self.color {
                    ansi(self.color, "96", &code)
                } else {
                    format!("`{code}`")
                };
                if let Some(link) = self.link.as_mut() {
                    link.text.push_str(&rendered);
                } else if let Some(table) = self.table.as_mut() {
                    table.cell.push_str(&rendered);
                } else {
                    self.out.push_str(&rendered);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(link) = self.link.as_mut() {
                    link.text.push(' ');
                } else if let Some(table) = self.table.as_mut() {
                    table.cell.push(' ');
                } else {
                    self.out.push('\n');
                    let prefix = self.quote_prefix();
                    self.out.push_str(&prefix);
                }
            }
            Event::Rule => {
                self.ensure_blank_line();
                self.out
                    .push_str(&ansi(self.color, "90", "────────────────────"));
                self.out.push('\n');
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                self.out.push_str(mark);
            }
            // HTML and footnotes are passed through as-is / ignored.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.ensure_blank_line();
                let prefix = self.quote_prefix();
                self.out.push_str(&prefix);
            }
            Tag::Heading { .. } => {
                self.ensure_blank_line();
                self.inline.push("1;36"); // bold cyan
            }
            Tag::BlockQuote(_) => {
                self.ensure_blank_line();
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.ensure_blank_line();
                self.in_code_block = true;
                let lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                self.out
                    .push_str(&ansi(self.color, "90", &format!("```{lang}")));
                self.out.push('\n');
            }
            Tag::List(start) => {
                self.ensure_blank_line();
                self.lists.push(start);
            }
            Tag::Item => {
                self.ensure_line_start();
                let indent = self.current_indent();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "• ".to_string(),
                };
                self.out.push_str(&indent);
                self.out.push_str(&ansi(self.color, "36", &marker));
            }
            Tag::Emphasis => self.inline.push("3"),
            Tag::Strong => self.inline.push("1"),
            Tag::Strikethrough => self.inline.push("9"),
            Tag::Link { dest_url, .. } => {
                // Start buffering link text; the URL is folded in at `end`.
                self.link = Some(LinkAcc {
                    url: dest_url.to_string(),
                    text: String::new(),
                });
            }
            Tag::Table(_) => {
                self.ensure_blank_line();
                self.table = Some(TableAcc::default());
            }
            Tag::TableHead => {}
            Tag::TableRow => {}
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.cell.clear();
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.out.push('\n');
            }
            TagEnd::Heading(_) => {
                self.inline.pop();
                self.out.push('\n');
            }
            TagEnd::BlockQuote(_) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                self.out.push_str(&ansi(self.color, "90", "```"));
                self.out.push('\n');
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item => {
                self.ensure_line_start();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline.pop();
            }
            TagEnd::Link => {
                if let Some(link) = self.link.take() {
                    let rendered = render_link(self.color, &link.url, &link.text);
                    self.emit(&rendered);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(table);
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.current);
                    t.rows.push(row);
                    t.header_rows = t.rows.len();
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.current);
                    t.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let cell = std::mem::take(&mut t.cell);
                    t.current.push(cell);
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, s: &str) {
        // Link text is buffered so the destination URL can be folded in at the
        // closing tag (OSC-8 hyperlink when coloured, `text (url)` when plain).
        if let Some(link) = self.link.as_mut() {
            link.text.push_str(s);
            return;
        }
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(s);
            return;
        }
        if self.in_code_block {
            // Render code verbatim, colourised line by line, honouring quotes.
            for line in s.split_inclusive('\n') {
                let nl = line.ends_with('\n');
                let body = line.strip_suffix('\n').unwrap_or(line);
                self.out.push_str(&ansi(self.color, "32", body));
                if nl {
                    self.out.push('\n');
                }
            }
            return;
        }
        self.push_styled(s);
    }

    /// Render an accumulated table as aligned, optionally-coloured columns.
    fn render_table(&mut self, table: TableAcc) {
        if table.rows.is_empty() {
            return;
        }
        let cols = table.rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut widths = vec![0usize; cols];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(display_width(cell));
            }
        }

        for (idx, row) in table.rows.iter().enumerate() {
            let is_header = idx < table.header_rows;
            let cells: Vec<String> = (0..cols)
                .map(|i| {
                    let cell = row.get(i).map(String::as_str).unwrap_or("");
                    let padded = pad_to(cell, widths[i]);
                    if is_header {
                        ansi(self.color, "1", &padded)
                    } else {
                        padded
                    }
                })
                .collect();
            self.out.push_str(&cells.join("  "));
            self.out.push('\n');

            if is_header && idx + 1 == table.header_rows {
                let sep: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                self.out.push_str(&ansi(self.color, "90", &sep.join("  ")));
                self.out.push('\n');
            }
        }
    }
}

/// Wrap `s` in the SGR escape `code` when colour is enabled, else return it
/// unchanged. A free function (not a method) so callers can pass the `color`
/// field while mutably borrowing a disjoint field such as `out`.
fn ansi(color: bool, code: &str, s: &str) -> String {
    if color {
        format!("\x1b[{code}m{s}{RESET}")
    } else {
        s.to_string()
    }
}

/// Render a `[text](url)` link for the terminal.
///
/// - **colour on**: an OSC-8 hyperlink (`ESC]8;;URL ST  text  ESC]8;; ST`) so
///   supporting terminals make the label clickable, with the label coloured
///   like a link (underlined blue) for terminals that ignore OSC-8.
/// - **colour off**: `text (url)` so the destination is never lost when piped
///   or in `NO_COLOR` mode; collapses to just `url` for bare/auto links where
///   the label already equals the destination.
fn render_link(color: bool, url: &str, text: &str) -> String {
    if url.is_empty() {
        return text.to_string();
    }
    if color {
        let label = if text.is_empty() { url } else { text };
        let styled = ansi(true, "4;34", label);
        format!("\x1b]8;;{url}\x1b\\{styled}\x1b]8;;\x1b\\")
    } else if text.is_empty() || text == url {
        url.to_string()
    } else {
        format!("{text} ({url})")
    }
}

// Display-width helpers live in `super::width` (CJK-aware via unicode-width).
use super::width::{display_width, pad_to};

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip ANSI SGR sequences so assertions can target structure regardless
    /// of the ambient colour gate.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Skip until the SGR terminator 'm'.
                for n in chars.by_ref() {
                    if n == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn renders_heading_text_without_markers() {
        let out = strip_ansi(&render("# Title\n\nbody"));
        assert!(out.contains("Title"));
        assert!(
            !out.contains('#'),
            "heading markers should be stripped: {out:?}"
        );
        assert!(out.contains("body"));
    }

    #[test]
    fn renders_bullet_list() {
        let out = strip_ansi(&render("- one\n- two"));
        assert!(out.contains("• one"), "got: {out:?}");
        assert!(out.contains("• two"), "got: {out:?}");
    }

    #[test]
    fn renders_ordered_list_with_ordinals() {
        let out = strip_ansi(&render("1. first\n2. second"));
        assert!(out.contains("1. first"), "got: {out:?}");
        assert!(out.contains("2. second"), "got: {out:?}");
    }

    #[test]
    fn keeps_code_fence_and_body() {
        let out = strip_ansi(&render("```rust\nlet x = 1;\n```"));
        assert!(out.contains("```rust"), "got: {out:?}");
        assert!(out.contains("let x = 1;"), "got: {out:?}");
    }

    #[test]
    fn renders_table_aligned_with_header_separator() {
        let md = "| a | bb |\n| - | -- |\n| 1 | 2 |";
        let out = strip_ansi(&render(md));
        assert!(out.contains("a"), "got: {out:?}");
        assert!(out.contains("bb"), "got: {out:?}");
        assert!(out.contains("─"), "expected a header separator: {out:?}");
        assert!(out.contains('1') && out.contains('2'), "got: {out:?}");
    }

    #[test]
    fn emphasis_markers_removed_in_plain_output() {
        // strip_ansi removes escapes; the markers themselves must be gone too.
        let out = strip_ansi(&render("a **bold** and *italic* word"));
        assert!(out.contains("bold"));
        assert!(out.contains("italic"));
        assert!(!out.contains("**"), "got: {out:?}");
    }

    #[test]
    fn inline_code_preserved_as_text() {
        let out = strip_ansi(&render("call `foo()` now"));
        assert!(out.contains("foo()"), "got: {out:?}");
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(render(""), "");
    }

    #[test]
    fn plain_link_keeps_url_as_suffix() {
        // Colour off: the destination must survive as `text (url)`.
        let out = render_with_color("see [Anthropic](https://anthropic.com)", false);
        assert!(
            out.contains("Anthropic (https://anthropic.com)"),
            "got: {out:?}"
        );
    }

    #[test]
    fn plain_autolink_collapses_to_single_url() {
        // When the label already equals the destination, don't print it twice.
        let out = render_with_color("[https://x.dev](https://x.dev)", false);
        assert!(out.contains("https://x.dev"), "got: {out:?}");
        assert_eq!(out.matches("https://x.dev").count(), 1, "got: {out:?}");
    }

    #[test]
    fn coloured_link_emits_osc8_hyperlink() {
        // Colour on: an OSC-8 hyperlink wraps the (styled) label.
        let out = render_with_color("[docs](https://a.io)", true);
        assert!(out.contains("\x1b]8;;https://a.io\x1b\\"), "open: {out:?}");
        assert!(out.contains("\x1b]8;;\x1b\\"), "close: {out:?}");
        assert!(out.contains("docs"), "label: {out:?}");
    }

    #[test]
    fn link_inside_table_cell_renders() {
        let md = "| name | site |\n| - | - |\n| a | [x](https://x.io) |";
        let plain = render_with_color(md, false);
        assert!(plain.contains("https://x.io"), "got: {plain:?}");
    }

    #[test]
    fn cjk_table_columns_stay_aligned() {
        // First data column has a 1-char and a 2-char (CJK) cell; both rows must
        // pad the second column to the same starting offset.
        let md = "| 名 | v |\n| - | - |\n| 中文 | 1 |\n| a | 2 |";
        let out = strip_ansi(&render_with_color(md, false));
        let data_lines: Vec<&str> = out
            .lines()
            .filter(|l| l.contains('1') || l.contains('2'))
            .collect();
        assert_eq!(data_lines.len(), 2, "got: {out:?}");
        // The value column should begin at the same display offset on both rows.
        let offset = |line: &str, needle: char| {
            let idx = line.find(needle).unwrap();
            display_width(&line[..idx])
        };
        assert_eq!(
            offset(data_lines[0], '1'),
            offset(data_lines[1], '2'),
            "CJK row misaligned: {data_lines:?}"
        );
    }
}
