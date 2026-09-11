// Chat area widget: renders the scrollable message list with support for
// user messages, assistant messages (with reasoning, tool blocks, markdown),
// system messages, and streaming cursors.

use std::collections::HashMap;

use ratatui::{
    layout::{Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use shared_ui_logic::transcript::{
    turn_summary_text, Modality, RowBody, RowStatus, ToolRow, TranscriptEntry,
};

use crate::tui::app::{AppState, Focus};
use crate::tui::markdown::{
    markdown_to_lines, markdown_to_lines_incremental, StreamLines, StreamPrefix,
};
use crate::tui::regions::RegionKind;
use crate::tui::theme::theme;

#[cfg(test)]
use super::tool_row::KEY_MODALITY;
use super::tool_row::{render_tool_group, render_tool_row};

/// A click target the current frame actually painted, in rows from the top of
/// the visible window.
///
/// Row indices rather than `Rect`s because this is produced by the line
/// builder, which knows nothing about where on the screen the window lands —
/// that translation happens once, in `render_chat_area`.
struct VisibleToggle {
    out_row: usize,
    row_id: String,
}

/// Per-message rendered-line cache, owned by `AppState` across frames (see
/// `render_chat_area`'s caller in `render.rs` for where it's threaded
/// through). Keyed by the message's index in `state.messages` — safe because
/// a cache entry also validates against the message's own variant kind and
/// content length before being trusted (see `build_all_lines_cached`); a
/// coincidental `(kind, len)` match at a shifted index is the only failure
/// mode, and it self-heals the next frame once content actually diverges.
#[derive(Debug, Default)]
pub struct LineCache {
    entries: HashMap<usize, CachedEntry>,
    /// Fine-grained incremental cache for the ONE currently-streaming
    /// message's markdown conversion (see `markdown_to_lines_incremental`).
    /// Distinct from `entries` above: that whole-message cache deliberately
    /// never caches a streaming message (its content grows every tick), so
    /// this is the only cache the streaming message gets, and it caches at
    /// the safe-prefix-offset granularity rather than the whole message.
    /// The `Rc`-shared prefix lines are reused across frames with zero deep
    /// copies; a mid-stream resize invalidates via `StreamPrefix::width`.
    streaming_markdown_cache: Option<StreamPrefix>,
    /// The message index this `streaming_markdown_cache` belongs to. Reset
    /// (along with the cache above) whenever the streaming message changes
    /// — e.g. a new turn starts streaming — so the new message doesn't
    /// inherit a stale safe-offset from the previous one.
    streaming_message_idx: Option<usize>,
}

#[derive(Debug)]
struct CachedEntry {
    kind: MessageKind,
    /// Sampled fingerprint of the message content (see
    /// [`content_fingerprint`]) — a bare length match can serve lines
    /// rendered from *different* content after a same-length replacement
    /// (e.g. a peer message inserted before the tail shifting indices).
    fingerprint: u64,
    width: u16,
    /// Which syntax-highlighting generation these lines were built with.
    ///
    /// Not derivable from the message: `syntect` loads on a background
    /// thread, so the same content at the same width renders plain before the
    /// load lands and coloured after. Without this key the transcript keeps
    /// whichever state each message happened to be rendered in, and a reader
    /// gets a mix of highlighted and plain code blocks that never converges.
    highlight_generation: u64,
    lines: Vec<Line<'static>>,
    /// Offsets into `lines` that toggle this entry's fold, from the renderer
    /// that emitted them. Cached beside the lines because they are only true
    /// of *these* lines: a cache hit that re-derived them would be re-deriving
    /// them from content that is no longer what was drawn.
    toggles: Vec<usize>,
}

/// Cheap discriminant for a `TranscriptEntry`, used only to invalidate the
/// cache safely across `messages.insert(at, ...)` (peer messages can be
/// inserted before the streaming tail, shifting its index — see
/// `app/events.rs::StreamEvent::...` peer-message handling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    ToolGroup,
    TurnSummary,
    System,
}

/// O(1) sampled content fingerprint for cache validation: length plus the
/// first and last 32 bytes. A full hash would re-scan every settled message
/// on every frame — the exact cost the cache exists to avoid — while a bare
/// length can't tell a same-length replacement apart from a hit.
fn content_fingerprint(content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let bytes = content.as_bytes();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut h);
    let edge = 32.min(bytes.len());
    bytes[..edge].hash(&mut h);
    bytes[bytes.len() - edge..].hash(&mut h);
    h.finish()
}

/// A tool row's rendered appearance changes without its text changing — the
/// status settles, a duration arrives, a body is replaced by a diff — so its
/// fingerprint has to cover those, not just a string.
fn tool_fingerprint(row: &ToolRow) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    row.id.hash(&mut h);
    row.summary.display_name.hash(&mut h);
    row.summary.args_text.hash(&mut h);
    row.expanded.hash(&mut h);
    match &row.status {
        RowStatus::Pending => 0u8.hash(&mut h),
        RowStatus::Running { .. } => 1u8.hash(&mut h),
        RowStatus::Ok { duration_ms } => {
            2u8.hash(&mut h);
            duration_ms.hash(&mut h);
        }
        RowStatus::Err {
            duration_ms,
            message,
        } => {
            3u8.hash(&mut h);
            duration_ms.hash(&mut h);
            message.hash(&mut h);
        }
    }
    match &row.body {
        RowBody::None => 0u8.hash(&mut h),
        RowBody::Text(t) => {
            1u8.hash(&mut h);
            content_fingerprint(t).hash(&mut h);
        }
        RowBody::FileChanges(c) => {
            2u8.hash(&mut h);
            c.len().hash(&mut h);
            for change in c {
                change.path.hash(&mut h);
                change.added.hash(&mut h);
                change.removed.hash(&mut h);
                change.hunks.len().hash(&mut h);
            }
        }
    }
    h.finish()
}

fn message_kind_and_fingerprint(message: &TranscriptEntry) -> (MessageKind, u64) {
    match message {
        TranscriptEntry::UserText { text, .. } => (MessageKind::User, content_fingerprint(text)),
        TranscriptEntry::AssistantText { markdown, .. } => {
            (MessageKind::Assistant, content_fingerprint(markdown))
        }
        TranscriptEntry::Reasoning { text, .. } => {
            (MessageKind::Reasoning, content_fingerprint(text))
        }
        // A RUNNING row is deliberately never cached by the caller (its
        // spinner is a function of the clock), so this fingerprint only has to
        // be right for settled ones.
        TranscriptEntry::Tool(row) => (MessageKind::Tool, tool_fingerprint(row)),
        TranscriptEntry::ToolGroup(g) => {
            let mut acc = 0u64;
            for r in &g.rows {
                acc = acc.rotate_left(7) ^ tool_fingerprint(r);
            }
            (MessageKind::ToolGroup, acc)
        }
        TranscriptEntry::TurnSummary(s) => (
            MessageKind::TurnSummary,
            content_fingerprint(&turn_summary_text(s)),
        ),
        TranscriptEntry::SystemNotice { text, .. } => {
            (MessageKind::System, content_fingerprint(text))
        }
    }
}

