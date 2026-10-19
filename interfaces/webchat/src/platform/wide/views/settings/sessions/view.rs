// Main container for the Markdown-backed session browser.

use crate::api::session_management::{
    CompressionStatus, SessionListFilters, SessionMetadata, SessionsApi,
};
use crate::context::DashboardState;
use crate::i18n::use_i18n;
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::detail::SessionDetail;
use super::filters::SessionFilters;
use super::list::SessionList;

#[derive(Debug, Clone, Default)]
pub struct SessionFilterParams {
    pub agent_name: Option<String>,
    pub workspace: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub compression_status: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

#[component]
#[must_use]
pub fn SessionsView() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let sessions = RwSignal::new(Vec::<SessionMetadata>::new());
    let selected_session_id = RwSignal::new(Option::<String>::None);
    let has_more = RwSignal::new(false);
    let current_page = RwSignal::new(0u32);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let filter_params = RwSignal::new(SessionFilterParams {
        limit: 20,
        ..Default::default()
    });

    let fetch_sessions = move || {
        loading.set(true);
        error.set(None);
        let params = filter_params.get();
        let filters = SessionListFilters {
            agent_id: params.agent_name,
            workspace_id: params.workspace,
            start_date: params.start_date.as_deref().and_then(parse_date),
            end_date: params.end_date.as_deref().and_then(parse_date),
            compression_status: params.compression_status.as_deref().and_then(parse_status),
            limit: Some(params.limit as usize),
            offset: Some(params.offset as usize),
        };
        let state = state.clone();
        spawn_local(async move {
            match SessionsApi::list(&state, filters).await {
                Ok(response) => {
                    sessions.set(response.sessions);
                    has_more.set(response.has_more);
                }
                Err(message) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &message,
                        str::to_string,
                    )))
                }
            }
            loading.set(false);
        });
    };

    Effect::new(move || {
        if state.is_connected.get() {
            fetch_sessions();
        }
    });

    let on_filter_apply = move |new_params: SessionFilterParams| {
        current_page.set(0);
        filter_params.set(new_params);
        fetch_sessions();
    };
    let on_page_change = move |page: u32| {
        current_page.set(page);
        filter_params.update(|params| params.offset = page * params.limit);
        fetch_sessions();
    };
    let on_session_select = move |session_id: String| selected_session_id.set(Some(session_id));

    view! {
        <div class="flex flex-col h-full p-6 gap-6">
            <div class="flex flex-col gap-4 pb-4 border-b border-border">
                <h2 class="text-2xl font-bold text-text-primary">"Session History"</h2>
                <SessionFilters on_apply=on_filter_apply current_params=filter_params />
                {move || error.get().map(|message| view! { <p class="text-danger text-sm">{message}</p> })}
            </div>
            <div class="grid grid-cols-[60%_40%] gap-4 flex-1 overflow-hidden">
                <div class="flex flex-col overflow-hidden border border-border rounded-lg bg-surface-raised">
                    <SessionList sessions selected_id=selected_session_id on_select=on_session_select loading has_more current_page on_page_change />
                </div>
                <div class="flex flex-col overflow-hidden border border-border rounded-lg bg-surface-raised">
                    {move || selected_session_id.get().map(|id| view! { <SessionDetail session_id=id /> }.into_any()).unwrap_or_else(|| view! { <div class="flex items-center justify-center h-full"><p class="text-text-tertiary">"Select a session to view details"</p></div> }.into_any())}
                </div>
            </div>
        </div>
    }
}

fn parse_date(value: &str) -> Option<i64> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc().timestamp())
}

fn parse_status(value: &str) -> Option<CompressionStatus> {
    match value {
        "pending" => Some(CompressionStatus::Pending),
        "compressed" => Some(CompressionStatus::Compressed),
        "archived" => Some(CompressionStatus::Archived),
        _ => None,
    }
}
