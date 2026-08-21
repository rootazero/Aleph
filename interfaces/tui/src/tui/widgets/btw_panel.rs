// Side-question overlay widget (`/btw`).
//
// Renders the question on screen — the one being answered, or the page of
// history the user paged to — plus a key legend. Draws over the whole frame
// like the approval overlay, because a side question is a conversation of its
// own and reading it half-obscured by the transcript it is deliberately NOT
// part of would be actively confusing.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::btw_overlay::BtwOverlay;
use crate::tui::theme::DEFAULT_THEME;

/// Spinner frames, matching the status bar's cadence.
const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

/// The title line: which page of the side thread is on screen.
///
/// A live question is always the thing on screen, so it is titled as such; a
/// settled one is titled with its position, because "3 of 7" is the only thing
/// that tells the user paging is even possible.
fn title(overlay: &BtwOverlay) -> String {
    if overlay.active.is_some() {
        return " Side question (answering) ".to_string();
    }
    if overlay.exchanges.is_empty() {
        return " Side question ".to_string();
    }
    format!(
        " Side question {}/{} ",
        overlay.view_index + 1,
        overlay.exchanges.len()
    )
}

/// The key legend for the overlay's current mode.
///
/// Mode-dependent because the keys are: in compose mode the letters are text,
/// so advertising `c copy` there would name a key that does something else.
fn legend(overlay: &BtwOverlay) -> &'static str {
    if overlay.composing {
        // Esc says "keep draft" rather than "abort/close" because that is
        // what it does with text in the buffer — it steps back to browse and
        // keeps the draft, and only the NEXT Esc aborts or closes. A legend
        // that named the second behaviour would be advertising data loss the
        // key does not perform.
        "Enter send · Tab browse · ←→ page · ↑↓ scroll · Esc keep draft"
    } else {
        "Tab or type to reply · c copy · p promote · ←→ page · ↑↓ scroll · Esc abort/close"
    }
}

/// The question and answer to display, plus the status word beside the
/// question.
///
/// A live question always wins: it is what the user is waiting for. Otherwise
/// it is whichever settled exchange they paged to.
fn body(overlay: &BtwOverlay, spinner_frame: usize) -> (String, String, String) {
    if let Some(active) = &overlay.active {
        let spin = SPINNER[spinner_frame % SPINNER.len()];
        let status = match &active.tool_name {
            Some(tool) => format!("{spin} {tool}"),
            None => format!("{spin} thinking"),
        };
        return (active.question.clone(), active.answer.clone(), status);
    }
    match overlay.current() {
        Some(exchange) => {
            // The failure goes in the BODY, not in the status line.
            //
            // That line is one row tall, so a multi-line provider error
            // rendered there showed its first line and dropped the rest with
            // nowhere else to read it — and the interesting part of an error
            // is rarely its first line. The body region already wraps and
            // scrolls, so the whole text is reachable; the status line keeps
            // the one word that fits a single row by construction.
            let body = match &exchange.error {
                Some(err) if exchange.answer.is_empty() => format!("Error: {err}"),
                Some(err) => format!("Error: {err}\n\n{}", exchange.answer),
                None => exchange.answer.clone(),
            };
            (
                exchange.question.clone(),
                body,
                exchange.status().to_string(),
            )
        }
        None => (
            String::new(),
            "Ask a side question with /btw <question>.".to_string(),
            String::new(),
        ),
    }
}

/// Render the side-question overlay over `area`.
pub fn render_btw_panel(frame: &mut Frame, overlay: &BtwOverlay, spinner_frame: usize, area: Rect) {
    let width = area.width.saturating_sub(6).clamp(30, 100);
    let height = area.height.saturating_sub(4).clamp(9, 30);
    let rect = centered_rect(width, height, area);

    frame.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.border_focused))
        .title(title(overlay));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(2), // question (wraps to two lines)
        Constraint::Length(1), // status
        Constraint::Min(1),    // answer
        Constraint::Length(1), // composer
        Constraint::Length(1), // legend
    ])
    .split(inner);

    let (question, answer, status) = body(overlay, spinner_frame);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            question,
            Style::default()
                .fg(DEFAULT_THEME.user)
                .add_modifier(Modifier::BOLD),
        )))
        .wrap(Wrap { trim: true }),
        chunks.first().copied().unwrap_or_default(),
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            status,
            Style::default().fg(DEFAULT_THEME.muted),
        ))),
        chunks.get(1).copied().unwrap_or_default(),
    );

    // Raw markdown, not the transcript's rendered form: this is the text `c`
    // copies, and showing something other than what would be copied is the
    // kind of small lie that costs a user a paste.
    frame.render_widget(
        Paragraph::new(answer)
            .wrap(Wrap { trim: false })
            .scroll((overlay.scroll, 0)),
        chunks.get(2).copied().unwrap_or_default(),
    );

    let composer_style = if overlay.composing {
        Style::default().fg(DEFAULT_THEME.primary)
    } else {
        Style::default().fg(DEFAULT_THEME.muted)
    };
    let composer = if overlay.composing {
        format!("> {}\u{2588}", overlay.composer)
    } else {
        format!("> {}", overlay.composer)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(composer, composer_style))),
        chunks.get(3).copied().unwrap_or_default(),
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            legend(overlay),
            Style::default().fg(DEFAULT_THEME.muted),
        ))),
        chunks.get(4).copied().unwrap_or_default(),
    );
}

