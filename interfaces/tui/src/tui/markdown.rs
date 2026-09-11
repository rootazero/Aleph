//! Markdown → styled `ratatui` lines.
//!
//! # Why a real parser
//!
//! This was 837 lines of hand-written line scanning: `starts_with('#')`,
//! `starts_with("- ")`, a bespoke inline scanner for `*`/`_`/`` ` ``/`[]()`.
//! It recognised a subset, and the constructs outside that subset did not
//! degrade — they rendered as their source text, so a table arrived as a wall
//! of pipes and a numbered list as literal `1.` on every row. The Panel, the
//! CLI and the session exporter all parse the same assistant text with
//! `pulldown-cmark`; this file was the one surface answering a different
//! question about the same bytes.
//!
//! Extensions come from [`markdown_options`] — a single derivation point, not
//! a fourth copy of the flag set. See its module docs for why.
//!
//! # The pre-pass
//!
//! [`enhance`] runs first: it lifts bare URLs into links, folds GitHub
//! admonitions (`> [!WARNING]`) into a marked blockquote, and replaces
//! ```mermaid fences with a placeholder the renderer swaps back. Both surfaces
//! share it, so an admonition is an admonition in both. Mermaid renders as a
//! framed source block here — a terminal has no diagram, and ASCII art of one
//! is worse than the source (spec §6).
//!
//! # What is deliberately not done
//!
//! Syntax highlighting. A code block renders plain, in the code frame, and the
//! `syntect` load that B4c adds must not block a frame to do better.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use shared_ui_logic::transcript::{
    enhance, markdown_options, AdmonitionKind, Block, SemanticColor, MERMAID_PLACEHOLDER_PREFIX,
};
use std::rc::Rc;
use unicode_width::UnicodeWidthStr;

use super::highlight;
use super::theme::palette;

fn style(role: SemanticColor) -> Style {
    Style::default().fg(palette().color(role))
}

/// Columns one list nesting level indents by.
const LIST_INDENT: usize = 2;
/// The blockquote rail. Its width is measured, never assumed: it is what
/// every row of a quoted block is offset by.
const QUOTE_RAIL: &str = "\u{250a} ";

/// Convert markdown to styled lines wrapped to `width`.
///
/// `width` is the column budget for the whole block including any rail or
/// bullet, so a caller passes the pane width and gets back rows that fit it.
#[must_use]
pub fn markdown_to_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    render(text, width, false)
}

/// The `streaming` seam: [`enhance`]'s block-level rewrites need to see whole
/// fences and whole admonitions, which a still-growing tail does not have, so
/// it does only the line-local work when told the text is unfinished. Exactly
/// the flag the Panel passes on the same path.
fn render(text: &str, width: u16, streaming: bool) -> Vec<Line<'static>> {
    let enhanced = enhance(text, streaming);
    let mut r = Renderer::new(width as usize, enhanced.blocks);
    for event in Parser::new_ext(&enhanced.markdown, markdown_options()) {
        r.handle(event);
    }
    r.finish()
}

/// One table being accumulated until its closing tag. Cells are plain text:
/// a column width has to be known before any row can be emitted, and that
/// cannot be answered per-span.
#[derive(Default)]
struct TableAcc {
    rows: Vec<Vec<String>>,
    current: Vec<String>,
    cell: String,
    header_rows: usize,
}

/// One open `[text](url)`, buffered so the destination can be appended dimmed
/// after the label. `CommonMark` links do not nest, so one slot suffices.
struct LinkAcc {
    url: String,
    start_span: usize,
}

/// One open fenced block: its info string and its body.
#[derive(Default)]
struct CodeAcc {
    lang: String,
    body: String,
}

struct Renderer {
    width: usize,
    out: Vec<Line<'static>>,
    /// Inline spans of the block being built, flushed by its closing tag.
    spans: Vec<Span<'static>>,
    /// Nested emphasis; the top is the style new text is painted in.
    styles: Vec<Style>,
    /// One entry per open list: `None` bullets, `Some(n)` numbers from `n`.
    lists: Vec<Option<u64>>,
    /// One entry per open blockquote; the kind is filled in when the
    /// admonition marker [`enhance`] wrote is recognised.
    quotes: Vec<Option<AdmonitionKind>>,
    /// Set while the next inline run could be the admonition label, so it is
    /// consumed as the heading rather than painted as literal `[!WARNING]`.
    expect_admonition_label: bool,
    /// Text collected between the `Strong` tags [`enhance`] wraps the marker
    /// in.
    ///
    /// The marker cannot be matched against one `Event::Text`: the inline
    /// parser splits `[!WARNING]` into `"["`, `"!WARNING"`, `"]"`, because `[`
    /// opens a potential link. Measured, not assumed — matching a whole
    /// `Event::Text` looked obviously right and never fired once.
    label_buf: Option<String>,
    code: Option<CodeAcc>,
    table: Option<TableAcc>,
    link: Option<LinkAcc>,
    /// Mermaid sources, indexed by the placeholder that stands for them.
    blocks: Vec<Block>,
    /// A task-list checkbox waiting for its item's text.
    task: Option<bool>,
    /// Whether the open item still owes its marker.
    ///
    /// An item is flushed by whatever ends first — its paragraph, a nested
    /// list starting inside it, or its own closing tag — and a **loose** item
    /// holds several paragraphs. Without this, the second paragraph of
    /// `- one\n\n  two` gets its own bullet, and in an ordered list it also
    /// burns an ordinal, so the list counts 1, 2 where the source says 1.
    item_owes_marker: bool,
}

impl Renderer {
    fn new(width: usize, blocks: Vec<Block>) -> Self {
        Self {
            width: width.max(1),
            out: Vec::new(),
            spans: Vec::new(),
            styles: vec![Style::default().fg(palette().color(SemanticColor::Fg))],
            lists: Vec::new(),
            quotes: Vec::new(),
            expect_admonition_label: false,
            label_buf: None,
            code: None,
            table: None,
            link: None,
            blocks,
            task: None,
            item_owes_marker: false,
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        // An unterminated fence still has to show its content: the text
        // exists, and dropping it because the model has not closed the fence
        // yet would make a streaming code block invisible until it ended.
        if self.code.is_some() {
            self.close_code();
        }
        self.flush(Vec::new(), Vec::new());
        // Trailing blank rows are layout noise; the chat area decides the gap
        // between messages.
        while self.out.last().is_some_and(line_is_blank) {
            self.out.pop();
        }
        self.out
    }

    fn cur(&self) -> Style {
        *self.styles.last().unwrap_or(&Style::default())
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.spans.push(Span::styled(text.to_string(), style));
    }

    // ---- block emission ------------------------------------------------

    /// The rail every row of a quoted block carries, painted by the
    /// admonition kind when there is one.
    fn rail(&self) -> Vec<Span<'static>> {
        self.quotes
            .iter()
            .map(|kind| {
                let role = kind.map_or(SemanticColor::Dim, AdmonitionKind::color);
                Span::styled(QUOTE_RAIL.to_string(), style(role))
            })
            .collect()
    }

