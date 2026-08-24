//! Project room page (P2 Task 8) — the main-area content for `/projects`.
//! Mirrors `views::teams::{TeamsTabState, TeamsView}`: shared tab state is
//! lifted to the app root (`app.rs`) and rendered by both the sidebar
//! (`components::sidebar::projects::ProjectsSidebar`) and this view.
//!
//! With no project selected it renders an empty-state placeholder. With one
//! selected it renders the room page: header (name + owner/member badge), a
//! tab strip (群聊 default / 设置 / 看板 live; 工作区浏览·记忆浏览 still P3
//! placeholders),
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
//! ## Room chat = ONE conversation per project, shared by every member
//! Does NOT mount a second `<ChatView />` — see that component's doc for why
//! (every mount independently subscribes/unsubscribes the `stream.*` /
//! `team.*` Gateway topics, so a second instance unmounting would kill the
//! always-mounted `/` instance's event stream too). Instead this reuses
//! `ChatView`'s own building blocks (`MessageList` + `InputArea`, both
//! `pub(crate)`) directly, and drives the same `SessionMap` machinery
//! `ChatSidebar::on_select_session` uses to reopen a past session: find (or
//! open) a dedicated `ConvId`, `activate()` it into the singleton
//! `ChatState`, and mark it via `ChatState::room_project_id`.
//!
//! The `project_id -> session_key` mapping is **server-side**
//! (`projects.room_session`, persisted on the `projects` row). It used to live
//! in this device's `localStorage`, which cannot express a fact shared between
//! devices: the second member to enter a room found nothing there, opened a
//! brand-new session, and the two never saw each other's messages on any
//! surface.

mod kanban;
mod settings;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::agents::AgentsApi;
use crate::api::projects::{ProjectInfo, ProjectsApi};
use crate::components::chat_sidebar::hydrate_and_follow;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, I18nCtx};
use crate::platform::wide::views::chat::composer::InputArea;
use crate::platform::wide::views::chat::messages::MessageList;
use crate::platform::wide::views::chat::state::ChatState;
use crate::state::layout::WorkspaceState;
use crate::state::sessions::SessionMap;
use crate::state::user_directory::UserDirectoryState;
use kanban::KanbanTab;
use settings::SettingsTab;

/// Sub-tab inside a project room page. `Kanban` is live (P3 Task 8);
/// `Workspace` / `Memory` are still rendered as bare placeholders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomSubTab {
    Chat,
    Settings,
    Kanban,
    Workspace,
    Memory,
}

impl RoomSubTab {
    /// The tab strip's label. Lives on the variant rather than at the call
    /// site so a tab cannot be rendered under one name here and another name
    /// anywhere else — and so adding a sub-tab has to answer "what is it
    /// called" in both languages before it compiles.
    fn label(self, i18n: I18nCtx) -> String {
        match self {
            Self::Chat => t_string!(i18n, project_room.tab_chat).to_string(),
            Self::Settings => t_string!(i18n, project_room.tab_settings).to_string(),
            Self::Kanban => t_string!(i18n, project_room.tab_kanban).to_string(),
            Self::Workspace => t_string!(i18n, project_room.tab_workspace).to_string(),
            Self::Memory => t_string!(i18n, project_room.tab_memory).to_string(),
        }
    }
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
    let i18n = use_i18n();
    view! {
        <div class="flex-1 flex items-center justify-center text-text-tertiary text-sm">
            {t!(i18n, project_room.empty_state)}
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
    let i18n = crate::i18n::use_i18n();
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
                    Err(e) => load_error.set(Some(
                        crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    )),
                }
            });
        })
    };
    Effect::new(move |_| refresh.run(()));

    // `projects.changed` push topic (Task 6): a rename / archive /
    // bind_workspace / roster change from another surface, or another
    // member, refreshes this room's header AND settings tab — both read
    // the same `project` signal `refresh` sets, `member_ids` included, so
    // one re-fetch covers both. Filtered to THIS room: the event face is
    // roster-gated (`event_visibility::ByProjectScope`), so every frame
    // this client receives is one it may see, but a sibling room's rename
    // must not re-render a page that is not showing it.
    {
        let filter_id = project_id.clone();
        let subscription_id = dash.subscribe_events(move |event| {
            if event.topic != "projects.changed" {
                return;
            }
            if event.data.get("project_id").and_then(|v| v.as_str()) != Some(filter_id.as_str()) {
                return;
            }
            refresh.run(());
        });
        spawn_local(async move {
            if let Err(e) = dash.subscribe_topic("projects.changed").await {
                web_sys::console::error_1(
                    &format!("Failed to subscribe to projects.changed: {e}").into(),
                );
            }
        });
        on_cleanup(move || {
            dash.unsubscribe_events(subscription_id);
        });
    }

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
                        RoomSubTab::Kanban => view! {
                            <KanbanTab project=p.clone() />
                        }.into_any(),
                        RoomSubTab::Workspace | RoomSubTab::Memory => {
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
    let i18n = use_i18n();
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
                    {move || {
                        if is_owner() {
                            t_string!(i18n, project_room.owner).to_string()
                        } else {
                            t_string!(i18n, project_room.member).to_string()
                        }
                    }}
                </span>
            </div>
            <nav class="mt-3 flex items-center gap-1 -mb-px">
                <RoomTabButton current=sub_tab target=RoomSubTab::Chat />
                <RoomTabButton current=sub_tab target=RoomSubTab::Settings />
                <RoomTabButton current=sub_tab target=RoomSubTab::Kanban />
                <RoomTabButton current=sub_tab target=RoomSubTab::Workspace />
                <RoomTabButton current=sub_tab target=RoomSubTab::Memory />
            </nav>
        </div>
    }
}