/// Calculate a centered rect within the given area.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::btw_overlay::BtwExchange;

    /// Open a question and claim the run answering it — frame applications
    /// name their run.
    fn asking(o: &mut BtwOverlay, question: &str, run_id: &str) {
        o.begin(question.to_string());
        o.claim_pending_run(run_id.to_string());
    }

    /// A live question is what the user is waiting for, so it is what the
    /// panel shows — even when they had paged back through history first.
    #[test]
    fn a_live_question_outranks_the_page_that_was_on_screen() {
        let mut o = BtwOverlay::default();
        o.finish_exchange(BtwExchange::answered("old q", "old a"));
        asking(&mut o, "new q", "r1");
        o.push_delta("r1", "partial");
        let (question, answer, status) = body(&o, 0);
        assert_eq!(question, "new q");
        assert_eq!(answer, "partial");
        assert!(status.contains("thinking"), "got: {status}");
    }

    /// The status line names the running tool while one is running — a side
    /// question that goes quiet for thirty seconds should say what it is doing.
    #[test]
    fn the_status_line_names_the_running_tool() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "q", "r1");
        o.note_tool("r1", Some("file_read".into()));
        let (_, _, status) = body(&o, 0);
        assert!(status.contains("file_read"), "got: {status}");
    }

    /// A failure says what failed — all of it.
    ///
    /// The status line is one row tall, so an error rendered there loses
    /// every line after the first and there is nowhere else on screen to read
    /// it. Providers do not keep their errors to one line: the useful part is
    /// usually the response body, several lines down.
    #[test]
    fn a_failed_exchange_shows_every_line_of_the_reason() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "q", "r1");
        o.fail_active(
            "r1",
            "provider unreachable\nHTTP 503 from api.example.com\nretry-after: 30".into(),
        );

        let (_, shown, status) = body(&o, 0);
        assert_eq!(
            status, "failed",
            "the one-row line holds a word, not a body"
        );
        for line in [
            "provider unreachable",
            "HTTP 503 from api.example.com",
            "retry-after: 30",
        ] {
            assert!(
                shown.contains(line),
                "line missing from the scrollable body: {line:?} — got {shown:?}"
            );
        }
    }

    /// A failure that arrived after some text keeps both, in that order: the
    /// reason it stopped, then how far it got.
    #[test]
    fn a_partial_answer_survives_its_failure() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "q", "r1");
        o.push_delta("r1", "as far as I got");
        o.fail_active("r1", "connection reset".into());

        let (_, shown, _) = body(&o, 0);
        assert!(shown.contains("connection reset"));
        assert!(shown.contains("as far as I got"));
        assert!(
            shown.find("connection reset") < shown.find("as far as I got"),
            "the reason belongs above the partial answer: {shown:?}"
        );
    }

    /// The title is the only thing that tells the user paging exists, so it
    /// has to carry the position once there is more than one page.
    #[test]
    fn the_title_carries_the_position_once_history_exists() {
        let mut o = BtwOverlay::default();
        assert_eq!(title(&o), " Side question ");

        o.finish_exchange(BtwExchange::answered("q1", "a1"));
        o.finish_exchange(BtwExchange::answered("q2", "a2"));
        assert_eq!(title(&o), " Side question 2/2 ");
        o.page_left();
        assert_eq!(title(&o), " Side question 1/2 ");

        asking(&mut o, "q3", "r3");
        assert_eq!(title(&o), " Side question (answering) ");
    }

    /// The legend must not advertise a key that does something else in the
    /// current mode: in compose mode `c` is the letter c.
    #[test]
    fn the_legend_only_advertises_keys_that_are_live_in_this_mode() {
        let mut o = BtwOverlay::default();
        assert!(legend(&o).contains("c copy"));
        o.composing = true;
        assert!(
            !legend(&o).contains("c copy"),
            "compose mode sends c to the composer, not to copy"
        );
        assert!(legend(&o).contains("Enter send"));
    }

    /// An empty overlay says how to use it rather than rendering a blank box.
    #[test]
    fn an_empty_overlay_explains_itself() {
        let o = BtwOverlay::default();
        let (question, answer, _) = body(&o, 0);
        assert!(question.is_empty());
        assert!(answer.contains("/btw"), "got: {answer}");
    }
}