    /// Emit the accumulated spans as wrapped rows.
    ///
    /// `first` is the marker (a bullet, an ordinal, a checkbox) and `cont` is
    /// what continuation rows carry in its place. They must be the same width
    /// — that is what makes a wrapped list item line up under its own text
    /// instead of under its bullet.
    fn flush(&mut self, first: Vec<Span<'static>>, cont: Vec<Span<'static>>) {
        if self.spans.is_empty() {
            return;
        }
        let lead = span_cols(&first).max(span_cols(&cont));
        let body_width = self.width.saturating_sub(lead).max(1);
        let spans = std::mem::take(&mut self.spans);
        for (i, line) in wrap_line_spans(&spans, body_width).into_iter().enumerate() {
            let mut row = if i == 0 { first.clone() } else { cont.clone() };
            row.extend(line.spans);
            self.out.push(Line::from(row));
        }
    }

    /// Flush a block that carries only the ambient rail/indent.
    fn flush_plain(&mut self) {
        let mut first = self.rail();
        let pad = self.lists.len().saturating_sub(1) * LIST_INDENT;
        if pad > 0 {
            first.push(Span::raw(" ".repeat(pad)));
        }
        let cont = first.clone();
        self.flush(first, cont);
    }

    fn blank_line(&mut self) {
        if self.out.last().is_some_and(line_is_blank) || self.out.is_empty() {
            return;
        }
        self.out.push(Line::default());
    }

