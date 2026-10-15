//! Explicit, bounded paging of the existing masked tool-output RPC.

use aleph_protocol::{ToolOutputPage, ToolOutputSource};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::theme::theme;

pub(super) const PAGE_BYTES: u64 = 64 * 1024;
const MAX_VIEW_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub(super) struct ToolOutputView {
    pub session_key: String,
    pub tool_call_id: String,
    pub input_summary: Option<String>,
    pub generation: u64,
    pub text: String,
    pub next_offset: u64,
    pub total_bytes: u64,
    pub source: Option<ToolOutputSource>,
    pub loading: bool,
    pub load_due: bool,
    pub error: Option<String>,
    pub scroll: u16,
    has_more: bool,
}

impl ToolOutputView {
    pub fn new(session_key: String, tool_call_id: String, generation: u64) -> Self {
        Self {
            session_key,
            tool_call_id,
            input_summary: None,
            generation,
            text: String::new(),
            next_offset: 0,
            total_bytes: 0,
            source: None,
            loading: false,
            load_due: true,
            error: None,
            scroll: 0,
            has_more: true,
        }
    }

    pub fn can_load(&self) -> bool {
        !self.loading && self.has_more && self.text.len() < MAX_VIEW_BYTES
    }

    pub fn apply(&mut self, requested_offset: u64, page: ToolOutputPage) {
        self.loading = false;
        if page.tool_call_id != self.tool_call_id || requested_offset != self.next_offset {
            self.error = Some("Output page no longer matches this view; reopen it.".into());
            return;
        }
        // The server may round a mid-character offset down. Our own next
        // offset is always aligned; reject overlap rather than duplicating it.
        if page.offset != requested_offset {
            self.error = Some("Output changed while paging; reopen it.".into());
            return;
        }
        let end = page.offset.saturating_add(page.text.len() as u64);
        if end > page.total_bytes
            || (page.truncated && page.text.is_empty() && end < page.total_bytes)
        {
            self.error = Some("Output page did not advance; retry or reopen it.".into());
            return;
        }
        if let Some(source) = self.source {
            if source != page.source || self.total_bytes != page.total_bytes {
                self.error = Some("Output changed while paging; reopen it.".into());
                return;
            }
        }
        self.total_bytes = page.total_bytes;
        self.source = Some(page.source);
        let mut keep = page
            .text
            .len()
            .min(MAX_VIEW_BYTES.saturating_sub(self.text.len()));
        while !page.text.is_char_boundary(keep) {
            keep -= 1;
        }
        self.text.push_str(&page.text[..keep]);
        self.next_offset = page.offset.saturating_add(keep as u64);
        // Expired means the stored original is gone, not that another request
        // can recover it. Never poll on `truncated` alone.
        self.has_more = page.source != ToolOutputSource::Expired
            && end < page.total_bytes
            && keep == page.text.len();
        self.error = None;
    }

    pub fn fail(&mut self) {
        self.loading = false;
        // RPC errors may quote payloads. This surface deliberately uses a
        // fixed explanation; a retry is always an explicit user action.
        self.error = Some("Tool output unavailable. Press n to retry.".into());
    }

    pub fn scroll_by(&mut self, delta: i16) {
        self.scroll = self.scroll.saturating_add_signed(delta);
    }

    fn status(&self) -> String {
        let source = match self.source {
            None => "source pending",
            Some(ToolOutputSource::Inline) => "inline",
            Some(ToolOutputSource::Persisted) => "persisted",
            Some(ToolOutputSource::Expired) => "expired original; retained excerpt only",
        };
        let suffix = if self.loading {
            "loading"
        } else if self.text.len() >= MAX_VIEW_BYTES {
            "view limit reached (1 MiB)"
        } else if self.has_more {
            "n: load next page"
        } else {
            "end of available output"
        };
        format!(
            "{} / {} bytes · {source} · {suffix}",
            self.next_offset, self.total_bytes
        )
    }
}

pub(super) fn render(frame: &mut Frame, view: &mut ToolOutputView, area: Rect) {
    if area.width < 4 || area.height < 4 {
        return;
    }
    let popup = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().border_focused))
        .title(" Tool output · ↑↓ / PgUp PgDn · n next · Esc close ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let mut lines = vec![Line::from(Span::styled(
        view.status(),
        Style::default().fg(theme().muted),
    ))];
    if let Some(summary) = &view.input_summary {
        lines.push(Line::from(format!("Input summary: {summary}")));
    }
    if let Some(error) = &view.error {
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    // Keep status fixed while scrolling the output; Paragraph computes the
    // actual wrapped height, so the last page cannot scroll into blank space.
    let header_rows = lines.len() as u16;
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            height: header_rows.min(inner.height),
            ..inner
        },
    );
    let body = Rect {
        y: inner.y.saturating_add(header_rows),
        height: inner.height.saturating_sub(header_rows),
        ..inner
    };
    if body.width == 0 || body.height == 0 {
        return;
    }
    let rows: Vec<Line<'_>> = view
        .text
        .split('\n')
        .flat_map(|line| {
            textwrap::wrap(line, body.width as usize)
                .into_iter()
                .map(Line::from)
        })
        .collect();
    let total = rows.len().min(u16::MAX as usize) as u16;
    view.scroll = view.scroll.min(total.saturating_sub(body.height));
    frame.render_widget(Paragraph::new(rows).scroll((view.scroll, 0)), body);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(text: &str, offset: u64, total_bytes: u64, source: ToolOutputSource) -> ToolOutputPage {
        ToolOutputPage {
            tool_call_id: "tool".into(),
            text: text.into(),
            offset,
            total_bytes,
            truncated: offset + (text.len() as u64) < total_bytes
                || source == ToolOutputSource::Expired,
            source,
        }
    }

    #[test]
    fn pages_preserve_utf8_and_reject_overlap() {
        let mut view = ToolOutputView::new("s".into(), "tool".into(), 1);
        view.apply(0, page("你好", 0, 7, ToolOutputSource::Persisted));
        assert_eq!(view.next_offset, 6);
        view.apply(0, page("你好", 0, 7, ToolOutputSource::Persisted));
        assert!(view.error.is_some());
        assert_eq!(view.text, "你好");
        view.apply(6, page("!", 6, 7, ToolOutputSource::Persisted));
        assert_eq!(view.text, "你好!");
        assert!(!view.can_load());
    }

    #[test]
    fn expired_truncated_is_terminal_after_retained_excerpt() {
        let mut view = ToolOutputView::new("s".into(), "tool".into(), 1);
        view.apply(0, page("excerpt", 0, 7, ToolOutputSource::Expired));
        assert!(!view.can_load());
        assert!(view.status().contains("expired original"));
    }

    #[test]
    fn errors_preserve_loaded_bytes_and_need_explicit_retry() {
        let mut view = ToolOutputView::new("s".into(), "tool".into(), 1);
        view.apply(0, page("a", 0, 2, ToolOutputSource::Inline));
        view.fail();
        assert_eq!(view.text, "a");
        assert!(view.can_load());
        assert_eq!(view.next_offset, 1);
        view.apply(1, page("", 1, 2, ToolOutputSource::Inline));
        assert!(view.error.is_some());
        assert_eq!(view.next_offset, 1);
    }
}