/// The docked control that undoes a scroll-up, and the key that does the same.
///
/// One string so the button and the key binding cannot drift apart — the
/// label is the only place a reader is told which key this is (判据 §1).
pub(crate) const BACK_TO_BOTTOM: &str = " \u{2193} Back to bottom \u{b7} ctrl+end ";
/// Its wording when rows landed below the reader while they were scrolled up.
const BACK_TO_BOTTOM_UNSEEN: &str = " \u{2193} New output \u{b7} ctrl+end ";

/// Render the chat area with all messages, handling scrolling.
pub fn render_chat_area(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let border_color = match state.focus {
        Focus::Chat => theme().border_focused,
        _ => theme().border,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(" Chat ");

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Rebuilt from scratch every frame, because the transcript reflows every
    // frame — see `regions.rs` for why a maintained table would be the copy
    // that lies.
    state.regions.clear();

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let content_width = inner.width;
    let visible_height = inner.height as usize;

    // Render only the visible window: pass 1 ensures every message's lines
    // exist (settled ones in the per-message cache, the streaming one via
    // its incremental prefix cache) and records line counts; pass 2 clones
    // out just the rows the window intersects. The previous implementation
    // assembled the FULL transcript (`entry.lines.clone()` per message,
    // then `visible_lines.to_vec()` on top) on every frame — O(transcript)
    // deep copies per draw, 20x/s while a spinner runs.
    let modality = state.modality();
    let verbose = state.verbose;
    let scroll_offset = state.scroll_offset;
    let (total_lines, visible, hits) = build_visible_lines(
        &state.messages,
        verbose,
        now_ms(),
        content_width,
        &mut state.chat_line_cache,
        modality,
        scroll_offset,
        visible_height,
    );
    // Write the offset back clamped to what the transcript can actually
    // reach. `visible_window` already clamps for its own arithmetic, so the
    // frame would look right either way; what this buys is the NEXT
    // keystroke. `Home` maps to `usize::MAX / 2`, and without this an
    // unreachable offset makes every following `scroll_down` a no-op the
    // reader sees nothing change from, and gives the scrollbar a position
    // nobody is at.
    //
    // Once, here, from the heights this frame measured. A pre-pass clamp
    // against the cache's last-known heights was written first and deleted:
    // a mutation run left it green with the guard removed, which is the
    // tell for a second derivation that can never be the one that fires
    // (判据 §2, §12).
    state.scroll_offset = state
        .scroll_offset
        .min(total_lines.saturating_sub(visible_height));

    // One row per line, no wrapping. `build_visible_lines` counts logical
    // lines and sized this window in them, so a line that wrapped to two rows
    // would push the newest content off the bottom and shift every click
    // target below it by one.
    let rows: Vec<Line<'static>> = visible
        .into_iter()
        .map(|line| clip_to_width(line, content_width))
        .collect();
    frame.render_widget(Paragraph::new(rows), inner);

    for hit in hits {
        let Ok(dy) = u16::try_from(hit.out_row) else {
            continue;
        };
        if dy >= inner.height {
            continue;
        }
        state.regions.push(
            Rect {
                x: inner.x,
                y: inner.y + dy,
                width: inner.width,
                height: 1,
            },
            RegionKind::ToggleRow,
            hit.row_id,
        );
    }

    render_scrollbar(
        frame,
        area,
        total_lines,
        visible_height,
        state.scroll_offset,
    );
    render_back_to_bottom(frame, state, inner);
}

/// Truncate a line to `width` columns, marking the cut with `…`.
///
/// # Why every row is clipped, rather than each producer wrapping
///
/// This widget counts *logical* lines to size its scroll window and paints
/// one row per line. A line wider than the pane would wrap to two rows, and
/// then the count and the paint disagree: the transcript's newest row falls
/// off the bottom, and every click target below the wrap sits one row from
/// where it was drawn.
///
/// Most producers already wrap to width on their own — markdown,
/// `render_system_message`, `render_reasoning`. Three do not, for reasons
/// that are theirs rather than oversights: a tool header is one unbreakable
/// `Bash(cargo test …)`, a diff row would have to re-emit its gutter on a
/// continuation to stay readable, and a turn summary is a single sentence
/// that simply gets long. Rather than ask each of them to remember, the
/// invariant is enforced once, here, at the only place that knows both the
/// line and the pane it is about to be painted into (判据 §12). For the
/// producers that already fit, this is a measurement and nothing else.
fn clip_to_width(line: Line<'static>, width: u16) -> Line<'static> {
    let budget = width as usize;
    if budget == 0 {
        return Line::default();
    }
    let total: usize = line
        .spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if total <= budget {
        return line;
    }
    // One column goes to the ellipsis that says a cut happened: a reader who
    // cannot see the cut reads a truncated path as the whole path.
    let keep = budget - 1;
    let mut used = 0usize;
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
    for span in line.spans {
        let w = UnicodeWidthStr::width(span.content.as_ref());
        if used + w <= keep {
            used += w;
            spans.push(span);
            continue;
        }
        // Split inside this span, by accumulated COLUMNS and on a char
        // boundary — a byte split would panic on the first CJK row, and a
        // char count would under-fill a line of them by half.
        let mut text = String::new();
        for ch in span.content.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + cw > keep {
                break;
            }
            used += cw;
            text.push(ch);
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
        }
        break;
    }
    spans.push(Span::styled(
        "\u{2026}".to_string(),
        Style::default().fg(theme().muted),
    ));
    Line::from(spans)
}

/// The vertical scrollbar, on the block's right border, only when there is
/// something to scroll.
fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    total_lines: usize,
    visible_height: usize,
    scroll_offset: usize,
) {
    if total_lines <= visible_height {
        return;
    }
    // `position` counts from the TOP, `scroll_offset` from the bottom.
    let max_offset = total_lines - visible_height;
    let position = max_offset - scroll_offset.min(max_offset);
    let mut scroll_state = ScrollbarState::new(max_offset).position(position);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .style(Style::default().fg(theme().border)),
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut scroll_state,
    );
}

