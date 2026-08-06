//! Project room page (P2 Task 8) — the main-area content for `/projects`.
//! Mirrors `views::teams::{TeamsTabState, TeamsView}`: shared tab state is
//! lifted to the app root (`app.rs`) and rendered by both the sidebar
//! (`components::sidebar::projects::ProjectsSidebar`) and this view.
//!
//! With no project selected it renders an empty-state placeholder. With one
//! selected it renders the room page: header (name + owner/member badge), a
//! tab strip (群聊 default / 设置 / 看板·工作区浏览·记忆浏览 P3 placeholders),
//! and the active tab's body.
//!
//! ## i18n
//! This surface ships with plain Chinese literals rather than new
//! `t!`/`t_string!` locale keys — sanctioned for this task (the surrounding
//! shell is already per-surface-bilingual; `PanelMode::More`'s hardcoded
//! `"More"` in `nav_menu.rs` is existing precedent for the same tradeoff),
//! and keeps this already-large task from also having to learn and edit the
//! locale YAML pipeline. A follow-up can port these strings the same way any
//! other single-language surface would be localized.
//!
//! ## Room chat = a dedicated conversation per project
//! Does NOT mount a second `<ChatView />` — see that component's doc for why
//! (every mount independently subscribes/unsubscribes the `stream.*` /
//! `team.*` Gateway topics, so a second instance unmounting would kill the
//! always-mounted `/` instance's event stream too). Instead this reuses
//! `ChatView`'s own building blocks (`MessageList` + `InputArea`, both
//! `pub(crate)`) directly, and drives the same `SessionMap` machinery
//! `ChatSidebar::on_select_session` uses to reopen a past session: find (or
//! open) a dedicated `ConvId`, `activate()` it into the singleton
//! `ChatState`, and mark it via `ChatState::room_project_id`. The
//! `project_id -> session_key` mapping is remembered in `localStorage`
//! (`session_key_storage`) since the server has no `projects.get`-reachable
//! "this room's session" field.

mod session_key_storage;
mod settings;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::agents::AgentsApi;
use crate::api::projects::{ProjectInfo, ProjectsApi};
use crate::components::chat_sidebar::hydrate_session_history;
use crate::context::DashboardState;
use crate::i18n::use_i18n;
use crate::platform::wide::views::chat::composer::InputArea;
use crate::platform::wide::views::chat::messages::MessageList;
use crate::platform::wide::views::chat::state::ChatState;
use crate::state::layout::WorkspaceState;
use crate::state::sessions::SessionMap;
use crate::state::user_directory::UserDirectoryState;
use settings::SettingsTab;

/// Sub-tab inside a project room page. `Kanban` / `Workspace` / `Memory` are
/// P3 (spec §6.4) — rendered as bare placeholders here, no content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomSubTab {
    Chat,
    Settings,
    Kanban,
    Workspace,
    Memory,
}

/// Shared signals for the Projects mode — mirrors `views::teams::TeamsTabState`.
///
/// `selected_project_id` is the GLOBAL "which project page is showing"
/// state, deliberately NOT per-conversation: it must survive the user
/// switching chat tabs (there is no chat tab open at all until a room's
/// chat sub-tab activates one). The room CONVERSATION's own project binding
/// is the opposite and lives on `ChatState::room_project_id` /
/// `SessionSnapshot` — see that field's doc for the landmine this splits.
#[derive(Clone, Copy)]
pub struct ProjectsTabState {
    pub selected_project_id: RwSignal<Option<String>>,
}

impl Default for ProjectsTabState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectsTabState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            selected_project_id: RwSignal::new(None),
        }
    }
}

#[component]
#[must_use]
pub fn ProjectsView() -> impl IntoView {
    let tab_state = expect_context::<ProjectsTabState>();
    view! {
        <div class="flex-1 flex flex-col h-full overflow-hidden">
            {move || match tab_state.selected_project_id.get() {
                Some(id) => view! { <ProjectRoomPage project_id=id /> }.into_any(),
                None => view! { <EmptyState /> }.into_any(),
            }}
        </div>
    }
}

