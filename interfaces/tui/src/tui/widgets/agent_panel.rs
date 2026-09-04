// Agent panel widget: draws the runtime agent table (`runtime.agents.list`,
// kept fresh by `runtime.agents.changed` — Task 8a) in the sidebar's left
// column (Task 8b).
//
// Row-drawing shape ported from herdr's `render_agent_detail`
// (`herdr/src/ui/sidebar.rs`, ~L1432): a glyph and the label. Data access is
// NOT ported — herdr reads its own in-process `AppState`; this reads the
// slice Task 8a already fetched and stored on `AppState::runtime_agents`.
// This module never fetches and never sorts its own input: `sort_entries`
// (from `shared-ui-logic`, Task 7) is the only ordering operation here,
// called on a clone. A source-level guard (Task 10) fails the build if this
// file gains an ordering call of its own.
//
// A dim manifest-version suffix (R8-10) was here and was removed (Task 9
// fix round 1, F4): `agent_detect::manifest_version` returns the CalVer
// stamp of our own bundled detection-rules TOML
// (`crates/agent-detect/src/manifests/*.toml`'s `version` key — see e.g.
// `claude.toml:2`, next to its own `updated_at`), not the agent program's
// version. Rendered next to the agent's name it reads as the latter to
// every user who sees it — 判据 §17: a wrong label costs more than a
// missing one. `agent_label` (the display name) is unaffected; it was
// always the part that meant something.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState};
use shared_ui_logic::state::agent_panel::sort_entries;

use crate::tui::app::AgentPanelData;
use crate::tui::theme::DEFAULT_THEME;

/// Column width in cells when the panel is shown.
///
/// A named constant, not a magic number in `render.rs` (R8-9). Not derived
/// from `AgentPanelState::split_ratio`: that field is a RATIO owned by the
/// Panel's draggable divider (R7-2 names Task 9's drag handle as its
/// driver), and R8-0 scopes drag-to-resize to the Panel only — the TUI
/// column this phase is a plain toggle with no divider to drag, so there is
/// nothing here for a ratio to measure. R8-9 says a cell-measured TUI
/// column "is not obliged to use it at all"; this one does not.
pub const AGENT_PANEL_WIDTH: u16 = 28;

/// One glyph set. herdr keeps two (Dots / Symbols in `ui/status.rs`); R8-1
/// picks one for this port. `Unknown` gets `?`, never `Idle`'s glyph — an
/// unrecognised state must never be misread as "nothing is happening here"
/// (判据 §8).
fn state_glyph(state: RuntimeAgentState) -> &'static str {
    match state {
        RuntimeAgentState::Blocked => "\u{25cf}", // ●
        RuntimeAgentState::Working => "\u{25d0}", // ◐
        RuntimeAgentState::Idle => "\u{25cb}",    // ○
        RuntimeAgentState::Unknown => "?",
    }
}

fn state_color(state: RuntimeAgentState) -> Color {
    match state {
        RuntimeAgentState::Blocked => DEFAULT_THEME.error,
        RuntimeAgentState::Working => DEFAULT_THEME.warning,
        RuntimeAgentState::Idle | RuntimeAgentState::Unknown => DEFAULT_THEME.muted,
    }
}

/// One row's spans: glyph (colored by state) and the label.
fn entry_line(entry: &RuntimeAgentEntry) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{} ", state_glyph(entry.state)),
            Style::default().fg(state_color(entry.state)),
        ),
        Span::raw(entry.label.clone()),
    ])
}