    // ---- events --------------------------------------------------------

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(c) => {
                // Inline code keeps its ticks: a terminal has no background
                // shading that survives every theme, and a bare word is
                // indistinguishable from prose.
                let s = self.cur().fg(palette().color(SemanticColor::Accent));
                self.push_text(&format!("`{c}`"), s);
            }
            Event::SoftBreak => {
                let s = self.cur();
                self.push_text(" ", s);
            }
            Event::HardBreak => {
                self.flush_plain();
            }
            Event::Rule => {
                self.blank_line();
                self.out.push(Line::from(Span::styled(
                    "\u{2500}".repeat(self.width.min(60)),
                    style(SemanticColor::Dim),
                )));
                self.blank_line();
            }
            Event::TaskListMarker(done) => self.task = Some(done),
            Event::Html(t) | Event::InlineHtml(t) => self.html(&t),
            // Footnotes are not in the enabled flag set, so no event for them
            // can arrive here; anything else is inert by construction.
            _ => {}
        }
    }

    fn text(&mut self, t: &str) {
        if let Some(code) = self.code.as_mut() {
            code.body.push_str(t);
            return;
        }
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(t);
            return;
        }
        if let Some(buf) = self.label_buf.as_mut() {
            buf.push_str(t);
            return;
        }
        // Content that is not the marker settles the question: this quote is
        // an ordinary one, and nothing later in it may be eaten as a label.
        self.expect_admonition_label = false;
        let s = self.cur();
        self.push_text(t, s);
    }

    /// The only HTML this renderer acts on is [`enhance`]'s own mermaid
    /// placeholder. Everything else is model-authored and renders as the text
    /// it is — a terminal cannot execute it, but silently dropping it would
    /// hide content the model meant to show.
    fn html(&mut self, t: &str) {
        let trimmed = t.trim();
        if let Some(rest) = trimmed.strip_prefix(MERMAID_PLACEHOLDER_PREFIX) {
            if let Some(idx) = rest
                .strip_suffix("-->")
                .and_then(|n| n.parse::<usize>().ok())
            {
                let source = self.blocks.iter().find_map(|b| match b {
                    Block::Mermaid { index, source } if *index == idx => Some(source.clone()),
                    _ => None,
                });
                if let Some(source) = source {
                    self.flush_plain();
                    self.blank_line();
                    let lines: Vec<String> = source
                        .lines()
                        .map(std::string::ToString::to_string)
                        .collect();
                    let width = self.width;
                    render_code_block("mermaid", &lines, width, &mut self.out);
                    return;
                }
            }
        }
        let s = self.cur();
        self.push_text(t, s);
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.blank_line(),
            Tag::Heading { level, .. } => {
                self.blank_line();
                let base = style(SemanticColor::Accent).add_modifier(Modifier::BOLD);
                // `h1`/`h2` keep a visible hash run: a bold line in a
                // transcript already full of bold is not a level.
                let hashes = match level {
                    HeadingLevel::H1 => "# ",
                    HeadingLevel::H2 => "## ",
                    HeadingLevel::H3 => "### ",
                    _ => "",
                };
                if !hashes.is_empty() {
                    self.spans
                        .push(Span::styled(hashes.to_string(), style(SemanticColor::Dim)));
                }
                self.styles.push(base);
            }
            Tag::BlockQuote(_) => {
                self.blank_line();
                self.quotes.push(None);
                self.expect_admonition_label = true;
            }
            Tag::CodeBlock(kind) => {
                self.flush_plain();
                self.blank_line();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeAcc {
                    lang,
                    body: String::new(),
                });
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.blank_line();
                } else {
                    // A nested list interrupts its parent's item. Emit the
                    // parent's own text first, or the two run together on one
                    // row: `• outerinner`.
                    self.flush_item();
                }
                self.lists.push(start);
            }
            Tag::Item => self.item_owes_marker = true,
            Tag::Emphasis => {
                let s = self.cur().add_modifier(Modifier::ITALIC);
                self.styles.push(s);
            }
            Tag::Strong => {
                let s = self.cur().add_modifier(Modifier::BOLD);
                self.styles.push(s);
                if self.expect_admonition_label {
                    self.label_buf = Some(String::new());
                }
            }
            Tag::Strikethrough => {
                let s = self.cur().add_modifier(Modifier::CROSSED_OUT);
                self.styles.push(s);
            }
            Tag::Link { dest_url, .. } => {
                self.link = Some(LinkAcc {
                    url: dest_url.to_string(),
                    start_span: self.spans.len(),
                });
                let s = self
                    .cur()
                    .fg(palette().color(SemanticColor::Link))
                    .add_modifier(Modifier::UNDERLINED);
                self.styles.push(s);
            }
            Tag::Table(_) => {
                self.flush_plain();
                self.blank_line();
                self.table = Some(TableAcc::default());
            }
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
            TagEnd::Paragraph | TagEnd::Heading(_) => {
                if matches!(tag, TagEnd::Heading(_)) {
                    self.styles.pop();
                }
                if self.lists.is_empty() {
                    self.flush_plain();
                } else {
                    self.flush_item();
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_plain();
                self.quotes.pop();
                self.expect_admonition_label = false;
                self.blank_line();
            }
            TagEnd::CodeBlock => self.close_code(),
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank_line();
                }
            }
            TagEnd::Item => {
                // A loose list's item ends after its paragraph already
                // flushed; a tight one's text is still pending.
                self.flush_item();
            }
            TagEnd::Strong => {
                let strong = self.styles.pop().unwrap_or_default();
                let Some(buf) = self.label_buf.take() else {
                    return;
                };
                self.expect_admonition_label = false;
                // `enhance` wrote `**[!WARNING]**` as the quote's first inline
                // run. Consume it: the rail's colour and the label say it
                // once, and leaving the marker in the prose says it twice.
                let kind = buf
                    .strip_prefix("[!")
                    .and_then(|r| r.strip_suffix(']'))
                    .and_then(AdmonitionKind::parse);
                match kind {
                    Some(kind) => {
                        if let Some(slot) = self.quotes.last_mut() {
                            *slot = Some(kind);
                        }
                        self.spans.push(Span::styled(
                            format!("{} ", kind.label()),
                            style(kind.color()).add_modifier(Modifier::BOLD),
                        ));
                    }
                    // Bold text that merely opened a quote. It is ordinary
                    // content and has to reach the screen — swallowing it
                    // because the guess was wrong would delete a sentence.
                    None => self.push_text(&buf, strong),
                }
            }
            TagEnd::Emphasis | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                if let Some(link) = self.link.take() {
                    // A label that already IS its destination (what
                    // `linkify_bare_urls` produces from a bare URL) would
                    // otherwise print the same string twice.
                    let label: String = self.spans[link.start_span..]
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect();
                    if label.trim() != link.url.trim() {
                        self.spans.push(Span::styled(
                            format!(" ({})", link.url),
                            style(SemanticColor::Dim),
                        ));
                    }
                }
            }
            TagEnd::Table => self.close_table(),
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.rows.push(std::mem::take(&mut t.current));
                    t.header_rows = t.rows.len();
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.rows.push(std::mem::take(&mut t.current));
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

    /// Emit the pending inline run as a list item: marker on the first row,
    /// aligned padding on the rest.
    fn flush_item(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        // Only the first flush of an item wears the marker. A later
        // paragraph of the same (loose) item is body text that lines up
        // under it.
        let owed = std::mem::take(&mut self.item_owes_marker);
        let marker = match self.lists.last_mut() {
            Some(Some(n)) if owed => {
                let m = format!("{n}. ");
                *n += 1;
                m
            }
            Some(Some(n)) => " ".repeat(format!("{n}. ").len()),
            _ if owed => "\u{2022} ".to_string(),
            _ => "  ".to_string(),
        };
        let check = self.task.take().map(|done| {
            let mark = if done { "[x] " } else { "[ ] " };
            Span::styled(
                mark.to_string(),
                style(if done {
                    SemanticColor::ToolOk
                } else {
                    SemanticColor::Dim
                }),
            )
        });

        let mut first = self.rail();
        let pad = self.lists.len().saturating_sub(1) * LIST_INDENT;
        if pad > 0 {
            first.push(Span::raw(" ".repeat(pad)));
        }
        let mut cont = first.clone();
        first.push(Span::styled(marker.clone(), style(SemanticColor::Accent)));
        cont.push(Span::raw(
            " ".repeat(UnicodeWidthStr::width(marker.as_str())),
        ));
        if let Some(check) = check {
            let cols = UnicodeWidthStr::width(check.content.as_ref());
            first.push(check);
            cont.push(Span::raw(" ".repeat(cols)));
        }
        self.flush(first, cont);
    }

    fn close_code(&mut self) {
        let Some(code) = self.code.take() else { return };
        let body: Vec<String> = code
            .body
            .strip_suffix('\n')
            .unwrap_or(&code.body)
            .lines()
            .map(std::string::ToString::to_string)
            .collect();
        let width = self.width;
        render_code_block(&code.lang, &body, width, &mut self.out);
    }

    fn close_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        render_table(&table, self.width, &mut self.out);
    }
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|s| s.content.trim().is_empty())
}

fn span_cols(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// A GFM table as box-drawn rows.
///
/// Column widths are measured with `unicode-width`, not `len()`: a CJK cell is
/// two columns per character, and padding it by bytes puts every following
/// column in the wrong place on exactly the content most likely to need a
/// table.
fn render_table(table: &TableAcc, width: usize, out: &mut Vec<Line<'static>>) {
    if table.rows.is_empty() {
        return;
    }
    let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    let mut widths = vec![0usize; cols];
    for row in &table.rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }
    // Fit the budget by shrinking the widest column first, so one long cell
    // does not squeeze every other column to nothing.
    let border_cols = cols * 3 + 1;
    while widths.iter().sum::<usize>() + border_cols > width {
        let Some(widest) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .map(|(i, _)| i)
        else {
            break;
        };
        if widths[widest] <= 1 {
            break;
        }
        widths[widest] -= 1;
    }

    let border = style(SemanticColor::Dim);
    let rule = |l: &str, m: &str, r: &str| {
        let mut s = String::from(l);
        for (i, w) in widths.iter().enumerate() {
            if i > 0 {
                s.push_str(m);
            }
            s.push_str(&"\u{2500}".repeat(w + 2));
        }
        s.push_str(r);
        s
    };

    out.push(Line::from(Span::styled(
        rule("\u{250c}", "\u{252c}", "\u{2510}"),
        border,
    )));
    for (r, row) in table.rows.iter().enumerate() {
        let header = r < table.header_rows;
        let mut spans = vec![Span::styled("\u{2502}".to_string(), border)];
        for (i, w) in widths.iter().enumerate() {
            let cell = row.get(i).map_or("", String::as_str);
            let text = clip_to_cols(cell, *w);
            let pad = w.saturating_sub(UnicodeWidthStr::width(text.as_str()));
            let cell_style = if header {
                style(SemanticColor::Accent).add_modifier(Modifier::BOLD)
            } else {
                style(SemanticColor::Fg)
            };
            spans.push(Span::styled(
                format!(" {text}{} ", " ".repeat(pad)),
                cell_style,
            ));
            spans.push(Span::styled("\u{2502}".to_string(), border));
        }
        out.push(Line::from(spans));
        if header && r + 1 == table.header_rows {
            out.push(Line::from(Span::styled(
                rule("\u{251c}", "\u{253c}", "\u{2524}"),
                border,
            )));
        }
    }
    out.push(Line::from(Span::styled(
        rule("\u{2514}", "\u{2534}", "\u{2518}"),
        border,
    )));
}