#[component]
fn EmptyState() -> impl IntoView {
    view! {
        <div class="flex-1 flex items-center justify-center text-text-tertiary text-sm">
            "从左侧选择一个项目，或创建一个新项目"
        </div>
    }
}

/// One project's page: fetches the room, then renders the header + tab
/// strip + active tab body. Keyed by `project_id` through `ProjectsView`'s
/// `match` (a full remount on project switch — same mechanism
/// `views::teams::TeamsView`'s sub-tab `match` uses), so every signal here
/// is scoped to exactly one project's lifetime.
#[component]
fn ProjectRoomPage(project_id: String) -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let project: RwSignal<Option<ProjectInfo>> = RwSignal::new(None);
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let sub_tab = RwSignal::new(RoomSubTab::Chat);

    // Fix round 1 (Important #1): the room's default sub-tab is Chat, and
    // neither `RoomHeader`'s owner badge nor `MessageBubble`'s author label
    // used to trigger this fetch — only `SettingsTab` did. Until a user
    // happened to open Settings first, `UserDirectoryState.my_user_id` was
    // `None`, which made every "is this the viewer's own id" comparison
    // vacuously false: the owner badge always read "成员", and — worse —
    // `author_label`'s self-suppression never fired, so the viewer's own
    // messages picked up a (wrong, raw-id) label too. `ensure_loaded` is
    // idempotent (guards on `loading` + "already populated"), so calling it
    // here in addition to `SettingsTab`'s own call is harmless.
    let dir = expect_context::<UserDirectoryState>();
    dir.ensure_loaded(dash);

    let refresh = {
        let project_id = project_id.clone();
        Callback::new(move |()| {
            let project_id = project_id.clone();
            spawn_local(async move {
                match ProjectsApi::get(&dash, &project_id).await {
                    Ok(p) => {
                        project.set(Some(p));
                        load_error.set(None);
                    }
                    Err(e) => load_error.set(Some(e)),
                }
            });
        })
    };
    Effect::new(move |_| refresh.run(()));

    view! {
        <div class="flex-1 flex flex-col h-full overflow-hidden">
            <Show when=move || load_error.get().is_some()>
                <div class="px-6 py-3 text-sm text-danger">
                    {move || load_error.get().unwrap_or_default()}
                </div>
            </Show>
            {move || project.get().map(|p| view! {
                <RoomHeader project=p.clone() sub_tab=sub_tab />
                <div class="flex-1 min-h-0 overflow-y-auto">
                    {move || match sub_tab.get() {
                        RoomSubTab::Chat => view! { <RoomChat project=p.clone() /> }.into_any(),
                        RoomSubTab::Settings => view! {
                            <SettingsTab project=p.clone() refresh=refresh />
                        }.into_any(),
                        RoomSubTab::Kanban | RoomSubTab::Workspace | RoomSubTab::Memory => {
                            view! { <PlaceholderTab /> }.into_any()
                        }
                    }}
                </div>
            })}
        </div>
    }
}

#[component]
fn RoomHeader(project: ProjectInfo, sub_tab: RwSignal<RoomSubTab>) -> impl IntoView {
    // Owner/member badge — a lighter-weight echo of the settings tab's own
    // "所有者" row, so the viewer's standing in the room is visible without
    // switching tabs. `use_context` (not `expect_context`): harmless to omit
    // if `UserDirectoryState` somehow isn't provided.
    let is_owner = {
        let owner = project.owner_user_id.clone();
        move || {
            use_context::<UserDirectoryState>()
                .is_some_and(|dir| dir.my_user_id.get().is_some() && dir.my_user_id.get() == owner)
        }
    };
    view! {
        <div class="px-6 pt-5 pb-2 flex-shrink-0 border-b border-border-subtle">
            <div class="flex items-center gap-2">
                <h2 class="text-lg font-semibold text-text-primary truncate">{project.name.clone()}</h2>
                <span class="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-surface-sunken text-text-tertiary">
                    {move || if is_owner() { "所有者" } else { "成员" }}
                </span>
            </div>
            <nav class="mt-3 flex items-center gap-1 -mb-px">
                <RoomTabButton label="群聊" current=sub_tab target=RoomSubTab::Chat />
                <RoomTabButton label="设置" current=sub_tab target=RoomSubTab::Settings />
                <RoomTabButton label="看板" current=sub_tab target=RoomSubTab::Kanban />
                <RoomTabButton label="工作区浏览" current=sub_tab target=RoomSubTab::Workspace />
                <RoomTabButton label="记忆浏览" current=sub_tab target=RoomSubTab::Memory />
            </nav>
        </div>
    }
}

