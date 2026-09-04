//! The terminal's tab strip: a pure model, and the component that draws it.
//!
//! D2: this view could only ever show ONE terminal — whichever session
//! `pty.list` happened to name first — so a user running three agents had no
//! way to reach the other two from here at all. The model below is what makes
//! "which session am I looking at" a thing the page can answer, and the agent
//! panel's row click is what makes it answerable from the place a user
//! notices the problem (spec R2-4, D3).
//!
//! Model and component are separated for the usual reason: everything
//! interesting here is a merge and a selection rule, and a Leptos effect
//! cannot be asked about either in a unit test.
//!
//! # What this model is NOT allowed to do
//!
//! It never orders agent entries. Ordering for the agent PANEL lives in
//! `shared_ui_logic::state::agent_panel::sort_entries` and nowhere else (R2),
//! and a source-level guard in `alephcore` enforces that on both frontends'
//! `agent_panel.rs`. Tab order is a different question with a different
//! answer — it is the order the sessions were opened in, which is the order
//! `pty.list` returns them in and which must stay stable across refreshes so
//! tabs do not swap places under the pointer. So this file joins agent state
//! onto tabs BY `session_id` and takes its order from the session list.

use aleph_protocol::pty::PtySessionInfo;
use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState};
use leptos::prelude::*;
use shared_ui_logic::state::agent_panel::state_glyph;

use crate::i18n::{t_string, use_i18n};

/// One tab: a session, plus whatever the runtime sampler knows about it.
///
/// `state` and `program` are `Option` because the two lists are independent:
/// `pty.list` answers "which sessions exist" and `runtime.agents.list`
/// answers "what is running in them", and the second can be silent about a
/// session the first knows perfectly well. `None` there means the sampler has
/// nothing to say — never `Idle`, which is a claim (判据 §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabEntry {
    pub session_id: String,
    /// What the tab shows. Derived by [`derive_title`] and never empty.
    pub title: String,
    pub state: Option<RuntimeAgentState>,
    pub program: Option<String>,
    /// Set by [`TabModel::on_exit`] and by nothing else: a session the server
    /// reports closed in `pty.list` is dropped outright by
    /// [`TabModel::reconcile`], so this only ever means "we saw `pty.exit`
    /// for this one and have not re-listed yet".
    pub closed: bool,
    /// The last OSC title this session set. Kept out of `title` so a
    /// reconcile can re-derive the rest without discarding it.
    osc_title: Option<String>,
    /// The shell the session was spawned as — the last-resort name.
    shell: String,
}

/// A tab's display name, in falling order of how much it actually knows:
/// the title the program set for itself, then the foreground program, then
/// the shell the session was started as, then the session id.
///
/// The last fallback is not decoration. A tab has to be clickable, and a tab
/// with no text is a control the user cannot see or describe — 判据 §17 says
/// a wrong label costs more than a missing one, but a *missing* label on a
/// control still costs. The session id is at least true.
///
/// An empty string is treated as absent at every level: the server sends
/// `""` for "no title" and for "the spawn inherited the server's shell", and
/// rendering that as a name would make an empty tab look like a bug in the
/// terminal rather than a gap in what we know.
fn derive_title(
    osc_title: Option<&str>,
    program: Option<&str>,
    shell: &str,
    session_id: &str,
) -> String {
    fn present(value: Option<&str>) -> Option<&str> {
        value.filter(|v| !v.is_empty())
    }
    present(osc_title)
        .or_else(|| present(program))
        .or_else(|| present(Some(shell)))
        .unwrap_or(session_id)
        .to_string()
}

/// Which sessions have tabs, in what order, and which one is showing.
///
/// Deliberately owns its own order rather than re-deriving one on every
/// render: tabs that reorder themselves while a user is aiming at one are a
/// worse failure than a stale order, and `pty.list` makes no ordering
/// promise across calls.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TabModel {
    tabs: Vec<TabEntry>,
    /// The `session_id` of the showing tab. Held as an id rather than an
    /// index so nothing can silently re-point it at a different session when
    /// the list changes shape; every mutator below re-establishes the
    /// invariant that it names a tab that is present and open, so
    /// [`TabModel::selected`] is a lookup and not a second decision (判据 §6).
    selected: Option<String>,
}

impl TabModel {
    #[must_use]
    pub fn tabs(&self) -> &[TabEntry] {
        &self.tabs
    }