/// The docked `[ ↓ Back to bottom ]` control, shown only while the reader is
/// somewhere else.
///
/// Painted over the transcript's last row rather than in a reserved row of
/// its own: a row reserved for it would change the transcript's height every
/// time the user scrolled, reflowing the thing they are trying to read.
fn render_back_to_bottom(frame: &mut Frame, state: &mut AppState, inner: Rect) {
    if state.scroll_offset == 0 {
        return;
    }
    let label = if state.unseen_below {
        BACK_TO_BOTTOM_UNSEEN
    } else {
        BACK_TO_BOTTOM
    };
    let Ok(label_width) = u16::try_from(UnicodeWidthStr::width(label)) else {
        return;
    };
    if label_width > inner.width || inner.height == 0 {
        return;
    }
    let rect = Rect {
        x: inner.x + inner.width - label_width,
        y: inner.y + inner.height - 1,
        width: label_width,
        height: 1,
    };
    let style = if state.unseen_below {
        Style::default()
            .fg(theme().primary)
            .bg(theme().status_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme().status_fg).bg(theme().status_bg)
    };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(label, style))), rect);
    state
        .regions
        .push(rect, RegionKind::BackToBottom, String::new());
}

/// Build all rendered lines from the message history.
///
/// Uncached: `render_chat_area` uses [`build_visible_lines`] instead. This
/// is kept as the reference implementation the cache/windowing is checked
/// against (see `build_all_lines_cached_matches_uncached_output`) and for
/// tests that don't care about caching — hence `#[cfg(test)]` rather than
/// dead-code warnings.
#[cfg(test)]
fn build_all_lines(state: &AppState, width: u16) -> Vec<Line<'static>> {
    build_all_lines_at(state, width, 0)
}

#[cfg(test)]
fn build_all_lines_at(state: &AppState, width: u16, now_ms: u64) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for message in &state.messages {
        render_settled_message(
            message,
            state.verbose,
            now_ms,
            width,
            KEY_MODALITY,
            &mut lines,
        );
        // The streaming cursor is the one thing the settled path does not
        // draw, because the cached path never renders a streaming entry
        // through it.
        if matches!(
            message,
            TranscriptEntry::AssistantText {
                streaming: true,
                ..
            }
        ) {
            lines.push(streaming_cursor_line());
        }
        // Add a blank line between messages
        lines.push(Line::default());
    }
    lines
}

/// Cached full-transcript build for tests: a window tall enough to cover
/// everything (`usize::MAX`) makes [`build_visible_lines`] return the whole
/// transcript, exercising the same cache machinery the production path uses.
#[cfg(test)]
fn build_all_lines_cached(
    messages: &[TranscriptEntry],
    verbose: bool,
    now_ms: u64,
    width: u16,
    cache: &mut LineCache,
) -> Vec<Line<'static>> {
    let (_total, lines, _) = build_visible_lines(
        messages,
        verbose,
        now_ms,
        width,
        cache,
        KEY_MODALITY,
        0,
        usize::MAX,
    );
    lines
}

/// Render only the lines inside the current scroll window.
///
/// Returns `(total_lines, visible_lines)`. Pass 1 materializes every
/// message's rendered lines exactly once per change (settled messages hit
/// the per-message [`LineCache`] entry, the streaming message reuses its
/// `Rc`-shared frozen prefix and re-renders only the unfrozen tail) and
/// records per-message heights; pass 2 walks the cumulative offsets and
/// clones out ONLY the rows the window intersects — per-frame cost is
/// O(messages) pointer arithmetic plus O(visible rows) of cloning, never
/// O(transcript).
///
/// Each message occupies `rendered_lines + 1` rows: the blank separator the
/// old `build_all_lines` pushed after every message is folded into the
/// height so window arithmetic stays exact.
#[allow(clippy::too_many_arguments)]
fn build_visible_lines(
    messages: &[TranscriptEntry],
    verbose: bool,
    now_ms: u64,
    width: u16,
    cache: &mut LineCache,
    modality: Modality,
    scroll_offset: usize,
    visible_height: usize,
) -> (usize, Vec<Line<'static>>, Vec<VisibleToggle>) {
    let streaming_idx = messages.iter().position(|m| {
        matches!(
            m,
            TranscriptEntry::AssistantText {
                streaming: true,
                ..
            }
        )
    });
    if cache.streaming_message_idx != streaming_idx {
        cache.streaming_markdown_cache = None;
        cache.streaming_message_idx = streaming_idx;
    }

    // Pass 1: ensure lines exist, record heights (rendered lines + 1 blank
    // separator per message).
    let mut heights: Vec<usize> = Vec::with_capacity(messages.len());
    let mut streaming_content: Option<StreamLines> = None;
    for (idx, message) in messages.iter().enumerate() {
        if Some(idx) == streaming_idx {
            // The streaming entry's text grows every tick, so it is never in
            // `entries`; its markdown still gets the incremental prefix cache.
            // It no longer carries a "head" — reasoning and tool rows are
            // their own entries, each cached or re-rendered on its own terms.
            cache.entries.remove(&idx);
            if let TranscriptEntry::AssistantText { markdown, .. } = message {
                if !markdown.is_empty() {
                    streaming_content = Some(markdown_to_lines_incremental(
                        markdown,
                        width.saturating_sub(2),
                        &mut cache.streaming_markdown_cache,
                    ));
                }
                let content_rows = streaming_content
                    .as_ref()
                    .map_or(0, StreamLines::line_count);
                // +1 streaming cursor, +1 blank separator.
                heights.push(content_rows + 2);
            }
        } else {
            // A running row's glyph is a function of the clock, so its cached
            // lines are stale the moment they are stored — drop the entry and
            // let the miss below re-render it. Settled rows cache normally,
            // which is what keeps a long tool-heavy transcript cheap.
            if matches!(
                message,
                TranscriptEntry::Tool(row) if matches!(row.status, RowStatus::Running { .. })
            ) {
                cache.entries.remove(&idx);
            }
            let (kind, fingerprint) = message_kind_and_fingerprint(message);
            let generation = crate::tui::highlight::generation();
            let hit = cache.entries.get(&idx).filter(|e| {
                e.kind == kind
                    && e.fingerprint == fingerprint
                    && e.width == width
                    && e.highlight_generation == generation
            });
            match hit {
                Some(entry) => heights.push(entry.lines.len() + 1),
                None => {
                    let mut buf = Vec::new();
                    let toggles =
                        render_settled_message(message, verbose, now_ms, width, modality, &mut buf);
                    heights.push(buf.len() + 1);
                    cache.entries.insert(
                        idx,
                        CachedEntry {
                            kind,
                            fingerprint,
                            width,
                            highlight_generation: generation,
                            lines: buf,
                            toggles,
                        },
                    );
                }
            }
        }
    }
    // Drop cache entries for indices beyond the current message count (a
    // conversation switch or `.clear()` shrinks the vec).
    cache.entries.retain(|idx, _| *idx < messages.len());

    let total_lines: usize = heights.iter().sum();
    let (start, end) = visible_window(total_lines, visible_height, scroll_offset);

    // Pass 2: clone out only the intersecting rows.
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut hits: Vec<VisibleToggle> = Vec::new();
    let mut pos = 0usize;
    for (idx, message) in messages.iter().enumerate() {
        let height = heights[idx];
        let mstart = pos;
        pos += height;
        if pos <= start || mstart >= end {
            continue;
        }
        let lo = start.saturating_sub(mstart);
        let hi = (end - mstart).min(height);
        // Body rows occupy [0, height-1); the blank separator sits at
        // height-1.
        let body_hi = hi.min(height - 1);
        if lo < body_hi {
            if Some(idx) == streaming_idx {
                copy_streaming_slice(streaming_content.as_ref(), lo, body_hi, &mut out);
            } else if let Some(entry) = cache.entries.get(&idx) {
                out.extend(entry.lines[lo..body_hi].iter().cloned());
                // Rows [lo, body_hi) of this message land at out-indices
                // `mstart + i - start`; a toggle outside that slice is
                // scrolled off and has no cell to be clicked in.
                if let TranscriptEntry::Tool(row) = message {
                    for &t in &entry.toggles {
                        if t >= lo && t < body_hi {
                            hits.push(VisibleToggle {
                                out_row: mstart + t - start,
                                row_id: row.id.clone(),
                            });
                        }
                    }
                }
            }
        }
        if hi == height {
            out.push(Line::default());
        }
    }
    (total_lines, out, hits)
}

