// apps/panel/src/components/top_bar.rs
//
// Top bar — logo, workspace selector, contextual actions (new chat button in Chat mode).
//
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use super::bottom_bar::PanelMode;
use super::theme_toggle::ThemeToggle;
use crate::context::DashboardState;
use crate::views::chat::state::ChatState;
use crate::api::chat::ChatApi;
use crate::api::{WorkspaceApi, WorkspaceEntry};

#[component]
pub fn TopBar() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    let on_new_chat = move |_| {
        let dashboard = expect_context::<DashboardState>();
        let chat = expect_context::<ChatState>();

        // Clear local state immediately
        let old_session_key = chat.session_key.get_untracked();
        chat.clear();

        // Clear server-side session if one existed
        if let Some(sk) = old_session_key {
            leptos::task::spawn_local(async move {
                let _ = ChatApi::clear(&dashboard, &sk).await;
            });
        }
    };

    // Workspace state
    let workspaces = RwSignal::new(Vec::<WorkspaceEntry>::new());
    let active_workspace_id = RwSignal::new(String::new());
    let dropdown_open = RwSignal::new(false);
    let is_switching = RwSignal::new(false);

    // Fetch workspaces and active workspace on mount when connected
    let dash = dashboard;
    Effect::new(move || {
        if dash.is_connected.get() {
            let dash = dash;
            leptos::task::spawn_local(async move {
                // Fetch workspace list
                match WorkspaceApi::list(&dash).await {
                    Ok(list) => {
                        workspaces.set(list);
                    }
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to list workspaces: {e}").into(),
                        );
                    }
                }

                // Fetch active workspace
                match WorkspaceApi::get_active(&dash).await {
                    Ok(info) => {
                        active_workspace_id.set(info.workspace_id);
                    }
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to get active workspace: {e}").into(),
                        );
                    }
                }
            });
        }
    });

    // Derive active workspace name from id
    let active_workspace_name = Memo::new(move |_| {
        let id = active_workspace_id.get();
        let list = workspaces.get();
        list.iter()
            .find(|w| w.id == id)
            .map(|w| w.name.clone())
            .unwrap_or_else(|| {
                if id.is_empty() {
                    "default".to_string()
                } else {
                    id.clone()
                }
            })
    });

    // Handle workspace switch
    let on_switch_workspace = move |ws_id: String| {
        dropdown_open.set(false);

        // Skip if already active
        if ws_id == active_workspace_id.get_untracked() {
            return;
        }

        is_switching.set(true);
        let dash = dashboard;
        leptos::task::spawn_local(async move {
            match WorkspaceApi::switch(&dash, &ws_id).await {
                Ok(()) => {
                    active_workspace_id.set(ws_id);
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to switch workspace: {e}").into(),
                    );
                }
            }
            is_switching.set(false);
        });
    };

    // Close dropdown when clicking outside
    let on_backdrop_click = move |_| {
        dropdown_open.set(false);
    };

    view! {
        <header class="h-12 border-b border-border bg-sidebar flex items-center justify-between px-4 flex-shrink-0">
            // Left: Logo + Workspace selector
            <div class="flex items-center gap-3">
                <div class="w-7 h-7 bg-primary rounded-lg flex items-center justify-center">
                    <span class="text-text-inverse font-bold text-base">"A"</span>
                </div>
                <h1 class="text-sm font-semibold tracking-tight">"Aleph"</h1>

                // Workspace selector (only show when we have workspaces)
                <Show when=move || !workspaces.get().is_empty()>
                    <div class="relative">
                        // Separator
                        <span class="text-text-tertiary text-xs mx-1">"/"</span>

                        // Current workspace button
                        <button
                            class="inline-flex items-center gap-1 px-2 py-1 rounded-md text-xs font-medium
                                   text-text-secondary hover:text-text-primary hover:bg-surface-sunken
                                   transition-colors"
                            on:click=move |_| dropdown_open.update(|v| *v = !*v)
                            disabled=move || is_switching.get()
                        >
                            <span>{move || active_workspace_name.get()}</span>
                            // Chevron down icon
                            <svg
                                width="12" height="12" viewBox="0 0 24 24" fill="none"
                                stroke="currentColor" stroke-width="2"
                                stroke-linecap="round" stroke-linejoin="round"
                                class="opacity-50"
                            >
                                <polyline points="6 9 12 15 18 9" />
                            </svg>
                        </button>

                        // Dropdown menu
                        <Show when=move || dropdown_open.get()>
                            // Invisible backdrop to catch outside clicks
                            <div
                                class="fixed inset-0 z-40"
                                on:click=on_backdrop_click
                            />

                            <div class="absolute left-0 top-full mt-1 z-50 min-w-48 py-1
                                        bg-surface-raised border border-border rounded-lg shadow-lg">
                                <div class="px-3 py-1.5 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                                    "Workspaces"
                                </div>
                                <For
                                    each=move || workspaces.get()
                                    key=|ws| ws.id.clone()
                                    children=move |ws: WorkspaceEntry| {
                                        let ws_id = ws.id.clone();
                                        let ws_id_for_click = ws.id.clone();
                                        let ws_name = ws.name.clone();
                                        let ws_desc = ws.description.clone();
                                        let ws_icon = ws.icon.clone();
                                        let is_active = Memo::new(move |_| active_workspace_id.get() == ws_id);
                                        let on_switch = on_switch_workspace.clone();
                                        view! {
                                            <button
                                                class="w-full flex items-center gap-2 px-3 py-2 text-left text-xs
                                                       hover:bg-surface-sunken transition-colors"
                                                class:bg-surface-sunken=move || is_active.get()
                                                on:click=move |_| {
                                                    let id = ws_id_for_click.clone();
                                                    on_switch(id);
                                                }
                                            >
                                                // Workspace icon or fallback
                                                <span class="w-5 h-5 rounded flex items-center justify-center
                                                             bg-surface-sunken text-text-tertiary text-xs flex-shrink-0">
                                                    {ws_icon.unwrap_or_default()}
                                                </span>
                                                <div class="flex-1 min-w-0">
                                                    <div class="font-medium text-text-primary truncate">
                                                        {ws_name}
                                                    </div>
                                                    {ws_desc.map(|d| view! {
                                                        <div class="text-text-tertiary truncate">{d}</div>
                                                    })}
                                                </div>
                                                // Active indicator
                                                <Show when=move || is_active.get()>
                                                    <svg
                                                        width="14" height="14" viewBox="0 0 24 24" fill="none"
                                                        stroke="currentColor" stroke-width="2.5"
                                                        stroke-linecap="round" stroke-linejoin="round"
                                                        class="text-primary flex-shrink-0"
                                                    >
                                                        <polyline points="20 6 9 17 4 12" />
                                                    </svg>
                                                </Show>
                                            </button>
                                        }
                                    }
                                />
                            </div>
                        </Show>
                    </div>
                </Show>
            </div>

            // Right: contextual actions + theme
            <div class="flex items-center gap-2">
                <Show when=move || mode.get() == PanelMode::Chat>
                    <button
                        class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
                               text-text-secondary hover:text-text-primary hover:bg-surface-sunken transition-colors"
                        on:click=on_new_chat
                    >
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <line x1="12" y1="5" x2="12" y2="19" />
                            <line x1="5" y1="12" x2="19" y2="12" />
                        </svg>
                        "New Chat"
                    </button>
                </Show>
                <ThemeToggle />
            </div>
        </header>
    }
}