/// Truncate to a column budget on char boundaries, with an ellipsis when
/// anything was dropped.
fn clip_to_cols(text: &str, cols: usize) -> String {
    if UnicodeWidthStr::width(text) <= cols {
        return text.to_string();
    }
    if cols <= 1 {
        return "\u{2026}".to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + w > cols - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('\u{2026}');
    out
}

/// Frozen-prefix half of an incremental streaming render — see
/// [`markdown_to_lines_incremental`]. `Rc`-shared so the cache and the
/// current frame can both hold the prefix without a deep copy.
#[derive(Debug)]
pub struct StreamPrefix {
    /// Byte offset into the source text up to which `lines` renders.
    pub safe_offset: usize,
    /// Pane width the prefix was wrapped for; a resize invalidates (mirrors
    /// the width check `CachedEntry` does for the whole-message cache).
    pub width: u16,
    /// Rendered lines for exactly `text[..safe_offset]` — nothing more. The
    /// unfrozen tail is never baked in here (it keeps changing byte for
    /// byte; a snapshot of it would resurface as a stale duplicate the next
    /// time the boundary advanced).
    pub lines: Rc<Vec<Line<'static>>>,
}

/// One frame of an incremental streaming render: the frozen prefix (shared
/// with the cache, so holding it costs one `Rc` bump) plus the freshly
/// rendered unfrozen tail.
#[derive(Debug)]
pub struct StreamLines {
    pub prefix: Rc<Vec<Line<'static>>>,
    pub tail: Vec<Line<'static>>,
}

impl StreamLines {
    /// Total rendered line count across the prefix/tail seam.
    pub fn line_count(&self) -> usize {
        self.prefix.len() + self.tail.len()
    }

    /// Borrow line `i` across the prefix/tail seam.
    pub fn get(&self, i: usize) -> Option<&Line<'static>> {
        if i < self.prefix.len() {
            self.prefix.get(i)
        } else {
            self.tail.get(i - self.prefix.len())
        }
    }
}

/// Incremental variant of [`markdown_to_lines`] for a still-growing message.
///
/// `cache` holds the [`StreamPrefix`] from the previous call. Only the text
/// from `cache.safe_offset` to the new
/// `shared_ui_logic::markdown_stream::safe_freeze_offset` boundary is
/// re-converted; the frozen prefix is reused via `Rc` with zero deep copies.
/// Falls back to a full re-run on the very first call (`cache == None`) and
/// whenever `width` no longer matches the cached one (a resize mid-stream).
///
/// A stale cached offset (longer than `text` or off its char boundary —
/// possible only after a wholesale content swap the cache didn't observe) is
/// treated as "no cache": the prefix is dropped and the whole text renders as
/// tail, so a stale prefix can never paint text that is no longer there.
///
/// # The seam is a BLOCK boundary, not a line boundary
///
/// `block_freeze_offset`, not `safe_freeze_offset`. The latter advances past
/// any complete line outside a fence, which is exactly right for a renderer
/// that maps one source line to one output line — this file's was one until
/// pulldown-cmark replaced it. A block parser cut there sees two documents
/// where the finished text is one, and the prefix is frozen, so it never
/// settles: a half-arrived table stays a header row plus literal pipes.
///
/// It is also what makes this function's output equal
/// [`markdown_to_lines`]'s on the same complete text — the invariant the
/// chat area's window arithmetic rests on, since it measures the transcript
/// through this path and slices it through that one.
///
/// The Panel's HTML cache (`extend_stable_prefix`) still takes the line
/// boundary and still chops a streaming table; its render is transient
/// (a settled message re-renders whole), so it is a flicker rather than a
/// frozen mistake. Moving it over is a one-line change and a measurement
/// this session cannot make.
pub fn markdown_to_lines_incremental(
    text: &str,
    width: u16,
    cache: &mut Option<StreamPrefix>,
) -> StreamLines {
    let prev_offset = match cache {
        Some(p) if p.width == width => p.safe_offset,
        _ => {
            // No cache yet, or the pane was resized — start over.
            *cache = None;
            0
        }
    };
    let (prefix, tail_start) =
        match shared_ui_logic::markdown_stream::block_freeze_offset(text, prev_offset) {
            Some(new_offset) if new_offset > prev_offset => {
                // The safe prefix grew. Re-run the full conversion on it —
                // the prefix is finished text, so it gets the finished-text
                // pre-pass (fences and admonitions are whole by definition of
                // the boundary), which is what `streaming = false` asks for.
                let lines = Rc::new(markdown_to_lines(&text[..new_offset], width));
                *cache = Some(StreamPrefix {
                    safe_offset: new_offset,
                    width,
                    lines: Rc::clone(&lines),
                });
                (lines, new_offset)
            }
            _ => match cache {
                Some(p) if p.safe_offset <= text.len() && text.is_char_boundary(p.safe_offset) => {
                    let off = p.safe_offset;
                    (Rc::clone(&p.lines), off)
                }
                _ => {
                    // No usable cache: nothing is frozen, everything is tail.
                    if cache.is_some() {
                        *cache = None;
                    }
                    (Rc::new(Vec::new()), 0)
                }
            },
        };
    let tail_text = &text[tail_start..];
    let mut tail = if tail_text.is_empty() {
        Vec::new()
    } else {
        // The tail is unfinished: an open fence has no closing line yet, so
        // the block-level pre-pass would mis-read it.
        render(tail_text, width, true)
    };
    // The blank row between two blocks belongs to the seam, and neither half
    // can emit it: each render trims its own trailing blanks and suppresses a
    // leading one. Without this the incremental build is exactly one row
    // shorter than the settled build per frozen block, and the chat area
    // measures with one and slices with the other.
    if !prefix.is_empty() && !tail.is_empty() {
        tail.insert(0, Line::default());
    }
    StreamLines { prefix, tail }
}