    /// The showing session's id, for the code that has to name it in an RPC.
    #[must_use]
    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The showing tab, or `None` when there is nothing open to show.
    #[must_use]
    pub fn selected(&self) -> Option<&TabEntry> {
        let id = self.selected.as_deref()?;
        self.tabs.iter().find(|t| t.session_id == id)
    }

    /// Point at `session_id`. Answers `false` — and changes nothing — when
    /// no open tab has that id.
    ///
    /// Refusing rather than creating is the whole point: the id arrives from
    /// outside (an agent-panel row that may be a refresh behind), and a tab
    /// invented for a session that no longer exists would attach to nothing
    /// and show a blank screen with no explanation of why.
    pub fn select(&mut self, session_id: &str) -> bool {
        if !self
            .tabs
            .iter()
            .any(|t| t.session_id == session_id && !t.closed)
        {
            return false;
        }
        self.selected = Some(session_id.to_string());
        true
    }

    /// Adopt an OSC title for one session. Unknown ids are ignored — the
    /// screen topic carries every session's frames, including ones this
    /// model has not listed yet.
    pub fn on_title(&mut self, session_id: &str, title: &str) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.session_id == session_id) {
            tab.osc_title = Some(title.to_string());
            tab.title = derive_title(
                tab.osc_title.as_deref(),
                tab.program.as_deref(),
                &tab.shell,
                &tab.session_id,
            );
        }
    }

    /// `pty.exit` for one session: mark it closed and, if it was showing,
    /// move to a neighbour.
    ///
    /// The row is kept rather than removed. A tab that disappears the instant
    /// its process exits takes the last frames of output with it, and the
    /// user is left without the one thing that would explain what happened.
    /// The next [`Self::reconcile`] drops it, by which point the server has
    /// confirmed the same fact.
    pub fn on_exit(&mut self, session_id: &str) {
        let Some(idx) = self.tabs.iter().position(|t| t.session_id == session_id) else {
            return;
        };
        self.tabs[idx].closed = true;
        if self.selected.as_deref() == Some(session_id) {
            self.selected = self.neighbour_of(idx);
        }
    }

    /// The nearest open tab to `idx`: the next one, else the previous one.
    fn neighbour_of(&self, idx: usize) -> Option<String> {
        self.tabs
            .iter()
            .skip(idx + 1)
            .find(|t| !t.closed)
            .or_else(|| self.tabs[..idx].iter().rev().find(|t| !t.closed))
            .map(|t| t.session_id.clone())
    }

    /// Merge a fresh `pty.list` and `runtime.agents.list` into the tab strip.
    ///
    /// Order: tabs already on screen keep their positions, then sessions this
    /// model has not seen are appended in the order the server listed them.
    /// Sessions the server reports `closed` are dropped.
    ///
    /// The agent list is joined BY `session_id` and its order is never read —
    /// ordering agent entries belongs to
    /// `shared_ui_logic::state::agent_panel::sort_entries` (R2), and this
    /// file has no business having an opinion about it.
    pub fn reconcile(&mut self, sessions: &[PtySessionInfo], agents: &[RuntimeAgentEntry]) {
        let mut next: Vec<TabEntry> = Vec::with_capacity(sessions.len());
        for tab in &self.tabs {
            if let Some(session) = sessions
                .iter()
                .find(|s| s.session_id == tab.session_id && !s.closed)
            {
                next.push(Self::merged(Some(tab), session, agents));
            }
        }
        for session in sessions.iter().filter(|s| !s.closed) {
            if next.iter().any(|t| t.session_id == session.session_id) {
                continue;
            }
            next.push(Self::merged(None, session, agents));
        }
        self.tabs = next;

        // The selection must always name a tab that is present and open. A
        // selection left pointing at a session the server no longer lists
        // would leave `selected()` answering `None` while `selected_id()`
        // still named it — two answers to one question, and the view would
        // attach to neither.
        let still_open = self
            .selected
            .as_deref()
            .is_some_and(|id| self.tabs.iter().any(|t| t.session_id == id && !t.closed));
        if !still_open {
            self.selected = self
                .tabs
                .iter()
                .find(|t| !t.closed)
                .map(|t| t.session_id.clone());
        }
    }

    /// Adopt a session `pty.spawn` just created, without waiting for a
    /// `pty.list` to confirm it exists.
    ///
    /// The spawn response IS the authoritative fact about a session this
    /// client just asked for, and re-listing to discover what we created is a
    /// second round trip that can itself fail — leaving a running shell with
    /// no tab pointing at it, which is exactly the shape D2 was.
    ///
    /// Idempotent on `session_id`: a retry that lands twice updates the
    /// existing tab rather than growing a duplicate.
    pub fn adopt_spawned(&mut self, session_id: &str, shell: &str) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.session_id == session_id) {
            tab.closed = false;
            tab.shell = shell.to_string();
            tab.title = derive_title(
                tab.osc_title.as_deref(),
                tab.program.as_deref(),
                &tab.shell,
                &tab.session_id,
            );
        } else {
            self.tabs.push(TabEntry {
                session_id: session_id.to_string(),
                title: derive_title(None, None, shell, session_id),
                state: None,
                program: None,
                closed: false,
                osc_title: None,
                shell: shell.to_string(),
            });
        }
        self.selected = Some(session_id.to_string());
    }

    /// One session's row, carrying forward whatever the previous tab for it
    /// already knew (its OSC title) and re-reading everything else.
    fn merged(
        previous: Option<&TabEntry>,
        session: &PtySessionInfo,
        agents: &[RuntimeAgentEntry],
    ) -> TabEntry {
        let agent = agents.iter().find(|a| a.session_id == session.session_id);
        let osc_title = previous.and_then(|p| p.osc_title.clone());
        let program = agent.and_then(|a| a.program.clone());
        let title = derive_title(
            osc_title.as_deref(),
            program.as_deref(),
            &session.shell,
            &session.session_id,
        );
        TabEntry {
            session_id: session.session_id.clone(),
            title,
            state: agent.map(|a| a.state),
            program,
            closed: false,
            osc_title,
            shell: session.shell.clone(),
        }
    }
}