/// Render the agent panel into `area`.
///
/// Distinguishes all four [`AgentPanelData`] states on screen (R8-11):
/// `Loading` is not an empty panel, `Ready(vec![])` is not `Refused`, and
/// `Refused`/`Unavailable` each show their own message rather than
/// collapsing into "no agents" — that collapse would delete the fix R8-6
/// made one layer up (a refusal is not an absence).
pub fn render_agent_panel(f: &mut Frame, area: Rect, data: &AgentPanelData) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let header = Line::from(Span::styled(
        "agents",
        Style::default()
            .fg(DEFAULT_THEME.primary)
            .add_modifier(Modifier::BOLD),
    ));
    f.render_widget(
        Paragraph::new(header),
        Rect::new(area.x, area.y, area.width, 1),
    );

    if area.height < 2 {
        return;
    }
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);

    let muted = Style::default().fg(DEFAULT_THEME.muted);
    let lines: Vec<Line<'static>> = match data {
        AgentPanelData::Loading => vec![Line::from(Span::styled("loading…", muted))],
        AgentPanelData::Ready(entries) if entries.is_empty() => {
            vec![Line::from(Span::styled("no agents running", muted))]
        }
        AgentPanelData::Ready(entries) => {
            // The ONLY ordering operation in this file: `sort_entries` on a
            // clone. This widget never sorts its own input.
            let mut sorted = entries.clone();
            sort_entries(&mut sorted);
            sorted.iter().map(entry_line).collect()
        }
        AgentPanelData::Refused(message) => vec![Line::from(Span::styled(
            format!("access denied: {message}"),
            Style::default().fg(DEFAULT_THEME.warning),
        ))],
        AgentPanelData::Unavailable(message) => vec![Line::from(Span::styled(
            format!("unavailable: {message}"),
            Style::default().fg(DEFAULT_THEME.error),
        ))],
    };

    // Wrapped: at AGENT_PANEL_WIDTH (28), the real operator-gate refusal
    // (`aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE`) is longer than the
    // panel is wide and cannot fit on one row no matter how short a prefix
    // gets — R8-6 carried that message through four variants specifically
    // so a human could read WHY the panel is empty, and an unwrapped single
    // line would truncate it to a handful of characters past the prefix,
    // silently deleting that work at the last inch.
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::runtime::RuntimeAgentState as S;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn entry(
        session_id: &str,
        label: &str,
        agent: Option<&str>,
        state: S,
        updated_at: i64,
    ) -> RuntimeAgentEntry {
        RuntimeAgentEntry {
            session_id: session_id.to_string(),
            label: label.to_string(),
            cwd: String::new(),
            agent: agent.map(str::to_string),
            program: None,
            state,
            updated_at,
            quiet_since: None,
        }
    }

    /// Joins one row's cell symbols into a plain `String` — robust against
    /// `Buffer`'s `Debug` escaping, per R8-1's fallback note.
    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| {
                buf.cell((x, y))
                    .map(|cell| cell.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height).map(|y| row_text(buf, y)).collect()
    }

    /// Renders `data` into a backend at the SHIPPED width (`AGENT_PANEL_WIDTH`)
    /// and returns its rows. Any other width is an instrument that
    /// disagrees with the artefact (判据 §18) — a wider backend can make an
    /// assertion true that would be false in the product.
    fn render(data: &AgentPanelData) -> Vec<String> {
        render_at(AGENT_PANEL_WIDTH, 6, data)
    }

    /// As [`render`], but with an explicit height — for messages long
    /// enough that they need more than 5 body rows once wrapped.
    fn render_at(width: u16, height: u16, data: &AgentPanelData) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_agent_panel(f, f.area(), data))
            .unwrap();
        rows(term.backend().buffer())
    }

    /// The brief's Step-1 test (R8-11: one test per state, amended into
    /// this crate's four-state model). Blocked sorts before idle; unknown
    /// renders its OWN glyph, never idle's.
    ///
    /// Reddens if: `sort_entries` is skipped or replaced by a homegrown
    /// sort that does not rank `Blocked` first; or if `state_glyph` maps
    /// `Unknown` to the same glyph as `Idle`.
    #[test]
    fn blocked_renders_first_and_unknown_is_not_shown_as_idle() {
        let entries = vec![
            entry("i", "idle-one", None, S::Idle, 1),
            entry("b", "blocked-one", None, S::Blocked, 1),
            entry("u", "unknown-one", None, S::Unknown, 1),
        ];
        let dump = render(&AgentPanelData::Ready(entries)).join("\n");

        let blocked_at = dump.find("blocked-one").expect("blocked row must render");
        let idle_at = dump.find("idle-one").expect("idle row must render");
        assert!(blocked_at < idle_at, "blocked must sort before idle");

        assert!(
            !dump.contains("\u{25cb} unknown-one"),
            "unknown must not use the idle glyph"
        );
        assert!(
            dump.contains("? unknown-one"),
            "unknown must render its own glyph"
        );
    }

    /// R8-11: `Loading` must not read as an empty panel.
    ///
    /// Reddens if the `Loading` arm is dropped and falls through to
    /// whatever the empty-`Ready` text is (or to nothing at all).
    #[test]
    fn loading_is_not_an_empty_panel() {
        let dump = render(&AgentPanelData::Loading).join("\n");
        assert!(
            dump.contains('…'),
            "loading state must show a loading indicator, not blank rows: {dump:?}"
        );
        assert!(!dump.to_lowercase().contains("no agents"));
    }

    /// R8-11's explicit bar: `Ready(vec![])` and `Refused(..)` must render
    /// different text — collapsing them back together would delete R8-6's
    /// fix (a refusal is not an absence).
    ///
    /// Reddens if `Ready(vec![])`'s branch is merged with `Refused`'s (or
    /// vice versa) so both print the same line.
    #[test]
    fn ready_empty_renders_a_no_agents_line_distinct_from_refused() {
        let ready = render(&AgentPanelData::Ready(vec![])).join("\n");
        let refused = render(&AgentPanelData::Refused("operators only".to_string())).join("\n");

        assert_ne!(
            ready, refused,
            "an empty Ready and a Refused must not render identically"
        );
        assert!(ready.to_lowercase().contains("no agents"));
        // "access denied: operators only" is 30 chars, wider than
        // AGENT_PANEL_WIDTH (28) — it wraps, so "operators only" does not
        // survive as one contiguous substring of `refused` (word-wrapping
        // inserts a row boundary, not a space). Check the two words
        // separately instead of assuming they land on the same line.
        assert!(refused.contains("operators"));
        assert!(refused.contains("only"));
    }

    /// The fourth state: `Unavailable` must also be distinguishable from
    /// `Refused` — a transport failure is not an operator-gate refusal.
    ///
    /// Reddens if both variants share one rendering branch.
    #[test]
    fn refused_and_unavailable_render_differently() {
        let refused = render(&AgentPanelData::Refused("nope".to_string())).join("\n");
        let unavailable = render(&AgentPanelData::Unavailable("timed out".to_string())).join("\n");

        assert_ne!(refused, unavailable);
        assert!(unavailable.contains("timed out"));
    }

    /// R8-6 carried a refusal message through four variants specifically so
    /// a human could read WHY the panel is empty — a message truncated to a
    /// handful of characters past its prefix deletes that work at the last
    /// inch. Uses the REAL production text
    /// (`aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE` — what
    /// `agent_panel_data` in `mod.rs` actually wraps into `Refused` for a
    /// non-operator), not a short test fixture, rendered at the SHIPPED
    /// width (`AGENT_PANEL_WIDTH`, not a wider instrument — 判据 §18).
    ///
    /// The message is longer than the panel is wide, so it wraps across
    /// several rows; checks distinguishing WORDS survive rather than one
    /// contiguous phrase, since a word can land on either side of a row
    /// boundary once wrapped.
    ///
    /// Reddens if `Wrap` is removed from the body `Paragraph` — the message
    /// then truncates to roughly 13 characters past the "access denied: "
    /// prefix and none of these words reach the buffer at all.
    #[test]
    fn a_realistic_refusal_message_survives_wrapped_at_the_shipped_width() {
        let dump = render_at(
            AGENT_PANEL_WIDTH,
            10,
            &AgentPanelData::Refused(aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE.to_string()),
        )
        .join("\n");

        for word in ["operator", "privileges", "authorized"] {
            assert!(
                dump.contains(word),
                "distinguishing word {word:?} must survive to the buffer \
                 at AGENT_PANEL_WIDTH; got {dump:?}"
            );
        }
    }

}