/// The `[start, end)` row range the viewport shows.
///
/// One branch, not two: the old `auto_scroll == true` branch computed exactly
/// `(total - height, total)`, which is what the offset branch already yields
/// at `scroll_offset == 0` — the bool was a weakened second spelling of
/// "parked at the bottom" (判据 §1), and it is gone.
fn visible_window(
    total_lines: usize,
    visible_height: usize,
    scroll_offset: usize,
) -> (usize, usize) {
    // Clamp a large offset (a held PageUp, or a stale offset after the
    // transcript shrank) so it can never push the whole window off-screen and
    // blank the chat.
    let max_offset = total_lines.saturating_sub(visible_height);
    let offset = scroll_offset.min(max_offset);
    let end = total_lines.saturating_sub(offset);
    (end.saturating_sub(visible_height), end)
}

/// Copy the `[lo, hi)` slice of the streaming entry's rows into `out`,
/// applying the assistant prefix bar to content rows (deferred to here so
/// off-window rows never pay for it). Row layout: content | cursor.
///
/// There is no "head" any more: reasoning and tool rows used to be rendered
/// above the streaming message because they lived inside it, and they are
/// separate entries now — each windowed, cached and invalidated on its own.
fn copy_streaming_slice(
    content: Option<&StreamLines>,
    lo: usize,
    hi: usize,
    out: &mut Vec<Line<'static>>,
) {
    let content_len = content.map_or(0, StreamLines::line_count);

    let c_lo = lo.min(content_len);
    let c_hi = hi.min(content_len);
    if let Some(sl) = content {
        let prefix_style = Style::default().fg(theme().assistant);
        for i in c_lo..c_hi {
            if let Some(line) = sl.get(i) {
                let mut spans = Vec::with_capacity(line.spans.len() + 1);
                spans.push(Span::styled("\u{2503} ", prefix_style));
                spans.extend(line.spans.iter().cloned());
                out.push(Line::from(spans));
            }
        }
    }

    if lo <= content_len && content_len < hi {
        out.push(streaming_cursor_line());
    }
}

/// Wall clock in unix ms — the unit the shared spinner and duration helpers
/// take. A pre-1970 clock yields 0 rather than a panic.
fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// Render a settled (non-streaming) entry into `out`.
///
/// Tool rows are NOT prefixed with the assistant `┃ ` bar: they are peers of
/// the text now, not decoration inside a message, and indenting them under a
/// bar would put back the visual nesting the model change removed.
fn render_settled_message(
    message: &TranscriptEntry,
    verbose: bool,
    now_ms: u64,
    width: u16,
    modality: Modality,
    out: &mut Vec<Line<'static>>,
) -> Vec<usize> {
    let base = out.len();
    match message {
        TranscriptEntry::UserText { text, at_ms, .. } => {
            render_user_message(text, *at_ms, width, out);
        }
        TranscriptEntry::AssistantText { markdown, .. } => {
            if !markdown.is_empty() {
                let md_lines = markdown_to_lines(markdown, width.saturating_sub(2));
                push_prefixed_content(out, md_lines);
            }
        }
        TranscriptEntry::Reasoning { text, .. } => {
            if verbose {
                render_reasoning(text, width, out);
            }
        }
        TranscriptEntry::Tool(row) => {
            let rendered = render_tool_row(row, now_ms, width, modality);
            out.extend(rendered.lines);
            return rendered.toggles.into_iter().map(|t| base + t).collect();
        }
        TranscriptEntry::ToolGroup(group) => {
            out.extend(render_tool_group(group, now_ms, width, modality));
        }
        TranscriptEntry::TurnSummary(summary) => {
            out.push(Line::from(Span::styled(
                turn_summary_text(summary),
                Style::default().fg(theme().muted),
            )));
        }
        TranscriptEntry::SystemNotice { text, .. } => {
            render_system_message(text, width, out);
        }
    }
    Vec::new()
}

/// Render a user message with blue prefix bar.
fn render_user_message(
    content: &str,
    at_ms: Option<u64>,
    width: u16,
    lines: &mut Vec<Line<'static>>,
) {
    let prefix_style = Style::default().fg(theme().user);
    // No clock for a message this client was never told a time for. Falling
    // back to `now` would date a restored message to the moment it was
    // restored, which is a fact the header would be stating and getting wrong.
    let time_str = at_ms
        .and_then(|ms| i64::try_from(ms).ok())
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|t| format!("  {}", t.format("%H:%M")))
        .unwrap_or_default();

    // Header: ┃ You  12:34
    lines.push(Line::from(vec![
        Span::styled("\u{2503} ", prefix_style),
        Span::styled(
            "You".to_string(),
            Style::default()
                .fg(theme().user)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(time_str, Style::default().fg(theme().muted)),
    ]));

    // Content lines with prefix
    let content_width = width.saturating_sub(2); // account for "┃ " prefix
    let md_lines = markdown_to_lines(content, content_width);
    for md_line in md_lines {
        let mut spans = vec![Span::styled("\u{2503} ", prefix_style)];
        spans.extend(md_line.spans);
        lines.push(Line::from(spans));
    }
}