/// The tab strip.
///
/// Purely presentational: it holds no state, decides nothing, and hands every
/// interaction back to the view that owns the [`TabModel`]. That is what keeps
/// the selection rules in one place instead of half here and half in the
/// parent (判据 §6).
///
/// Rebuilt wholesale on every change to `tabs`, the same idiom
/// `components/sidebar/agent_panel.rs` uses for its rows — this crate has no
/// keyed-`<For>` usage anywhere, and a strip of at most a handful of tabs is
/// not where that would start.
#[component]
pub fn TabBar(
    tabs: Signal<Vec<TabEntry>>,
    selected: Signal<Option<String>>,
    on_select: Callback<String>,
    on_close: Callback<String>,
    on_new: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div
            class="flex items-stretch gap-0.5 shrink-0 overflow-x-auto border-b border-border px-1"
            role="tablist"
            data-terminal-tabs=""
        >
            {move || {
                let current = selected.get();
                tabs.get()
                    .into_iter()
                    .map(|tab| {
                        let is_selected = current.as_deref() == Some(tab.session_id.as_str());
                        view! { <TerminalTab tab=tab is_selected=is_selected on_select=on_select on_close=on_close /> }
                    })
                    .collect_view()
            }}
            <button
                type="button"
                class="shrink-0 px-2 py-1 text-xs rounded text-text-tertiary hover:text-text-primary"
                title=t_string!(i18n, terminal.new_tab).to_string()
                aria-label=t_string!(i18n, terminal.new_tab).to_string()
                on:click=move |_| on_new.run(())
            >
                "+"
            </button>
        </div>
    }
}