#[component]
fn RoomTabButton(
    label: &'static str,
    current: RwSignal<RoomSubTab>,
    target: RoomSubTab,
) -> impl IntoView {
    let is_active = move || current.get() == target;
    view! {
        <button
            type="button"
            on:click=move |_| current.set(target)
            class=move || {
                if is_active() {
                    "px-3 py-2 text-sm font-medium text-primary border-b-2 border-primary"
                } else {
                    "px-3 py-2 text-sm text-text-tertiary hover:text-text-primary border-b-2 border-transparent"
                }
            }
        >
            {label}
        </button>
    }
}

#[component]
fn PlaceholderTab() -> impl IntoView {
    view! {
        <div class="px-6 py-10 text-center text-sm text-text-tertiary">
            "即将推出"
        </div>
    }
}

/// The room's live chat surface. Opens (or reopens) this project's dedicated
/// conversation exactly once per mount, then renders the same `MessageList`
/// + `InputArea` the single-agent `ChatView` uses — see this module's doc
/// for why `ChatView` itself is not mounted a second time.
#[component]
fn RoomChat(project: ProjectInfo) -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let session_map = expect_context::<SessionMap>();
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();

    // Open (or reopen) the room's conversation exactly once when this tab
    // mounts. Every read below is `_untracked`, so the effect fires once and
    // never again for this component's lifetime — a project switch fully
    // remounts `RoomChat` (see `ProjectRoomPage`'s doc), which is the only
    // time this needs to re-run.
    Effect::new({
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        move |_| {
            if chat.room_project_id.get_untracked().as_deref() == Some(project_id.as_str()) {
                // Already the active room (e.g. a re-render that didn't
                // actually remount) — do not reopen/rehydrate.
                return;
            }
            let project_id = project_id.clone();
            let project_name = project_name.clone();
            let locale = i18n.get_locale_untracked();
            spawn_local(async move {
                let agent_id = match chat.agent_id.get_untracked() {
                    Some(a) => a,
                    None => AgentsApi::list(&dash)
                        .await
                        .map(|r| r.default_id)
                        .unwrap_or_default(),
                };
                if let Some(key) = session_key_storage::load(&project_id) {
                    let conv = session_map.conv_for_session_key(&key).unwrap_or_else(|| {
                        session_map.open_conversation(&agent_id, project_name.clone())
                    });
                    session_map.activate(chat, conv);
                    chat.clear_team_context();
                    chat.room_project_id.set(Some(project_id.clone()));
                    chat.session_key.set(Some(key.clone()));
                    // Mirror `ChatSidebar::on_select_session`: only hydrate
                    // when there is nothing live to preserve — a
                    // conversation already open (background `ChatState`)
                    // is at least as fresh as `chat.history`.
                    if chat.messages.with_untracked(Vec::is_empty) {
                        spawn_local(hydrate_session_history(dash, chat, workspace, key, locale));
                    }
                } else {
                    let conv = session_map.open_conversation(&agent_id, project_name);
                    session_map.activate(chat, conv);
                    chat.clear_session();
                    chat.clear_team_context();
                    chat.room_project_id.set(Some(project_id));
                }
                if let Some(ws) = workspace {
                    ws.reset();
                }
            });
        }
    });

    // Persist the session_key the moment this room's conversation gets one
    // (its first `chat.send` response) — idempotent, safe to re-run on every
    // change while this room stays the active conversation.
    Effect::new({
        let project_id = project.id.clone();
        move |_| {
            if chat.room_project_id.get().as_deref() != Some(project_id.as_str()) {
                return;
            }
            if let Some(sk) = chat.session_key.get() {
                session_key_storage::store(&project_id, &sk);
            }
        }
    });

    view! {
        <div class="relative flex flex-col h-full">
            <div class="relative flex-1 min-h-0">
                <MessageList />
                <InputArea />
            </div>
        </div>
    }
}