/// A reasoning entry, shown only under `/verbose`.
///
/// Its own entry now rather than a field on the assistant message, so it
/// renders where it happened instead of always above the whole turn.
fn render_reasoning(text: &str, width: u16, lines: &mut Vec<Line<'static>>) {
    let prefix_style = Style::default().fg(theme().assistant);
    let reasoning_style = Style::default().fg(theme().reasoning);
    let reasoning_prefix = Style::default().fg(theme().muted);
    let content_width = width.saturating_sub(4); // account for "┃ ┊ " prefix

    for reason_line in text.lines() {
        if reason_line.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("\u{2503} ", prefix_style),
                Span::styled("\u{250a} ", reasoning_prefix),
            ]));
            continue;
        }
        for wrapped_line in textwrap::wrap(reason_line, content_width as usize) {
            lines.push(Line::from(vec![
                Span::styled("\u{2503} ", prefix_style),
                Span::styled("\u{250a} ", reasoning_prefix),
                Span::styled(wrapped_line.into_owned(), reasoning_style),
            ]));
        }
    }
}

/// Append markdown-rendered content lines, each prefixed with the assistant
/// `┃ ` bar.
fn push_prefixed_content(lines: &mut Vec<Line<'static>>, md_lines: Vec<Line<'static>>) {
    let prefix_style = Style::default().fg(theme().assistant);
    for md_line in md_lines {
        let mut spans = vec![Span::styled("\u{2503} ", prefix_style)];
        spans.extend(md_line.spans);
        lines.push(Line::from(spans));
    }
}

/// The `┃ ▍` line shown under a still-streaming assistant message.
fn streaming_cursor_line() -> Line<'static> {
    let prefix_style = Style::default().fg(theme().assistant);
    Line::from(vec![
        Span::styled("\u{2503} ", prefix_style),
        Span::styled(
            "\u{258d}".to_string(), // ▍
            Style::default().fg(theme().assistant),
        ),
    ])
}

/// Render a system message with yellow text and indentation.
///
/// System content is frequently multi-line (e.g. `/help`, `/usage`, `/replay`
/// output joined with `\n`) and individual lines may be wider than the pane.
/// ratatui does not treat an embedded `\n` inside a `Span` as a row break, and
/// the chat scroll window counts *logical* `Line`s — so emitting the whole
/// message as one `Span` both mis-renders it and desyncs the scroll height from
/// the physical rows, which clips the newest content off-screen. Split on `\n`
/// and wrap each physical line to the content width so every emitted `Line` is
/// `<= width` and the logical-line window matches the rendered rows.
fn render_system_message(content: &str, width: u16, lines: &mut Vec<Line<'static>>) {
    let style = Style::default().fg(theme().system);
    let content_width = (width.saturating_sub(2)).max(1) as usize; // account for "  " indent
    for raw_line in content.split('\n') {
        if raw_line.is_empty() {
            lines.push(Line::from(vec![Span::styled("  ", style)]));
            continue;
        }
        for wrapped in textwrap::wrap(raw_line, content_width) {
            lines.push(Line::from(vec![
                Span::styled("  ", style),
                Span::styled(wrapped.into_owned(), style),
            ]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_lines_with_system_message() {
        let state = AppState::new("test".into(), "claude".into());
        let lines = build_all_lines(&state, 80);
        // Should have at least the welcome system message + blank line
        assert!(lines.len() >= 2);
    }

    #[test]
    fn build_lines_with_user_and_assistant() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_user_message("Hello".into());
        state.ensure_assistant_message();
        if let TranscriptEntry::AssistantText {
            markdown: content, ..
        } = state.current_assistant_mut()
        {
            content.push_str("Hi there!");
        }

        let lines = build_all_lines(&state, 80);
        assert!(lines.len() >= 6);

        let has_you = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("You"))
        });
        assert!(has_you, "Should contain 'You' header");

        let has_reply = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("Hi there!"))
        });
        assert!(has_reply, "the assistant's text must be rendered");
    }

    /// The whole point of the model change: a tool row sits where it ran.
    ///
    /// # When this goes red
    ///
    /// It could not even be written before — tools lived inside
    /// `ChatMessage::Assistant`, so every one of a turn's calls rendered above
    /// all of that turn's text and this ordering was unrepresentable. Put the
    /// rows back under the message and the second text lands above the tool.
    #[test]
    fn a_tool_row_renders_between_the_texts_that_surround_it() {
        use serde_json::json;
        let mut state = AppState::new("test".into(), "claude".into());
        state.messages.clear();

        state.append_assistant_content("before the call");
        state.start_tool_execution("c1".into(), "file_read".into(), &json!({"path": "a.rs"}));
        state.append_assistant_content("after the call");

        let rendered: Vec<String> = build_all_lines(&state, 80)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let idx = |needle: &str| {
            rendered
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle} not rendered: {rendered:?}"))
        };
        assert!(
            idx("before the call") < idx("Read(a.rs)"),
            "text that preceded the call must render above it: {rendered:?}"
        );
        assert!(
            idx("Read(a.rs)") < idx("after the call"),
            "text that followed the call must render below it: {rendered:?}"
        );
    }

    #[test]
    fn streaming_message_shows_cursor() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.ensure_assistant_message();

        let lines = build_all_lines(&state, 80);
        // Should contain the streaming cursor character ▍
        let has_cursor = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains('\u{258d}'))
        });
        assert!(has_cursor, "Streaming message should show cursor");
    }

    #[test]
    fn non_streaming_message_no_cursor() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.ensure_assistant_message();
        if let TranscriptEntry::AssistantText {
            streaming: is_streaming,
            ..
        } = state.current_assistant_mut()
        {
            *is_streaming = false;
        }

        let lines = build_all_lines(&state, 80);
        let has_cursor = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains('\u{258d}'))
        });
        assert!(!has_cursor, "Non-streaming message should not show cursor");
    }

    #[test]
    fn zero_width_area_does_not_panic() {
        let state = AppState::new("test".into(), "claude".into());
        let lines = build_all_lines(&state, 0);
        // Should not panic, may produce empty or minimal output
        let _ = lines;
    }

    #[test]
    fn system_message_splits_and_wraps() {
        let mut state = AppState::new("test".into(), "claude".into());
        // Multi-line content with a line wider than the pane.
        let long = "x".repeat(60);
        state.add_system_message(format!("line one\n{long}"));

        let mut lines = Vec::new();
        // Render just the system messages via build_all_lines at a narrow width.
        let all = build_all_lines(&state, 20);
        lines.extend(all);

        // No rendered Line may contain an embedded newline (would mis-render and
        // desync the scroll height).
        for line in &lines {
            for span in &line.spans {
                assert!(
                    !span.content.as_ref().contains('\n'),
                    "system message span must not contain an embedded newline"
                );
            }
        }
        // The 60-char line at width 20 must have wrapped to multiple rows.
        let x_rows = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.content.as_ref().contains('x')))
            .count();
        assert!(x_rows > 1, "long system line should wrap to > 1 row");
    }

    #[test]
    fn reasoning_shown_only_in_verbose() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.messages.push(TranscriptEntry::Reasoning {
            id: "r1".into(),
            text: "thinking...".into(),
            collapsed: true,
        });

        // Non-verbose: reasoning should not appear
        let lines = build_all_lines(&state, 80);
        let has_thinking = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("thinking"))
        });
        assert!(
            !has_thinking,
            "Reasoning should not show in non-verbose mode"
        );

        // Verbose: reasoning should appear
        state.verbose = true;
        let lines = build_all_lines(&state, 80);
        let has_thinking = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("thinking"))
        });
        assert!(has_thinking, "Reasoning should show in verbose mode");
    }

    #[test]
    fn build_all_lines_reuses_cached_lines_for_unchanged_messages() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_user_message("Hello".into());
        state.ensure_assistant_message();
        if let TranscriptEntry::AssistantText {
            markdown,
            streaming,
            ..
        } = state.current_assistant_mut()
        {
            markdown.push_str("Hi there!");
            *streaming = false;
        }

        let mut cache = LineCache::default();
        let first = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        let second = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        assert_eq!(first, second);
        // Cache must actually have been populated, not silently bypassed.
        assert!(!cache.entries.is_empty());
    }

    #[test]
    fn build_all_lines_invalidates_on_content_change() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.ensure_assistant_message();
        if let TranscriptEntry::AssistantText {
            streaming: is_streaming,
            ..
        } = state.current_assistant_mut()
        {
            // Must be non-streaming: `build_all_lines_cached` never caches a
            // streaming message (its spinner/tool content can change every
            // tick without `content_len` changing), so a still-streaming
            // message here would make the first call below a no-op cache
            // insert and the test would pass regardless of whether
            // `content_len` invalidation actually works.
            *is_streaming = false;
        }
        let mut cache = LineCache::default();
        let _first = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        assert!(
            !cache.entries.is_empty(),
            "the message must actually be cached before we test invalidation"
        );
        if let TranscriptEntry::AssistantText {
            markdown: content, ..
        } = state.current_assistant_mut()
        {
            content.push_str("new text");
        }
        let updated = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        let has_new_text = updated.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("new text"))
        });
        assert!(
            has_new_text,
            "changed content must not serve a stale cache entry"
        );
    }

    #[test]
    fn build_all_lines_invalidates_on_width_change() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_system_message("x".repeat(60));
        let mut cache = LineCache::default();
        let wide = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        let narrow = build_all_lines_cached(&state.messages, state.verbose, 0, 20, &mut cache);
        assert_ne!(
            wide.len(),
            narrow.len(),
            "resize must reformat, not reuse the wide cache"
        );
    }

    #[test]
    fn build_all_lines_cached_matches_uncached_output() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_user_message("Hello".into());
        state.ensure_assistant_message();
        if let TranscriptEntry::AssistantText {
            markdown: content, ..
        } = state.current_assistant_mut()
        {
            content.push_str("Hi there!");
        }
        let mut cache = LineCache::default();
        let cached = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        let uncached = build_all_lines(&state, 80);
        assert_eq!(cached, uncached, "caching must not change what's rendered");
    }

    #[test]
    fn build_visible_lines_window_matches_the_full_build_sliced() {
        // The windowed production path must be byte-identical to slicing the
        // full reference build — for both scroll modes and for a window that
        // cuts through the middle of a message (the blank-separator and
        // streaming-cursor boundary cases live in that cut).
        let mut state = AppState::new("test".into(), "claude".into());
        for i in 0..6 {
            state.add_user_message(format!("question {i}"));
            state.ensure_assistant_message();
            if let TranscriptEntry::AssistantText {
                markdown,
                streaming,
                ..
            } = state.current_assistant_mut()
            {
                markdown.push_str(&format!("answer {i}\nwith a second line"));
                *streaming = i == 5; // only the last one stays streaming
            }
        }
        let full = build_all_lines(&state, 80);
        let mut cache = LineCache::default();

        // Bottom window (offset 0).
        let height = 7;
        let (total, visible, _) = build_visible_lines(
            &state.messages,
            state.verbose,
            0,
            80,
            &mut cache,
            KEY_MODALITY,
            0,
            height,
        );
        assert_eq!(total, full.len());
        assert_eq!(visible, full[full.len() - height..].to_vec());

        // Scrolled-up window (offset from the bottom).
        let (_total, visible, _) = build_visible_lines(
            &state.messages,
            state.verbose,
            0,
            80,
            &mut cache,
            KEY_MODALITY,
            10,
            height,
        );
        let end = full.len() - 10;
        assert_eq!(visible, full[end - height..end].to_vec());

        // A second call with unchanged state must serve the same window from
        // cache (this is the per-frame steady state).
        let (_total, visible2, _) = build_visible_lines(
            &state.messages,
            state.verbose,
            0,
            80,
            &mut cache,
            KEY_MODALITY,
            10,
            height,
        );
        assert_eq!(visible, visible2);
    }
}