#[component]
fn RoomTabButton(current: RwSignal<RoomSubTab>, target: RoomSubTab) -> impl IntoView {
    let i18n = use_i18n();
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
            {move || target.label(i18n)}
        </button>
    }
}

#[component]
fn PlaceholderTab() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="px-6 py-10 text-center text-sm text-text-tertiary">
            {t!(i18n, project_room.coming_soon)}
        </div>
    }
}

/// The room's live chat surface: resolves the room's server-side session,
/// activates it as the current conversation, then renders the same
/// `MessageList` + `InputArea` the single-agent `ChatView` uses — see this
/// module's doc for why `ChatView` itself is not mounted a second time.
/// Bind the current conversation to a project room: resolve the room's shared
/// session, activate (or open) its tab, and mark the conversation as belonging
/// to that room.
///
/// Public because there are **two** ways into a room and they must agree. This
/// page's effect is one; the composer's 「进入项目工作」 pill is the other, and
/// its recents list is the registry of rooms. That pill used to call
/// `ChatState::set_active_project` with the room's folder instead, which sends
/// the path as a per-turn `project_root` — a config-tier capability. For an
/// operator that merely opened a private conversation inside a shared room's
/// directory; for a member it was refused outright, so every entry in a list
/// built from the rooms they belong to was a dead button. Entering by
/// `project_id` is the tier-2 path: the directory was chosen for them by the
/// owner, and using it is not choosing it.
///
/// Callers own their own re-entrancy guard — this function has no latch, and
/// two concurrent calls would open two conversations for one room.
pub async fn enter_project_room(
    dash: DashboardState,
    chat: ChatState,
    session_map: SessionMap,
    workspace: Option<WorkspaceState>,
    project_id: &str,
    project_name: String,
    locale: crate::i18n::Locale,
) {
    let agent_id = match chat.agent_id.get_untracked() {
        Some(a) => a,
        None => AgentsApi::list(&dash)
            .await
            .map(|r| r.default_id)
            .unwrap_or_default(),
    };
    // The room's canonical session, shared with every other member. `agent_id`
    // is only this Panel's proposal — a room somebody already opened answers
    // with the key it has.
    let Ok(key) = ProjectsApi::room_session(&dash, project_id, &agent_id).await else {
        // A room we cannot resolve a session for is one we cannot chat in.
        // Leave the current conversation alone rather than activating an empty
        // tab that sends nowhere.
        return;
    };
    // Reuse-or-open + activate + register, through the one writer every chat
    // surface shares. The registration half matters more here than anywhere
    // else: a room's canonical session belongs to every member, so a run
    // started by a peer is the NORMAL case — without the identity
    // `adopt_session` stamps, `conv_for_session_key` cannot find the tab and
    // the peer's turn has nowhere to render.
    session_map.adopt_session(chat, &agent_id, &key, || project_name);
    chat.clear_team_context();
    chat.room_project_id.set(Some(project_id.to_string()));
    chat.session_key.set(Some(key.clone()));

    // Mirror `ChatSidebar::on_select_session`: only hydrate when there is
    // nothing live to preserve — a conversation already open (background
    // `ChatState`) is at least as fresh as `chat.history`.
    if chat.messages.with_untracked(Vec::is_empty) {
        // `hydrate_and_follow`, not the bare hydrate: a room's session is
        // shared, so "somebody else is mid-turn on it right now" is the normal
        // state of a room you have just walked into.
        spawn_local(hydrate_and_follow(
            dash,
            chat,
            workspace,
            session_map,
            key,
            locale,
        ));
    }
    if let Some(ws) = workspace {
        ws.reset();
    }
}

#[component]
fn RoomChat(project: ProjectInfo) -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let session_map = expect_context::<SessionMap>();
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();
    let location = leptos_router::hooks::use_location();
    // One open at a time. The effect's body is async, so without this a second
    // trigger arriving mid-fetch would open a second conversation for the same
    // room and leave the first orphaned in the tab strip.
    let opening = StoredValue::new(false);

    // Re-open the room's conversation whenever it is not the active one and
    // this page is the surface the user is actually looking at.
    //
    // Both tracked reads earn their place. `session_map.active` is what makes
    // it re-run: `MainContent` only toggles CSS `display` (`app.rs`), so
    // `RoomChat` is never unmounted — leaving for `/chat`, selecting another
    // conversation and coming back used to strand this tab rendering (and
    // sending into) that other conversation. The path read is what stops the
    // fix from becoming a bug of its own: without it, selecting a conversation
    // on the `/chat` page would make this still-mounted effect immediately
    // steal it back.
    Effect::new({
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        move |_| {
            let _ = session_map.active.get();
            let on_this_page =
                crate::components::mode_sidebar::PanelMode::from_path(&location.pathname.get())
                    == crate::components::mode_sidebar::PanelMode::Projects;
            if !on_this_page {
                return;
            }
            if chat.room_project_id.get_untracked().as_deref() == Some(project_id.as_str()) {
                // Already the active room — do not reopen/rehydrate.
                return;
            }
            if opening.get_value() {
                return;
            }
            opening.set_value(true);
            let project_id = project_id.clone();
            let project_name = project_name.clone();
            let locale = i18n.get_locale_untracked();
            spawn_local(async move {
                enter_project_room(
                    dash,
                    chat,
                    session_map,
                    workspace,
                    &project_id,
                    project_name,
                    locale,
                )
                .await;
                opening.set_value(false);
            });
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