/// A fenced block as a framed, gutter-prefixed run of rows.
///
/// Highlighting is per BLOCK, not per line, so a multi-line string or block
/// comment carries its scanner state.
fn render_code_block(lang: &str, lines: &[String], width: usize, result: &mut Vec<Line<'static>>) {
    let highlighted = highlight::highlight_block(palette(), lang, lines);
    render_code_block_with(lang, lines, highlighted.as_deref(), width, result);
}

/// The body of [`render_code_block`] with the highlighting supplied rather
/// than looked up.
///
/// Split out so the un-highlighted shape — what every session renders until
/// the background `syntect` load lands — is reachable from a test without
/// racing that load or mutating the ambient palette.
///
/// `highlighted` is `None` for plain, or one entry per line of `lines`.
fn render_code_block_with(
    lang: &str,
    lines: &[String],
    highlighted: Option<&[Vec<(Style, String)>]>,
    width: usize,
    result: &mut Vec<Line<'static>>,
) {
    let border_style = style(SemanticColor::CodeBorder);
    let code_style = style(SemanticColor::Fg);
    let inner_width = if width > 4 { width - 2 } else { width };

    // Top border: ┌─ lang ──────
    let label = if lang.is_empty() {
        String::new()
    } else {
        format!(" {lang} ")
    };
    let label_width = UnicodeWidthStr::width(label.as_str());
    let dash_count = inner_width.saturating_sub(label_width + 1);
    let top = format!("\u{250c}\u{2500}{}{}", label, "\u{2500}".repeat(dash_count));
    result.push(Line::from(Span::styled(top, border_style)));

    // Code lines. Wrap each to the inner width (minus the "│ " gutter) so a long
    // code line becomes multiple physical rows instead of overflowing the pane.
    // The chat scroll window is computed from the logical line count, so an
    // unbounded row here would desync the height and clip the newest content.
    let code_wrap_width = inner_width.saturating_sub(2).max(1);
    for (i, code_line) in lines.iter().enumerate() {
        if code_line.is_empty() {
            result.push(Line::from(Span::styled(
                "\u{2502} ".to_string(),
                border_style,
            )));
            continue;
        }
        // `None` means plain — syntect is still loading, the language is
        // unknown, or the palette does not paint RGB.
        let spans: Vec<Span<'static>> = match highlighted.and_then(|rows| rows.get(i)) {
            Some(row) => row
                .iter()
                .map(|(style, text)| Span::styled(text.clone(), *style))
                .collect(),
            None => vec![Span::styled(code_line.clone(), code_style)],
        };
        // Through the same width-aware wrapper the prose uses, so a wrapped
        // code line keeps each token's colour on both halves.
        for row in wrap_line_spans(&spans, code_wrap_width) {
            let mut out = vec![Span::styled("\u{2502} ".to_string(), border_style)];
            out.extend(row.spans);
            result.push(Line::from(out));
        }
    }

    // Bottom border: └──────────────
    let bottom = format!("\u{2514}{}", "\u{2500}".repeat(inner_width));
    result.push(Line::from(Span::styled(bottom, border_style)));
}