/// One tab: the state glyph, the title, and a close button.
///
/// The glyph comes from `shared_ui_logic` — the same table the agent panel
/// draws — so a blocked agent looks blocked in both places without this file
/// having an opinion about what the symbols are. A tab with no agent row
/// carries NO glyph rather than a placeholder: `state: None` means the sampler
/// said nothing about this session, and any glyph there would be a claim
/// (判据 §8).
#[component]
fn TerminalTab(
    tab: TabEntry,
    is_selected: bool,
    on_select: Callback<String>,
    on_close: Callback<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    let select_id = tab.session_id.clone();
    let close_id = tab.session_id.clone();
    let glyph = tab.state.map(state_glyph);
    let border = if is_selected {
        "border-primary"
    } else {
        "border-transparent"
    };
    // An exited session keeps its tab until the next `pty.list` confirms it
    // gone; it is dimmed so it does not read as a live one.
    let dim = if tab.closed { " opacity-50" } else { "" };

    view! {
        <div class=format!(
            "flex items-center gap-1 px-2 py-1 text-xs rounded-t border-b-2 whitespace-nowrap {border}{dim}"
        )>
            <button
                type="button"
                role="tab"
                aria-selected=is_selected.to_string()
                class="flex items-center gap-1 min-w-0 outline-none"
                title=tab.program.clone().unwrap_or_default()
                on:click=move |_| on_select.run(select_id.clone())
            >
                {glyph.map(|g| view! { <span class="shrink-0 text-text-tertiary">{g}</span> })}
                <span class="truncate max-w-[12rem] text-text-primary">{tab.title.clone()}</span>
            </button>
            <button
                type="button"
                class="shrink-0 px-1 rounded text-text-tertiary hover:text-text-primary"
                title=t_string!(i18n, terminal.close_tab).to_string()
                aria-label=t_string!(i18n, terminal.close_tab).to_string()
                on:click=move |_| on_close.run(close_id.clone())
            >
                "\u{00d7}"
            </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(session_id: &str, shell: &str, closed: bool) -> PtySessionInfo {
        PtySessionInfo {
            session_id: session_id.to_string(),
            shell: shell.to_string(),
            cwd: String::new(),
            created_at: 0,
            closed,
        }
    }

    fn agent(
        session_id: &str,
        state: RuntimeAgentState,
        program: Option<&str>,
    ) -> RuntimeAgentEntry {
        RuntimeAgentEntry {
            session_id: session_id.to_string(),
            label: "zsh".to_string(),
            cwd: String::new(),
            agent: None,
            program: program.map(str::to_string),
            state,
            updated_at: 0,
            quiet_since: None,
        }
    }

    /// The join is BY `session_id`, not by position. The two lists come from
    /// two different server tables with no shared ordering guarantee, so a
    /// positional merge would paint one session's state onto another's tab
    /// the first time they disagreed — and a wrong state glyph reads as a
    /// fact (判据 §17). The fixture deliberately gives the agent list the
    /// OPPOSITE order and one MISSING member, so a positional merge cannot
    /// pass it.
    #[test]
    fn reconcile_joins_agent_state_by_session_id() {
        let mut model = TabModel::default();
        model.reconcile(
            &[
                session("a", "zsh", false),
                session("b", "zsh", false),
                session("c", "zsh", false),
            ],
            &[
                agent("c", RuntimeAgentState::Blocked, Some("claude")),
                agent("a", RuntimeAgentState::Working, Some("codex")),
            ],
        );

        let tabs = model.tabs();
        assert_eq!(
            tabs.iter()
                .map(|t| t.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "tab order comes from the session list, not the agent list"
        );
        assert_eq!(tabs[0].state, Some(RuntimeAgentState::Working));
        assert_eq!(tabs[0].program.as_deref(), Some("codex"));
        assert_eq!(tabs[2].state, Some(RuntimeAgentState::Blocked));
        // No agent row for "b". `None` means "the sampler has nothing to say
        // about this session", which is not the same fact as any of the four
        // states — least of all Idle (判据 §8).
        assert_eq!(tabs[1].state, None);
        assert_eq!(tabs[1].program, None);
    }

    /// A brand-new shell must be reachable the instant it exists. Re-listing
    /// to learn about a session we just created is a round trip that can fail
    /// on its own, and a running shell with no tab pointing at it is D2.
    #[test]
    fn a_spawned_session_gets_a_tab_without_waiting_for_a_list() {
        let mut model = TabModel::default();
        model.adopt_spawned("fresh", "zsh");

        assert_eq!(model.tabs().len(), 1);
        assert_eq!(model.tabs()[0].title, "zsh");
        assert_eq!(model.selected_id(), Some("fresh"));

        // A retry that lands twice must not grow a second tab.
        model.adopt_spawned("fresh", "zsh");
        assert_eq!(model.tabs().len(), 1);
    }

    /// I1's first half. The tab strip's decoration is a JOIN, and a join is
    /// only as fresh as its last run — a session that goes Blocked after the
    /// view mounted has to reach the glyph. Before the fix `reconcile` had one
    /// caller (the mount effect), so the glyph froze at whatever the sampler
    /// happened to be saying at mount and then rendered a state that was no
    /// longer true. A wrong label costs more than a missing one (判据 §17).
    ///
    /// This is the model half; the wire is pinned by
    /// `the_view_reconciles_on_runtime_agents_changed_and_on_exit` in
    /// `views/terminal/mod.rs`, because a model that reconciles correctly on
    /// demand proves nothing about whether anything demands it.
    #[test]
    fn a_state_change_after_the_first_reconcile_reaches_the_tab() {
        let mut model = TabModel::default();
        let sessions = [session("a", "zsh", false)];

        model.reconcile(
            &sessions,
            &[agent("a", RuntimeAgentState::Working, Some("claude"))],
        );
        assert_eq!(model.tabs()[0].state, Some(RuntimeAgentState::Working));
        assert_eq!(model.tabs()[0].program.as_deref(), Some("claude"));

        model.reconcile(
            &sessions,
            &[agent("a", RuntimeAgentState::Blocked, Some("codex"))],
        );
        assert_eq!(
            model.tabs()[0].state,
            Some(RuntimeAgentState::Blocked),
            "a later sample must replace the earlier one, not be ignored"
        );
        assert_eq!(model.tabs()[0].program.as_deref(), Some("codex"));

        // The sampler going silent about a session is "I have nothing to say",
        // and it must not leave the previous state on screen as if it were
        // still being asserted (判据 §8).
        model.reconcile(&sessions, &[]);
        assert_eq!(model.tabs()[0].state, None);
        assert_eq!(model.tabs()[0].program, None);
    }

    /// I1's second half. `on_exit` only MARKS a tab closed; `reconcile` is
    /// what drops it, and `on_exit`'s own doc promised exactly that. Nothing
    /// scheduled a reconcile, so the promise described behaviour that did not
    /// exist and the dimmed dead tab survived for the life of the mount
    /// (判据 §1).
    #[test]
    fn an_exited_tab_is_dropped_by_the_next_reconcile() {
        let mut model = TabModel::default();
        model.reconcile(
            &[session("a", "zsh", false), session("gone", "zsh", false)],
            &[],
        );
        assert!(model.select("gone"));

        model.on_exit("gone");
        assert!(
            model
                .tabs()
                .iter()
                .any(|t| t.session_id == "gone" && t.closed),
            "still listed, marked closed, for the one round trip it takes to confirm"
        );
        assert_eq!(model.selected().map(|t| t.session_id.as_str()), Some("a"));

        // The server no longer lists it — `pty.exit` fires after
        // `manager().remove()`, so the very next list omits it entirely.
        model.reconcile(&[session("a", "zsh", false)], &[]);
        assert!(
            !model.tabs().iter().any(|t| t.session_id == "gone"),
            "the next reconcile drops it, which is what `on_exit`'s doc claims"
        );
        assert_eq!(model.selected().map(|t| t.session_id.as_str()), Some("a"));
    }

    /// A session the server reports as closed is gone; it must not occupy a
    /// tab that clicking would attach to.
    #[test]
    fn reconcile_drops_sessions_the_server_reports_closed() {
        let mut model = TabModel::default();
        model.reconcile(
            &[session("dead", "zsh", true), session("live", "zsh", false)],
            &[],
        );
        assert_eq!(
            model
                .tabs()
                .iter()
                .map(|t| t.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["live"]
        );
    }

    /// `pty.exit` arrives before the next `pty.list`, and the tab that just
    /// died may be the one the user is looking at. Selection must land on a
    /// neighbour rather than on nothing — an empty terminal pane with tabs
    /// still on screen reads as a broken page, and "the session you were in
    /// exited" is not a reason to show no session at all (判据 §14: what does
    /// the person who was blocked do next).
    #[test]
    fn closing_the_selected_tab_falls_to_a_neighbour() {
        let mut model = TabModel::default();
        model.reconcile(
            &[
                session("a", "zsh", false),
                session("b", "zsh", false),
                session("c", "zsh", false),
            ],
            &[],
        );
        assert!(model.select("b"));

        model.on_exit("b");
        assert_eq!(
            model.selected().map(|t| t.session_id.as_str()),
            Some("c"),
            "the next open tab is the neighbour"
        );
        assert!(
            model.tabs().iter().any(|t| t.session_id == "b" && t.closed),
            "the exited tab stays listed, marked closed, until the next \
             reconcile — vanishing mid-keystroke hides WHY it went away"
        );

        // The last open tab: there is no next, so fall back to the previous.
        model.on_exit("c");
        assert_eq!(model.selected().map(|t| t.session_id.as_str()), Some("a"));

        // Nothing left open at all: `None` is the honest answer, not a
        // pointer at a dead session.
        model.on_exit("a");
        assert_eq!(model.selected().map(|t| t.session_id.as_str()), None);
    }

    /// Selecting a session this model has never heard of must be REFUSED,
    /// not silently turned into a tab. The id comes from outside (the agent
    /// panel's row click, and a stale row is exactly the case that matters),
    /// and inventing a tab for it would produce a tab pointing at a session
    /// that may not exist — the page would then attach to nothing and show
    /// an empty screen with no explanation (判据 §8).
    #[test]
    fn select_an_unknown_session_is_refused_not_silently_added() {
        let mut model = TabModel::default();
        model.reconcile(&[session("a", "zsh", false)], &[]);
        assert!(model.select("a"));

        assert!(!model.select("ghost"), "an unknown id must be refused");
        assert_eq!(model.tabs().len(), 1, "and must not create a tab");
        assert_eq!(
            model.selected().map(|t| t.session_id.as_str()),
            Some("a"),
            "a refused select must not move the selection either"
        );
    }

    /// Three sources can name a tab and they are not equally good: the
    /// program's own OSC title is what the user set, the foreground program
    /// is what is actually running, and the shell is only what the session
    /// was STARTED as. Each falls through to the next only when the one
    /// above is absent — an empty string is absent, not a title (判据 §17: a
    /// blank tab is a label that says nothing).
    #[test]
    fn title_prefers_osc_then_program_then_shell() {
        assert_eq!(
            derive_title(Some("build: web"), Some("claude"), "zsh", "s1"),
            "build: web"
        );
        assert_eq!(derive_title(None, Some("claude"), "zsh", "s1"), "claude");
        assert_eq!(derive_title(None, None, "zsh", "s1"), "zsh");
        // Empty is not a value at any level.
        assert_eq!(
            derive_title(Some(""), Some("claude"), "zsh", "s1"),
            "claude"
        );
        assert_eq!(derive_title(Some(""), Some(""), "zsh", "s1"), "zsh");
        // Nothing at all still has to render something clickable.
        assert_eq!(derive_title(None, None, "", "s1"), "s1");
    }

    /// The OSC title arrives on a screen frame, long after the tab was built
    /// from `pty.list`, and a later `pty.list` refresh must not throw it
    /// away — otherwise the tab's name flickers back to the shell every time
    /// an agent starts or stops anywhere on the server.
    #[test]
    fn a_reconcile_keeps_a_title_the_program_already_set() {
        let mut model = TabModel::default();
        model.reconcile(&[session("a", "zsh", false)], &[]);
        model.on_title("a", "build: web");
        assert_eq!(model.tabs()[0].title, "build: web");

        model.reconcile(&[session("a", "zsh", false)], &[]);
        assert_eq!(model.tabs()[0].title, "build: web");
    }

    /// Tabs must keep their positions across refreshes: a session list that
    /// comes back in a different order (or with a new session in it) must
    /// not shuffle the tabs a user is pointing at.
    #[test]
    fn reconcile_keeps_existing_tabs_in_place_and_appends_new_ones() {
        let mut model = TabModel::default();
        model.reconcile(
            &[session("a", "zsh", false), session("b", "zsh", false)],
            &[],
        );
        model.reconcile(
            &[
                session("b", "zsh", false),
                session("c", "zsh", false),
                session("a", "zsh", false),
            ],
            &[],
        );
        assert_eq!(
            model
                .tabs()
                .iter()
                .map(|t| t.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    /// A selection naming a session that is no longer listed cannot survive
    /// a reconcile: it would leave `selected()` answering `None` while
    /// `selected_id()` still named the dead session, and the view attaching
    /// to neither.
    #[test]
    fn a_reconcile_that_drops_the_selected_session_reselects() {
        let mut model = TabModel::default();
        model.reconcile(
            &[session("a", "zsh", false), session("b", "zsh", false)],
            &[],
        );
        assert!(model.select("b"));
        model.reconcile(&[session("a", "zsh", false)], &[]);
        assert_eq!(model.selected().map(|t| t.session_id.as_str()), Some("a"));

        model.reconcile(&[], &[]);
        assert_eq!(model.selected_id(), None);
    }
}
