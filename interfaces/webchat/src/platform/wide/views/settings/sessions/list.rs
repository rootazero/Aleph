//! Session list and pagination components.

use crate::api::session_management::SessionMetadata;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn SessionList(
    sessions: RwSignal<Vec<SessionMetadata>>,
    selected_id: RwSignal<Option<String>>,
    on_select: impl Fn(String) + Send + Sync + 'static + Copy,
    loading: RwSignal<bool>,
    has_more: RwSignal<bool>,
    current_page: RwSignal<u32>,
    on_page_change: impl Fn(u32) + Send + Sync + 'static + Copy,
) -> impl IntoView {
    view! {
        <div class="flex flex-col h-full">
            {move || {
                if loading.get() {
                    view! { <div class="flex items-center justify-center flex-1"><p class="text-text-tertiary">"Loading sessions..."</p></div> }.into_any()
                } else if sessions.get().is_empty() {
                    view! { <div class="flex items-center justify-center flex-1"><p class="text-text-tertiary">"No sessions found"</p></div> }.into_any()
                } else {
                    view! {
                        <div class="flex-1 overflow-y-auto p-3 flex flex-col gap-3">
                            <For
                                each=move || sessions.get()
                                key=|session| session.session_id.clone()
                                children=move |session| {
                                    let session_id = session.session_id.clone();
                                    let selected_session_id = session_id.clone();
                                    view! {
                                        <SessionCard
                                            session=session
                                            is_selected=move || selected_id.get().as_ref() == Some(&selected_session_id)
                                            on_click=move |_| on_select(session_id.clone())
                                        />
                                    }
                                }
                            />
                        </div>
                    }.into_any()
                }
            }}
            <SessionPagination current_page has_more on_page_change />
        </div>
    }
}

#[component]
fn SessionCard(
    session: SessionMetadata,
    is_selected: impl Fn() -> bool + Send + Sync + 'static,
    on_click: impl Fn(leptos::ev::MouseEvent) + Send + Sync + 'static,
) -> impl IntoView {
    let session_id = session.session_id.clone();
    let agent_label = session.agent_id.clone();
    let workspace = session
        .workspace_id
        .clone()
        .unwrap_or_else(|| "—".to_string());
    let started = format_timestamp(session.started_at);
    let ended = session
        .ended_at
        .map(format_timestamp)
        .unwrap_or_else(|| "Active".to_string());
    let status = format!("{:?}", session.compression_status);
    let short_id = if session_id.len() > 12 {
        format!("{}…", &session_id[..12])
    } else {
        session_id.clone()
    };

    view! {
        <button
            type="button"
            class=move || format!(
                "w-full text-left rounded-lg p-4 cursor-pointer transition-all {}",
                if is_selected() { "border-2 border-primary bg-primary/5" } else { "border border-border bg-surface hover:border-primary/50" }
            )
            on:click=on_click
        >
            <div class="flex items-center gap-3 mb-3">
                <div class="w-10 h-10 rounded-full bg-primary text-white flex items-center justify-center text-lg font-semibold flex-shrink-0">
                    {agent_label.chars().next().unwrap_or('A').to_string()}
                </div>
                <div class="flex-1 min-w-0">
                    <div class="font-semibold text-text-primary">{agent_label}</div>
                    <div class="text-xs text-text-tertiary font-mono" title=session_id>{short_id}</div>
                </div>
                <div class="px-2 py-1 rounded bg-surface-sunken text-xs font-medium text-text-secondary">{status}</div>
            </div>
            <div class="flex flex-col gap-1 mb-3 text-sm">
                <div class="flex justify-between"><span class="text-text-tertiary">"Started:"</span><span class="text-text-primary font-medium">{started}</span></div>
                <div class="flex justify-between"><span class="text-text-tertiary">"Ended:"</span><span class="text-text-primary font-medium">{ended}</span></div>
                <div class="flex justify-between"><span class="text-text-tertiary">"Workspace:"</span><span class="text-text-primary font-medium">{workspace}</span></div>
            </div>
            <div class="flex gap-4 pt-3 border-t border-border">
                <div class="flex flex-col items-center flex-1"><span class="text-xl font-semibold text-text-primary">{session.message_count}</span><span class="text-xs text-text-tertiary uppercase">"msgs"</span></div>
                <div class="flex flex-col items-center flex-1"><span class="text-xl font-semibold text-text-primary">{session.tool_call_count}</span><span class="text-xs text-text-tertiary uppercase">"tools"</span></div>
            </div>
        </button>
    }
}

#[component]
fn SessionPagination(
    current_page: RwSignal<u32>,
    has_more: RwSignal<bool>,
    on_page_change: impl Fn(u32) + Send + Sync + 'static + Copy,
) -> impl IntoView {
    let go_prev = move |_| {
        let page = current_page.get();
        if page > 0 {
            on_page_change(page - 1);
        }
    };
    let go_next = move |_| {
        if has_more.get() {
            on_page_change(current_page.get() + 1);
        }
    };
    view! {
        <div class="flex justify-between items-center p-4 border-t border-border bg-surface-sunken">
            <button type="button" class="px-4 py-2 border border-border rounded bg-surface text-text-primary text-sm disabled:opacity-50" on:click=go_prev disabled=move || current_page.get() == 0>"← Previous"</button>
            <span class="text-sm text-text-secondary font-medium">"Page " {move || current_page.get() + 1}</span>
            <button type="button" class="px-4 py-2 border border-border rounded bg-surface text-text-primary text-sm disabled:opacity-50" on:click=go_next disabled=move || !has_more.get()>"Next →"</button>
        </div>
    }
}

fn format_timestamp(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}