/// Wrap a line of spans if total visual width exceeds the given width,
/// preserving each span's style across the wrap boundaries.
///
/// The concatenated plain text is wrapped with `textwrap`; each resulting row is
/// mapped back to a byte range in the plain text and the styled spans are
/// re-sliced against that range, so bold/italic/inline-code/link styling (and
/// the colored bullet/quote prefix) survive on every wrapped row — including
/// spans that straddle a wrap boundary, which are split with their style carried
/// to both halves.
fn wrap_line_spans(spans: &[Span<'static>], width: usize) -> Vec<Line<'static>> {
    if width == 0 || spans.is_empty() {
        return vec![Line::from(spans.to_vec())];
    }

    // Fast path: fits on one line — keep the styled spans intact.
    let total_width: usize = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if total_width <= width {
        return vec![Line::from(spans.to_vec())];
    }

    // Build the concatenated plain text plus a parallel map of
    // (byte_start, byte_end, style) segments so every byte offset in `plain`
    // can be traced back to its originating span's style.
    let mut plain = String::new();
    let mut segments: Vec<(usize, usize, Style)> = Vec::with_capacity(spans.len());
    for span in spans {
        let start = plain.len();
        plain.push_str(span.content.as_ref());
        let end = plain.len();
        if end > start {
            segments.push((start, end, span.style));
        }
    }

    // Wrap, then map each row back to its byte range in `plain` and re-slice the
    // styled segments. textwrap may drop the whitespace it broke on, so locate
    // each row's content starting at/after a running cursor rather than assuming
    // the rows are byte-adjacent.
    let wrapped = textwrap::wrap(&plain, width);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(wrapped.len());
    let mut cursor = 0usize;
    let mut row_spans: Vec<Span<'static>> = Vec::new();
    for row in &wrapped {
        let row = row.as_ref();
        let row_start = plain[cursor..].find(row).map_or(cursor, |off| cursor + off);
        let row_end = row_start + row.len();
        cursor = row_end;

        row_spans.clear();
        for (seg_start, seg_end, style) in &segments {
            let lo = (*seg_start).max(row_start);
            let hi = (*seg_end).min(row_end);
            if lo < hi {
                if let Some(text) = plain.get(lo..hi) {
                    if !text.is_empty() {
                        row_spans.push(Span::styled(text.to_string(), *style));
                    }
                }
            }
        }
        if row_spans.is_empty() {
            row_spans.push(Span::raw(row.to_string()));
        }
        lines.push(Line::from(std::mem::take(&mut row_spans)));
    }

    if lines.is_empty() {
        lines.push(Line::from(spans.to_vec()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn render_text(md: &str, width: u16) -> Vec<String> {
        text_of(&markdown_to_lines(md, width))
    }

    /// A GFM table renders as a grid, not as its source pipes.
    ///
    /// # When this goes red
    ///
    /// Dropping `ENABLE_TABLES`, or reverting to a line scanner. Asserting on
    /// the box-drawing border rather than on the cell text is deliberate: the
    /// cell text is present either way — that is exactly how the old renderer
    /// passed for a reader skimming the output.
    #[test]
    fn a_table_renders_as_a_grid() {
        let out = render_text("| name | qty |\n| --- | --- |\n| bolt | 12 |", 60);
        assert!(
            out.iter().any(|l| l.contains('\u{250c}')),
            "no top border: {out:?}"
        );
        assert!(
            out.iter()
                .any(|l| l.contains("bolt") && l.contains('\u{2502}')),
            "cell not in a grid: {out:?}"
        );
        assert!(
            !out.iter().any(|l| l.contains("---")),
            "delimiter row leaked as text: {out:?}"
        );
    }

    /// CJK cells are padded by columns, not by bytes.
    #[test]
    fn a_cjk_table_keeps_its_columns_aligned() {
        let out = render_text("| 名 | v |\n| - | - |\n| 中文字 | 1 |\n| a | 2 |", 60);
        let body: Vec<&String> = out
            .iter()
            .filter(|l| l.contains('1') || l.contains('2'))
            .collect();
        assert_eq!(body.len(), 2, "{out:?}");
        let bar_at = |s: &str| s.char_indices().filter(|(_, c)| *c == '\u{2502}').count();
        assert_eq!(bar_at(body[0]), bar_at(body[1]), "{out:?}");
        let cols = |s: &str| UnicodeWidthStr::width(s);
        assert_eq!(cols(body[0]), cols(body[1]), "ragged: {out:?}");
    }

    /// An ordered list numbers its items.
    ///
    /// # When this goes red
    ///
    /// The old renderer had no ordered-list branch at all: `1.` reached the
    /// screen as literal text, so `1. a\n1. b` printed `1.` twice. This
    /// asserts the second item is `2.`, which only a parser that counts can
    /// produce.
    #[test]
    fn an_ordered_list_counts() {
        let out = render_text("1. first\n1. second", 40);
        assert!(out.iter().any(|l| l.contains("1. first")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("2. second")), "{out:?}");
    }

    /// Columns before the first non-space character.
    fn lead_cols(s: &str) -> usize {
        UnicodeWidthStr::width(&s[..s.len() - s.trim_start().len()])
    }

    /// A loose item's second paragraph is body text, not a second item.
    ///
    /// # When this goes red
    ///
    /// Flushing an item's marker on every paragraph rather than the first.
    /// The ordered case is the one that cannot be mistaken for a styling
    /// choice: the list would count 1, 2 where the source has one item.
    #[test]
    fn a_loose_item_wears_its_marker_once() {
        let out = render_text("1. one\n\n   still one\n\n2. two\n", 40);
        let joined = out.join("\n");
        assert!(joined.contains("1. one"), "{out:?}");
        assert!(joined.contains("2. two"), "{out:?}");
        assert!(
            !joined.contains("2. still one") && !joined.contains("3. two"),
            "the continuation took an ordinal: {out:?}"
        );
        let cont = out.iter().find(|l| l.contains("still one")).expect("cont");
        assert_eq!(lead_cols(cont), 3, "not aligned under the text: {out:?}");
    }

    #[test]
    fn a_nested_list_indents_under_its_parent() {
        let out = render_text("- outer\n  - inner", 40);
        let outer = out.iter().position(|l| l.contains("outer")).expect("outer");
        let inner = out.iter().position(|l| l.contains("inner")).expect("inner");
        assert_ne!(outer, inner, "items merged onto one row: {out:?}");
        assert!(lead_cols(&out[inner]) > lead_cols(&out[outer]), "{out:?}");
    }

    /// A wrapped list item's continuation lines up under its own text, not
    /// under its bullet.
    ///
    /// Measured in COLUMNS. `str::find` returns a byte offset, and the bullet
    /// is a three-byte `•`, so a byte-based version of this test reports the
    /// text starting at column 4 when it starts at column 2 — and fails a
    /// correct renderer.
    #[test]
    fn a_wrapped_item_hangs_under_its_text() {
        let out = render_text("- alpha beta gamma delta epsilon zeta", 20);
        assert!(out.len() > 1, "did not wrap: {out:?}");
        let byte = out[0].find("alpha").expect("first row has the text");
        let first_text = UnicodeWidthStr::width(&out[0][..byte]);
        assert_eq!(lead_cols(&out[1]), first_text, "not hung: {out:?}");
    }

    /// A task list paints a checkbox rather than the literal marker.
    ///
    /// Uppercase `[X]` on purpose — a lowercase source renders identically
    /// whether or not the marker was parsed, so it would pass both ways.
    #[test]
    fn a_task_list_paints_its_checkbox() {
        let out = render_text("- [ ] open\n- [X] closed", 40);
        assert!(out.iter().any(|l| l.contains("[ ] open")), "{out:?}");
        assert!(
            out.iter().any(|l| l.contains("[x] closed")),
            "not normalised — parsed as text: {out:?}"
        );
    }

    /// An admonition takes the rail colour of its kind and drops the literal
    /// `[!WARNING]` marker `enhance` wrote for the renderers to find.
    #[test]
    fn an_admonition_is_labelled_not_left_as_its_marker() {
        let lines = markdown_to_lines("> [!WARNING] disk is full\n", 60);
        let out = text_of(&lines);
        assert!(
            out.iter()
                .any(|l| l.contains("WARNING") && l.contains("disk is full")),
            "{out:?}"
        );
        assert!(
            !out.iter().any(|l| l.contains("[!WARNING]")),
            "raw marker survived: {out:?}"
        );
        let rail = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains(QUOTE_RAIL))
            .expect("a rail span");
        assert_eq!(
            rail.style.fg,
            Some(palette().color(SemanticColor::AdmonitionWarning)),
            "rail not painted by kind"
        );
    }

    /// A mermaid fence shows its source in a frame. The TUI draws no diagram
    /// (spec §6) — but it must not drop the block either.
    #[test]
    fn a_mermaid_fence_shows_its_source_framed() {
        let out = render_text("```mermaid\ngraph TD;\n  A-->B;\n```", 60);
        assert!(
            out.iter().any(|l| l.contains("mermaid")),
            "no label: {out:?}"
        );
        assert!(
            out.iter().any(|l| l.contains("graph TD;")),
            "no source: {out:?}"
        );
        assert!(
            !out.iter().any(|l| l.contains(MERMAID_PLACEHOLDER_PREFIX)),
            "placeholder leaked: {out:?}"
        );
    }

    /// Strikethrough is a modifier, not the surviving `~~` markers.
    #[test]
    fn strikethrough_is_styled_not_spelled() {
        let lines = markdown_to_lines("~~gone~~", 40);
        let out = text_of(&lines);
        assert!(
            !out.iter().any(|l| l.contains("~~")),
            "markers left: {out:?}"
        );
        assert!(
            lines.iter().flat_map(|l| l.spans.iter()).any(|s| {
                s.content.contains("gone") && s.style.add_modifier.contains(Modifier::CROSSED_OUT)
            }),
            "not struck: {out:?}"
        );
    }

    /// A link shows its destination dimmed after the label — except when the
    /// label already is the destination, which is what a bare URL becomes.
    #[test]
    fn a_link_shows_its_url_once() {
        let out = render_text("see [docs](https://a.io/x) here", 60);
        assert_eq!(out.concat().matches("https://a.io/x").count(), 1, "{out:?}");
        assert!(out.concat().contains("docs"), "{out:?}");

        let bare = render_text("see https://a.io/x here", 60);
        assert_eq!(
            bare.concat().matches("https://a.io/x").count(),
            1,
            "bare URL printed twice: {bare:?}"
        );
    }

    /// Highlighted and unhighlighted code blocks occupy exactly the same
    /// rows.
    ///
    /// # Why this is the guard
    ///
    /// `syntect` loads on a background thread, so the first code blocks of a
    /// session render plain and later ones render coloured. If the two laid
    /// out differently, the transcript would reflow under the reader the
    /// moment the load landed — and the chat area's scroll arithmetic, which
    /// is built on these row counts, would be measuring one thing and
    /// slicing another.
    ///
    /// Rendered at two widths, including one that forces the code lines to
    /// wrap: wrapping is where a per-token split could change the row count,
    /// because the highlighted path goes through `wrap_line_spans` with many
    /// spans where the plain path has one.
    ///
    /// Both sides are built explicitly — the highlighted one through
    /// `highlight_block_blocking`, because the ambient palette in a test
    /// binary paints no RGB and the production path would hand back plain
    /// rows for BOTH sides, making this pass while comparing nothing.
    ///
    /// # When this goes red
    ///
    /// Highlighting that trims, merges or drops a line — or a wrap that
    /// breaks differently once the line is several spans instead of one.
    #[test]
    fn highlighting_never_changes_the_rows_only_their_colour() {
        let body: Vec<String> = [
            "fn main() {",
            "",
            "    let x = \"a string that is quite long indeed\"; // trailing note",
            "}",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let hl = highlight::highlight_block_blocking(highlight::rgb_palette(), "rust", &body)
            .expect("the default syntax set has rust");
        for width in [30usize, 72] {
            let mut plain = Vec::new();
            render_code_block_with("rust", &body, None, width, &mut plain);
            let mut coloured = Vec::new();
            render_code_block_with("rust", &body, Some(&hl), width, &mut coloured);

            assert_eq!(
                text_of(&coloured),
                text_of(&plain),
                "width {width}: highlighting changed the rows"
            );
            // …and it really did colour something, or the assertion above is
            // comparing plain against plain.
            assert!(
                coloured
                    .iter()
                    .flat_map(|l| l.spans.iter())
                    .any(|s| matches!(s.style.fg, Some(ratatui::style::Color::Rgb(..)))),
                "nothing was highlighted"
            );
        }
    }

    /// An unclosed fence still shows what has arrived. A streaming code block
    /// that stayed invisible until its closing ``` would be a blank pane for
    /// the whole time the model is writing it.
    #[test]
    fn an_unterminated_fence_still_renders_its_body() {
        let out = render_text("```rust\nlet x = 1;\n", 60);
        assert!(out.iter().any(|l| l.contains("let x = 1;")), "{out:?}");
    }

    /// Nothing rendered here may exceed the width it was given: the chat
    /// scroll window is computed from this line count, so an overflowing row
    /// desyncs the viewport rather than just looking wrong.
    #[test]
    fn no_row_exceeds_the_requested_width() {
        let md = "# A heading that runs on and on and on\n\n\
                  | a very wide column indeed | and another one |\n\
                  | --- | --- |\n\
                  | with a long cell value here | and more text |\n\n\
                  - a list item long enough to need wrapping at this width\n\n\
                  > [!NOTE] a quoted admonition that also needs to wrap somewhere\n\n\
                  ```sh\necho a-very-long-command-line-that-will-not-fit\n```\n";
        for width in [20u16, 40, 80] {
            for line in markdown_to_lines(md, width) {
                let cols: usize = line
                    .spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                assert!(
                    cols <= width as usize,
                    "width {width}: {cols} cols in {:?}",
                    text_of(std::slice::from_ref(&line))
                );
            }
        }
    }

    /// Rendering complete text incrementally gives the same rows as
    /// rendering it settled.
    ///
    /// # Why this is the invariant that matters
    ///
    /// `chat_area` measures the transcript's height through the incremental
    /// path and slices the reference build out of the settled one. A
    /// one-row disagreement is not cosmetic there — the scroll window lands
    /// off by a row and clips the newest line, which reads as "the answer
    /// stopped early".
    ///
    /// # When this goes red
    ///
    /// Swapping `block_freeze_offset` back for `safe_freeze_offset` (a
    /// paragraph freezes mid-soft-break and renders as two paragraphs), or
    /// dropping the seam's blank separator (one row short per frozen block).
    /// Both were measured red while writing this.
    #[test]
    fn the_incremental_render_equals_the_settled_one() {
        let corpus = [
            "one paragraph only",
            "answer\nwith a second line",
            "first\n\nsecond\n\nthird\n",
            "intro\n\n```rust\nlet x = 1;\n```\n\nafter\n",
            "| a | b |\n| - | - |\n| 1 | 2 |\n\ntail\n",
            "- one\n- two\n\n# Heading\n\nbody text here\n",
            "> [!NOTE] quoted\n\nplain\n",
        ];
        for md in corpus {
            let mut cache = None;
            let s = markdown_to_lines_incremental(md, 40, &mut cache);
            let incremental: Vec<Line<'static>> = (0..s.line_count())
                .filter_map(|i| s.get(i))
                .cloned()
                .collect();
            let settled = markdown_to_lines(md, 40);
            assert_eq!(
                text_of(&incremental),
                text_of(&settled),
                "seam differs for {md:?}"
            );
        }
    }

    /// Inline emphasis is styling, not surviving markers.
    ///
    /// One test for four constructs because they share a mechanism (the
    /// style stack) and would fail together; the ones with their own failure
    /// mode — strikethrough, links, task markers — have their own tests.
    #[test]
    fn inline_emphasis_is_styled_and_its_markers_removed() {
        let lines = markdown_to_lines("**bold** and *italic* and `code`", 60);
        let out = text_of(&lines);
        let joined = out.concat();
        assert!(!joined.contains("**"), "bold markers left: {out:?}");
        assert!(!joined.contains('*'), "italic markers left: {out:?}");
        assert!(
            joined.contains("`code`"),
            "inline code lost its ticks: {out:?}"
        );
        let has = |needle: &str, m: Modifier| {
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.content.contains(needle) && s.style.add_modifier.contains(m))
        };
        assert!(has("bold", Modifier::BOLD), "{out:?}");
        assert!(has("italic", Modifier::ITALIC), "{out:?}");
    }

    #[test]
    fn headings_keep_a_level_marker() {
        let out = render_text("# One\n\n## Two\n\n### Three\n", 60);
        assert!(out.iter().any(|l| l.starts_with("# One")), "{out:?}");
        assert!(out.iter().any(|l| l.starts_with("## Two")), "{out:?}");
        assert!(out.iter().any(|l| l.starts_with("### Three")), "{out:?}");
    }

    /// A quote that is not an admonition keeps its bold text.
    ///
    /// # When this goes red
    ///
    /// The admonition detector buffers the first `**…**` of a blockquote to
    /// see whether it is a `[!KIND]` marker. If the "it wasn't" branch ever
    /// stops re-emitting what it buffered, that sentence disappears from the
    /// transcript — a silent deletion, not a formatting slip.
    #[test]
    fn a_plain_quote_keeps_bold_text_the_admonition_probe_looked_at() {
        let out = render_text("> **Note well:** something matters\n", 60);
        assert!(
            out.iter().any(|l| l.contains("Note well:")),
            "buffered text was swallowed: {out:?}"
        );
        assert!(
            out.iter().any(|l| l.contains(QUOTE_RAIL)),
            "no quote rail: {out:?}"
        );
    }

    #[test]
    fn incremental_conversion_reuses_the_cache_on_a_second_call_with_more_text() {
        let mut cache: Option<StreamPrefix> = None;
        // A blank line, because the boundary is block-granular: `para one\n`
        // alone is an open paragraph and freezes nothing.
        let first_text = "para one\n\n";
        let _first = markdown_to_lines_incremental(first_text, 80, &mut cache);
        let offset1 = cache.as_ref().map(|p| p.safe_offset);
        assert!(offset1.unwrap_or(0) > 0, "nothing froze");

        let grown_text = "para one\n\npara two\n";
        let second = markdown_to_lines_incremental(grown_text, 80, &mut cache);
        assert!(cache.as_ref().map(|p| p.safe_offset) >= offset1);
        let combined: Vec<Line<'static>> = second
            .prefix
            .iter()
            .cloned()
            .chain(second.tail.iter().cloned())
            .collect();
        assert_eq!(
            text_of(&combined),
            text_of(&markdown_to_lines(grown_text, 80))
        );
    }

    /// A tail rendered on one frame must never survive into the next
    /// alongside its replacement.
    #[test]
    fn incremental_conversion_does_not_duplicate_a_tail_baked_into_an_earlier_cache_write() {
        let mut cache: Option<StreamPrefix> = None;
        // The boundary advances (closing fence) while a non-empty tail
        // ("af") already sits past it.
        let text1 = "before\n\n```rust\ncode\n```\naf";
        markdown_to_lines_incremental(text1, 80, &mut cache);
        let offset1 = cache.as_ref().map(|p| p.safe_offset).unwrap_or(0);
        assert!(offset1 > 0, "the closed fence should have frozen");

        // The boundary does not move, but the tail grows.
        let text2 = "before\n\n```rust\ncode\n```\nafter";
        let second = markdown_to_lines_incremental(text2, 80, &mut cache);
        assert_eq!(cache.as_ref().map(|p| p.safe_offset), Some(offset1));
        let combined: Vec<Line<'static>> = second
            .prefix
            .iter()
            .cloned()
            .chain(second.tail.iter().cloned())
            .collect();
        assert_eq!(
            text_of(&combined),
            text_of(&markdown_to_lines(text2, 80)),
            "stale or duplicated tail"
        );
    }

    /// The `Rc`-sharing contract: across a no-advance frame the returned
    /// prefix is the SAME allocation as the cached one. The signature this
    /// replaced deep-copied the whole prefix `Vec` on every frame.
    #[test]
    fn incremental_conversion_shares_the_frozen_prefix_without_copying() {
        let mut cache: Option<StreamPrefix> = None;
        markdown_to_lines_incremental("para one\n\npartial", 80, &mut cache);
        let cached = Rc::clone(&cache.as_ref().expect("cache populated").lines);
        let frame = markdown_to_lines_incremental("para one\n\npartially", 80, &mut cache);
        assert!(
            Rc::ptr_eq(&cached, &frame.prefix),
            "a no-advance frame must reuse the cached prefix allocation"
        );
    }

    /// The streaming seam does not duplicate or drop text across a boundary
    /// advance.
    #[test]
    fn the_streaming_seam_neither_duplicates_nor_drops() {
        let full = "para one\n\nsecond para\n\nthird\n";
        let mut cache = None;
        let mut last = String::new();
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            let s = markdown_to_lines_incremental(&full[..end], 40, &mut cache);
            last = (0..s.line_count())
                .filter_map(|i| s.get(i))
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|sp| sp.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        for word in ["para one", "second para", "third"] {
            assert_eq!(last.matches(word).count(), 1, "{last:?}");
        }
    }
}
