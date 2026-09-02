// Agent panel widget: draws the runtime agent table (`runtime.agents.list`,
// kept fresh by `runtime.agents.changed` — Task 8a) in the sidebar's left
// column (Task 8b).
//
// Row-drawing shape ported from herdr's `render_agent_detail`
// (`herdr/src/ui/sidebar.rs`, ~L1432): a glyph, the label, and a dim
// detail suffix. Data access is NOT ported — herdr reads its own
// in-process `AppState`; this reads the slice Task 8a already fetched and
// stored on `AppState::runtime_agents`. This module never fetches and
// never sorts its own input: `sort_entries` (from `shared-ui-logic`,
// Task 7) is the only ordering operation here, called on a clone. A
// source-level guard (Task 10) fails the build if this file gains a
// `.sort_by`/`.sort()` of its own.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
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

/// The dim manifest-version suffix for one entry, or `None`.
///
/// `None` whenever `entry.agent` is `None` OR the manifest lookup itself
/// returns `None` (a recognised agent with no bundled screen manifest, or
/// one whose manifest declares no version) — both render nothing, never a
/// placeholder such as "unknown" or an empty `()` (R8-10, 判据 §17: a wrong
/// label costs more than a missing one).
fn manifest_suffix(entry: &RuntimeAgentEntry) -> Option<String> {
    let agent = agent_detect::identify_agent(entry.agent.as_deref()?)?;
    agent_detect::manifest_version(agent)
}

/// One row's spans: glyph (colored by state), the label, and the optional
/// dim manifest-version suffix.
fn entry_line(entry: &RuntimeAgentEntry) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            format!("{} ", state_glyph(entry.state)),
            Style::default().fg(state_color(entry.state)),
        ),
        Span::raw(entry.label.clone()),
    ];
    if let Some(version) = manifest_suffix(entry) {
        spans.push(Span::styled(
            format!(" {version}"),
            Style::default()
                .fg(DEFAULT_THEME.muted)
                .add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
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

    f.render_widget(Paragraph::new(lines), body);
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
            state,
            updated_at,
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

    /// Renders `data` into a fixed 30x6 backend and returns its rows.
    fn render(data: &AgentPanelData) -> Vec<String> {
        let backend = TestBackend::new(30, 6);
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
        assert!(refused.contains("operators only"));
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

    /// R8-10: a recognised agent with a bundled screen manifest gets a dim
    /// version suffix. `claude` is in `agent_detect::Agent::SCREEN_MANIFEST_AGENTS`
    /// (guarded by that crate's own `manifest_version_is_per_agent_and_matches_explain`
    /// test), so this is deterministic with no fixture of our own.
    ///
    /// Reddens if `manifest_suffix` is never called, or is called with the
    /// wrong field (e.g. `entry.label` instead of `entry.agent`).
    #[test]
    fn a_recognised_agent_with_a_manifest_gets_a_version_suffix() {
        let entries = vec![entry("s1", "claude", Some("claude"), S::Idle, 1)];
        let dump = render(&AgentPanelData::Ready(entries)).join("\n");

        let expected = agent_detect::manifest_version(
            agent_detect::identify_agent("claude").expect("claude is a recognised agent"),
        )
        .expect("claude has a bundled screen manifest");
        assert!(
            dump.contains(&expected),
            "manifest version {expected:?} must appear as a suffix in {dump:?}"
        );
    }

    /// R8-10's other half: a recognised agent with NO bundled manifest
    /// (`mastracode` — confirmed absent from `SCREEN_MANIFEST_AGENTS` by
    /// `agent-detect`'s own test) must render EXACTLY like no agent label
    /// at all: nothing appended, never a placeholder.
    ///
    /// Reddens if a placeholder (e.g. "(unknown)") is appended for either
    /// case, or if the two cases stop matching each other.
    #[test]
    fn an_agent_without_a_bundled_manifest_renders_identically_to_no_agent_label() {
        let with_unmanifested_agent = render(&AgentPanelData::Ready(vec![entry(
            "s1",
            "some-shell",
            Some("mastracode"),
            S::Idle,
            1,
        )]));
        let without_agent_label = render(&AgentPanelData::Ready(vec![entry(
            "s1",
            "some-shell",
            None,
            S::Idle,
            1,
        )]));
        assert_eq!(
            with_unmanifested_agent, without_agent_label,
            "no manifest version must render exactly like no agent label at all"
        );
    }
}