#[cfg(test)]
mod click_tests {
    use super::*;
    use crate::tui::regions::RegionKind;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;
    use shared_ui_logic::transcript::ToolRow;

    const W: u16 = 60;
    const H: u16 = 24;

    /// A state holding one finished shell call with far more output than a
    /// folded row shows — i.e. a row that has a hint.
    fn state_with_a_folded_call() -> AppState {
        let mut state = AppState::new("s".into(), "m".into());
        state.messages.clear();
        let mut row = ToolRow::new("c1", "bash", &json!({ "command": "ls" }));
        row.start(0);
        row.finish(
            &aleph_protocol::ToolResult::success("line of output\n".repeat(40)),
            10,
            10,
        );
        state.messages.push(TranscriptEntry::Tool(row));
        state
    }

    fn draw(state: &mut AppState) -> Vec<String> {
        let backend = TestBackend::new(W, H);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_chat_area(f, state, f.area())).unwrap();
        let buf = term.backend().buffer().clone();
        (0..H)
            .map(|y| {
                (0..W)
                    .map(|x| buf.cell((x, y)).map_or(" ", |c| c.symbol()).to_string())
                    .collect()
            })
            .collect()
    }

    fn row_containing(rows: &[String], needle: &str) -> u16 {
        let idx = rows
            .iter()
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} never painted:\n{}", rows.join("\n")));
        u16::try_from(idx).expect("a screen row fits u16")
    }

    /// The wire: the row a hint is *painted* on is the row a click on it is
    /// *hit-tested* to.
    ///
    /// # When this goes red
    ///
    /// Any drift between the line builder's offsets and the painter — a
    /// re-introduced `Wrap`, an off-by-one in the window→screen translation,
    /// a cache hit that serves lines without their toggles. Each of those is
    /// invisible to a test that only checks the region table's own arithmetic,
    /// because both halves would agree with each other and disagree with the
    /// screen (判据 §10).
    #[test]
    fn the_hint_a_frame_painted_is_the_row_a_click_lands_on() {
        let mut state = state_with_a_folded_call();
        state.mouse = true;
        let rows = draw(&mut state);

        let hint_y = row_containing(&rows, "to expand");
        let hit = state
            .regions
            .hit(6, hint_y)
            .unwrap_or_else(|| panic!("no region on the hint row:\n{}", rows.join("\n")));
        assert_eq!(hit.kind, RegionKind::ToggleRow);
        assert_eq!(hit.row_id, "c1");

        // And the header, which is what makes folding again reachable.
        let header_y = row_containing(&rows, "Bash(ls)");
        let hit = state.regions.hit(2, header_y).expect("no region on header");
        assert_eq!(hit.row_id, "c1");
    }

    /// Clicking that hint really unfolds the row, and the gesture that undoes
    /// it is reachable from where the unfolding left the reader.
    #[test]
    fn clicking_the_hint_unfolds_and_the_closing_hint_folds_again() {
        let mut state = state_with_a_folded_call();
        state.mouse = true;
        let rows = draw(&mut state);
        let hint_y = row_containing(&rows, "to expand");
        let id = state.regions.hit(6, hint_y).unwrap().row_id.clone();

        state.toggle_tool_expanded(&id);
        let rows = draw(&mut state);
        assert!(
            !rows.iter().any(|r| r.contains("to expand")),
            "still folded:\n{}",
            rows.join("\n")
        );

        // The way back is on screen without scrolling — the header is off the
        // top by now, which is the whole reason the closing hint exists.
        assert!(
            !rows.iter().any(|r| r.contains("Bash(ls)")),
            "this fixture is supposed to overflow the viewport:\n{}",
            rows.join("\n")
        );
        let collapse_y = row_containing(&rows, "to collapse");
        let id = state
            .regions
            .hit(6, collapse_y)
            .expect("no target on the collapse row")
            .row_id
            .clone();
        state.toggle_tool_expanded(&id);
        let rows = draw(&mut state);
        assert!(
            rows.iter().any(|r| r.contains("to expand")),
            "never refolded"
        );
    }

    /// The guard the plan names: with capture off, the hint has to say
    /// `ctrl+o`, because nothing is listening for a click.
    ///
    /// This is the fail-closed branch of spec §8, otherwise reachable only on
    /// a terminal nobody tests on.
    #[test]
    fn with_mouse_capture_off_the_hint_names_the_key() {
        let mut off = state_with_a_folded_call();
        off.mouse = false;
        let rows = draw(&mut off);
        let hint = rows[row_containing(&rows, "to expand") as usize].clone();
        assert!(hint.contains("ctrl+o to expand"), "{hint:?}");

        let mut on = state_with_a_folded_call();
        on.mouse = true;
        let rows = draw(&mut on);
        let hint = rows[row_containing(&rows, "to expand") as usize].clone();
        assert!(hint.contains("click to expand"), "{hint:?}");
    }

    /// Ctrl+O reaches every folded row, not just one — which is what makes
    /// the hint on *every* row true.
    #[test]
    fn the_expand_all_toggle_unfolds_then_refolds_every_row() {
        let mut state = state_with_a_folded_call();
        let mut second = ToolRow::new("c2", "bash", &json!({ "command": "pwd" }));
        second.start(0);
        second.finish(
            &aleph_protocol::ToolResult::success("more\n".repeat(40)),
            10,
            10,
        );
        state.messages.push(TranscriptEntry::Tool(second));

        let hints = |s: &mut AppState| draw(s).iter().filter(|r| r.contains("to expand")).count();
        assert_eq!(hints(&mut state), 2);
        state.toggle_expand_all();
        assert_eq!(hints(&mut state), 0);
        state.toggle_expand_all();
        assert_eq!(hints(&mut state), 2);
    }

    /// The docked control appears only while the reader is somewhere else,
    /// and it wins the transcript row it covers.
    #[test]
    fn the_back_to_bottom_control_is_docked_only_while_scrolled_up() {
        let mut state = state_with_a_folded_call();
        // Enough rows that there is something to scroll.
        for i in 0..30 {
            state.add_system_message(format!("row {i}"));
        }
        let rows = draw(&mut state);
        assert!(
            !rows.iter().any(|r| r.contains("Back to bottom")),
            "docked while already at the bottom:\n{}",
            rows.join("\n")
        );

        state.scroll_up(5);
        let rows = draw(&mut state);
        let y = row_containing(&rows, "Back to bottom");
        let hit = state
            .regions
            .hit(W - 3, y)
            .expect("the docked control has no click target");
        assert_eq!(hit.kind, RegionKind::BackToBottom);
    }

    /// Rows that landed below the reader change its wording — otherwise
    /// `unseen_below` is a field nothing renders (判据 §17).
    #[test]
    fn unseen_rows_change_the_docked_wording() {
        let mut state = state_with_a_folded_call();
        for i in 0..30 {
            state.add_system_message(format!("row {i}"));
        }
        state.scroll_up(5);
        state.unseen_below = true;
        let rows = draw(&mut state);
        assert!(
            rows.iter().any(|r| r.contains("New output")),
            "unseen rows read the same as none:\n{}",
            rows.join("\n")
        );
    }

    /// A wild offset (`Home` maps to `usize::MAX / 2`) is clamped to
    /// something the transcript can actually reach, so the next `scroll_down`
    /// moves the screen instead of spending 10^18 keypresses.
    #[test]
    fn an_unreachable_scroll_offset_is_clamped_by_the_frame_that_paints_it() {
        let mut state = state_with_a_folded_call();
        for i in 0..30 {
            state.add_system_message(format!("row {i}"));
        }
        draw(&mut state);
        state.scroll_up(usize::MAX / 2);
        draw(&mut state);
        assert!(
            state.scroll_offset < 1_000,
            "offset left at {}",
            state.scroll_offset
        );

        let before = state.scroll_offset;
        state.scroll_down(1);
        draw(&mut state);
        assert_eq!(state.scroll_offset, before - 1);
    }

    /// The table describes the LAST painted frame and nothing else: a frame
    /// with no transcript leaves no stale targets behind.
    #[test]
    fn a_repaint_replaces_the_targets_rather_than_adding_to_them() {
        let mut state = state_with_a_folded_call();
        state.mouse = true;
        let rows = draw(&mut state);
        let hint_y = row_containing(&rows, "to expand");
        assert!(state.regions.hit(6, hint_y).is_some());

        state.messages.clear();
        draw(&mut state);
        assert!(
            state.regions.hit(6, hint_y).is_none(),
            "a target survived the row it described"
        );
    }
}

