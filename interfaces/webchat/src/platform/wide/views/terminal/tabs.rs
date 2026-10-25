//! The terminal's tab strip: a pure model, and the component that draws it.
//!
//! Session order follows `pty.list` and remains stable across refreshes.

use aleph_protocol::pty::PtySessionInfo;
use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};

/// One tab: a PTY session and its locally derived title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabEntry {
    pub session_id: String,
    /// What the tab shows. Derived by `derive_title` and never empty.
    pub title: String,
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
/// the OSC title, then the shell the session was started as, then the session
/// id.
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
fn derive_title(osc_title: Option<&str>, shell: &str, session_id: &str) -> String {
    fn present(value: Option<&str>) -> Option<&str> {
        value.filter(|v| !v.is_empty())
    }
    present(osc_title)
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
    /// Refusing rather than creating is the whole point: the id must name an
    /// existing tab, and a tab invented for a session that no longer exists
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

    /// Whether adopting `title` for `session_id` would record anything new —
    /// the rate limiter in front of [`Self::on_title`], which runs on every
    /// applied frame.
    ///
    /// It compares against `osc_title`, the field that HOLDS this fact, and
    /// not against `title`, which is DERIVED. The frame handler used to ask
    /// `t.title != title`, and an OSC title equal to the name the tab already
    /// derived — `claude` setting its title to `claude`, the common case — read
    /// as unchanged. `osc_title` then stayed `None`, and the next
    /// [`Self::reconcile`] re-derived down to the shell, discarding a title
    /// the session really had set (判据 §1: the derived value and the raw fact
    /// derived value and the raw fact are two statements, and only one of them
    /// is what the question is about).
    ///
    /// An unknown id answers `false`: [`Self::on_title`] would ignore it, so
    /// there is nothing to change.
    #[must_use]
    pub fn osc_title_differs(&self, session_id: &str, title: &str) -> bool {
        self.tabs
            .iter()
            .find(|t| t.session_id == session_id)
            .is_some_and(|t| t.osc_title.as_deref() != Some(title))
    }

    /// Adopt an OSC title for one session. Unknown ids are ignored — the
    /// screen topic carries every session's frames, including ones this
    /// model has not listed yet.
    pub fn on_title(&mut self, session_id: &str, title: &str) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.session_id == session_id) {
            tab.osc_title = Some(title.to_string());
            tab.title = derive_title(tab.osc_title.as_deref(), &tab.shell, &tab.session_id);
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

    /// Merge a fresh `pty.list` into the tab strip.
    ///
    /// Order: tabs already on screen keep their positions, then sessions this
    /// model has not seen are appended in the order the server listed them.
    /// Sessions the server reports `closed` are dropped.
    pub fn reconcile(&mut self, sessions: &[PtySessionInfo]) {
        let mut next: Vec<TabEntry> = Vec::with_capacity(sessions.len());
        for tab in &self.tabs {
            if let Some(session) = sessions
                .iter()
                .find(|s| s.session_id == tab.session_id && !s.closed)
            {
                next.push(Self::merged(Some(tab), session));
            }
        }
        for session in sessions.iter().filter(|s| !s.closed) {
            if next.iter().any(|t| t.session_id == session.session_id) {
                continue;
            }
            next.push(Self::merged(None, session));
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
            tab.title = derive_title(tab.osc_title.as_deref(), &tab.shell, &tab.session_id);
        } else {
            self.tabs.push(TabEntry {
                session_id: session_id.to_string(),
                title: derive_title(None, shell, session_id),
                closed: false,
                osc_title: None,
                shell: shell.to_string(),
            });
        }
        self.selected = Some(session_id.to_string());
    }

    /// Build one tab row while retaining the last OSC title.
    fn merged(previous: Option<&TabEntry>, session: &PtySessionInfo) -> TabEntry {
        let osc_title = previous.and_then(|p| p.osc_title.clone());
        let title = derive_title(osc_title.as_deref(), &session.shell, &session.session_id);
        TabEntry {
            session_id: session.session_id.clone(),
            title,
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

/// One tab: the title and a close button.
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
                title=tab.title.clone()
                on:click=move |_| on_select.run(select_id.clone())
            >
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