#[cfg(test)]
mod width_tests {
    use super::*;
    use aleph_protocol::file_change::{FileChange, FileChangeKind, Hunk, HunkLine, LineTag};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;
    use shared_ui_logic::transcript::{ToolRow, TurnSummaryEntry};

    /// Every producer this widget paints, in one transcript, each with content
    /// wider than any reasonable pane.
    ///
    /// Named rather than sampled: the list is the set of `TranscriptEntry`
    /// variants `render_settled_message` matches on, so a variant added
    /// without a wrap or a clip is a variant missing from here — and the
    /// `match` it is added to is the reminder.
    fn a_transcript_of_over_wide_rows() -> AppState {
        let mut state = AppState::new("s".into(), "m".into());
        state.messages.clear();
        state.verbose = true;
        let long = "verylongunbreakabletoken".repeat(20);

        state.add_user_message(long.clone());
        state.messages.push(TranscriptEntry::AssistantText {
            id: "a1".into(),
            markdown: long.clone(),
            streaming: false,
        });
        state.messages.push(TranscriptEntry::Reasoning {
            id: "r1".into(),
            text: long.clone(),
            collapsed: false,
        });
        state.add_system_message(long.clone());

        let mut header_heavy = ToolRow::new("c1", "bash", &json!({ "command": long.clone() }));
        header_heavy.start(0);
        header_heavy.finish(&aleph_protocol::ToolResult::success("ok\n"), 10, 10);
        state.messages.push(TranscriptEntry::Tool(header_heavy));

        let mut diff_row = ToolRow::new("c2", "apply_patch", &json!({}));
        diff_row.start(0);
        diff_row.finish(&aleph_protocol::ToolResult::success("done"), 10, 10);
        diff_row.body = RowBody::FileChanges(vec![FileChange {
            path: format!("src/deeply/nested{}/name.rs", "/x".repeat(40)),
            kind: FileChangeKind::Modified,
            added: 1,
            removed: 1,
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                lines: vec![
                    HunkLine {
                        tag: LineTag::Del,
                        text: "y".repeat(300),
                    },
                    HunkLine {
                        tag: LineTag::Add,
                        text: "z".repeat(300),
                    },
                ],
            }],
            unavailable: None,
        }]);
        state.messages.push(TranscriptEntry::Tool(diff_row));

        state
            .messages
            .push(TranscriptEntry::TurnSummary(TurnSummaryEntry {
                commands: 12,
                reads: 30,
                edits: 8,
                writes: 3,
                others: 5,
                failed: 2,
                duration_ms: 80_000,
            }));
        state
    }

    /// An over-wide row above a tool row does not move that row's click
    /// target — i.e. one logical line still occupies exactly one screen row.
    ///
    /// # When this goes red
    ///
    /// Losing BOTH halves of the invariant: [`clip_to_width`] at the paint
    /// site and the absence of `Wrap` on the paragraph. Either alone still
    /// holds the line — a clipped line has nothing left to wrap, and an
    /// unwrapped paragraph truncates rather than reflows — so the mutation
    /// that reddens this is removing the clip AND putting `Wrap` back, which
    /// is what was measured. Stated this way rather than as "don't re-add
    /// `Wrap`" because an assertion whose named cause does not actually
    /// redden it is worse than no assertion (判据 §2, §3).
    ///
    /// Nothing else catches it: the buffer is exactly `width` columns wide
    /// either way, and the region table would agree with the line builder
    /// while both disagreed with the screen (判据 §10).
    ///
    /// The over-wide row goes ABOVE the tool row on purpose — a wrap below it
    /// would move nothing this test can see. It is also a tool HEADER: the
    /// first fixture used a system message, which wraps itself, and stayed
    /// green under both mutations.
    #[test]
    fn an_over_wide_row_above_a_tool_row_does_not_shift_its_click_target() {
        const W: u16 = 40;
        const H: u16 = 20;
        let mut state = AppState::new("s".into(), "m".into());
        state.messages.clear();
        state.mouse = true;
        // A tool HEADER, because it is one of the producers that does not
        // wrap on its own — a system message or a markdown paragraph would
        // have wrapped itself and proved nothing.
        let mut wide = ToolRow::new("c0", "bash", &json!({ "command": "w".repeat(200) }));
        wide.start(0);
        wide.finish(&aleph_protocol::ToolResult::success("ok\n"), 10, 10);
        state.messages.push(TranscriptEntry::Tool(wide));

        let mut row = ToolRow::new("c1", "bash", &json!({ "command": "ls" }));
        row.start(0);
        row.finish(
            &aleph_protocol::ToolResult::success("out\n".repeat(40)),
            10,
            10,
        );
        state.messages.push(TranscriptEntry::Tool(row));

        let backend = TestBackend::new(W, H);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_chat_area(f, &mut state, f.area()))
            .unwrap();
        let buf = term.backend().buffer().clone();
        let rows: Vec<String> = (0..H)
            .map(|y| {
                (0..W)
                    .map(|x| buf.cell((x, y)).map_or(" ", |c| c.symbol()).to_string())
                    .collect()
            })
            .collect();

        let hint_y = rows
            .iter()
            .position(|r| r.contains("to expand"))
            .unwrap_or_else(|| panic!("no hint painted:\n{}", rows.join("\n")));
        let hint_y = u16::try_from(hint_y).expect("a screen row fits u16");
        let hit = state.regions.hit(6, hint_y).unwrap_or_else(|| {
            panic!(
                "the hint painted at row {hint_y} has no click target there:\n{}",
                rows.join("\n")
            )
        });
        assert_eq!(hit.row_id, "c1");
    }

    /// A cut is marked, because a truncated path that looks whole reads as a
    /// whole path.
    #[test]
    fn a_clipped_row_says_it_was_clipped() {
        let backend = TestBackend::new(30, 12);
        let mut term = Terminal::new(backend).unwrap();
        let mut state = a_transcript_of_over_wide_rows();
        term.draw(|f| render_chat_area(f, &mut state, f.area()))
            .unwrap();
        let buf = term.backend().buffer().clone();
        let rows: Vec<String> = (0..12)
            .map(|y| {
                (0..30)
                    .map(|x| buf.cell((x, y)).map_or(" ", |c| c.symbol()).to_string())
                    .collect()
            })
            .collect();
        assert!(
            rows.iter().any(|r| r.contains('\u{2026}')),
            "nothing said it had been cut:\n{}",
            rows.join("\n")
        );
    }

    /// The clip splits by COLUMNS on a char boundary. A byte split would
    /// panic on the first CJK row; a char count would leave a line of them
    /// half empty.
    #[test]
    fn clipping_is_column_correct_on_wide_characters() {
        let line = Line::from(Span::raw("宽宽宽宽宽宽宽宽"));
        let clipped = clip_to_width(line, 7);
        let text: String = clipped.spans.iter().map(|s| s.content.as_ref()).collect();
        // 6 columns of CJK (3 chars) + the ellipsis = 7.
        assert_eq!(text, "宽宽宽\u{2026}");
        assert_eq!(UnicodeWidthStr::width(text.as_str()), 7);
    }
}
