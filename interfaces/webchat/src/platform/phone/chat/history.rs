//! Phone Chat history — the session list, reached via the chat surface's
//! history button. Tapping a row loads that session into ChatState and returns
//! to the chat surface (`/`).

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::components::chat_sidebar::hydrate_and_follow;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::state::layout::WorkspaceState;
use crate::state::sessions::SessionMap;
use crate::views::chat::ChatState;

/// One row of `sessions.list` — the same decoder the wide sidebar uses.
///
/// It used to be a second, narrower copy of that shape, and the narrowing was
/// the bug: it modelled `project_root` and none of the dials, so `on_select`
/// below restored the folder and left the tier / mode pills claiming "follow
/// global" for a session the server was governing with stored values. Team
/// chats never appear here — the server filters out `task`/`ephemeral` session
/// types.
pub(crate) use crate::api::sessions::SessionRow;

/// Sort newest-first by `updated_at`; rows with no timestamp sink to the bottom.
pub(crate) fn sort_sessions_desc(mut rows: Vec<SessionRow>) -> Vec<SessionRow> {
    rows.sort_by(|a, b| {
        b.updated_at
            .unwrap_or(i64::MIN)
            .cmp(&a.updated_at.unwrap_or(i64::MIN))
    });
    rows
}

/// Phone Chat history: the session list. Tapping a row loads that session into
/// the shared ChatState and returns to the chat surface (`/`).
#[component]
#[must_use]
pub fn PhoneChatHistory() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let workspace = expect_context::<WorkspaceState>();
    let sessions = expect_context::<SessionMap>();

    // Carried into the history replay so trace-derived narration (MoA turn
    // trace, compaction / veto notes) is localised off the component tree.
    let i18n = crate::i18n::use_i18n();
    let navigate = use_navigate();

    // loading | loaded(rows) | error(msg)
    let rows = RwSignal::new(Vec::<SessionRow>::new());
    let loading = RwSignal::new(true);
    let load_error = RwSignal::new(Option::<String>::None);

    // Fetch sessions.list. Reused by the connect-gated Effect and the retry
    // affordance; captures only Copy handles, so `load` is itself Copy.
    let load = move || {
        loading.set(true);
        load_error.set(None);
        let dash = dashboard;
        spawn_local(async move {
            match dash.rpc_call("sessions.list", serde_json::json!({})).await {
                Ok(result) => {
                    let parsed = result
                        .get("sessions")
                        .cloned()
                        .and_then(|v| serde_json::from_value::<Vec<SessionRow>>(v).ok())
                        .unwrap_or_default();
                    rows.set(sort_sessions_desc(parsed));
                    loading.set(false);
                }
                Err(e) => {
                    load_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                    loading.set(false);
                }
            }
        });
    };

    // Fetch once the socket is connected (mirrors ChatSidebar at
    // chat_sidebar.rs:320-324): rpc_call returns "Not connected" until the WS
    // handshake completes, so gating on is_connected avoids a guaranteed
    // first-paint error. The Effect re-runs when is_connected flips true.
    Effect::new(move || {
        if dashboard.is_connected.get() {
            load();
        }
    });

    // Select a session: set ChatState, restore project root, load history, return
    // to the chat surface.
    let on_select = move |row: SessionRow| {
        let navigate = navigate.clone();
        let dash = dashboard;
        if chat.session_key.get_untracked().as_deref() == Some(row.key.as_str()) {
            navigate("/", NavigateOptions::default());
            return;
        }
        // Reuse-or-open + activate + register, through the writer the wide
        // sidebar and the project-room entry share. `activate` swaps the
        // singleton's contents for the incoming conversation's, which is why it
        // runs before anything below writes to `chat`.
        let topic = row.topic.clone();
        let key_for_label = row.key.clone();
        sessions.adopt_session(chat, &row.agent_id, &row.key, move || {
            topic.filter(|t| !t.is_empty()).unwrap_or(key_for_label)
        });

        chat.clear_team_context();
        chat.agent_id.set(Some(row.agent_id.clone()));
        chat.session_key.set(Some(row.key.clone()));
        chat.active_project_root.set(row.project_root.clone());
        // The dials this conversation is actually being run with. Without this
        // the phone's pills reported the install defaults while the run loop
        // kept resolving the stored values every turn — and the tier pill is
        // the one that says which tool calls stop for approval.
        chat.apply_session_knobs(row.knobs());
        // `hydrate_and_follow`, not the bare hydrate: opening a session
        // somebody else is mid-turn on — another Panel, the CLI, a channel,
        // cron, a crash-recovered run — used to show a complete-looking
        // transcript that then never moved, because this client never saw that
        // run's `run_accepted` and so had no route for its frames. It now joins
        // from the open point on.
        let locale = i18n.get_locale_untracked();
        let key = row.key.clone();
        spawn_local(hydrate_and_follow(
            dash,
            chat,
            Some(workspace),
            sessions,
            key,
            locale,
        ));
        navigate("/", NavigateOptions::default());
    };

    view! {
        <PhoneShell title="History" back="/" back_label="Chat">
            // Single wrapping element for PhoneShell children (the dynamic list
            // block must not be a bare direct child).
            <div style="display:flex; flex-direction:column; gap:20px;">
            {move || {
                if loading.get() {
                    // Distinguish "waiting for the socket" from "fetch in flight"
                    // so a cold boot shows Connecting… instead of a stuck spinner.
                    let label = if dashboard.is_connected.get() { "Loading…" } else { "Connecting…" };
                    return view! { <div class="list-header">{label}</div> }.into_any();
                }
                if let Some(err) = load_error.get() {
                    return view! {
                        <div class="list">
                            <div class="cell"><div class="cell-body"><div class="cell-title">"Couldn't load conversations"</div><div class="cell-sub">{err}</div></div></div>
                            <div class="cell" on:click=move |_| load()><div class="cell-body"><div class="cell-title" style="color:var(--color-primary);">"Retry"</div></div></div>
                        </div>
                    }.into_any();
                }
                let items = rows.get();
                if items.is_empty() {
                    return view! { <div class="list-header">"No conversations yet"</div> }.into_any();
                }
                view! {
                    <div class="list">
                        {items.into_iter().map(|row| {
                            let on_select = on_select.clone();
                            let title = row.topic.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "Untitled".to_string());
                            let sub = format!("{} messages", row.message_count);
                            let row_for_click = row.clone();
                            view! {
                                <div class="cell" on:click=move |_| on_select(row_for_click.clone())>
                                    <div class="cell-body">
                                        <div class="cell-title">{title}</div>
                                        <div class="cell-sub">{sub}</div>
                                    </div>
                                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
            </div>
        </PhoneShell>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_sessions_list_row() {
        let json = serde_json::json!({
            "key": "agent-main:default",
            "agent_id": "agent-main",
            "topic": "Build the phone chat",
            "message_count": 7,
            "updated_at": 1_750_000_000_i64,
            "project_root": null
        });
        let row: SessionRow = serde_json::from_value(json).unwrap();
        assert_eq!(row.key, "agent-main:default");
        assert_eq!(row.agent_id, "agent-main");
        assert_eq!(row.topic.as_deref(), Some("Build the phone chat"));
        assert_eq!(row.message_count, 7);
        assert_eq!(row.updated_at, Some(1_750_000_000));
    }

    #[test]
    fn deserializes_with_missing_optional_fields() {
        let json = serde_json::json!({ "key": "k" });
        let row: SessionRow = serde_json::from_value(json).unwrap();
        assert_eq!(row.key, "k");
        assert_eq!(row.agent_id, "");
        assert_eq!(row.topic, None);
        assert_eq!(row.message_count, 0);
        assert_eq!(row.updated_at, None);
    }

    /// The sort is about `updated_at` only, so the rows are built from the
    /// default and given just that — a literal spelling out every dial would
    /// have to be edited whenever a dial is added, for a test that never reads
    /// one.
    fn row_at(key: &str, updated_at: Option<i64>) -> SessionRow {
        SessionRow {
            key: key.into(),
            updated_at,
            ..SessionRow::default()
        }
    }

    #[test]
    fn sorts_newest_first_none_last() {
        let rows = vec![
            row_at("old", Some(100)),
            row_at("none", None),
            row_at("new", Some(200)),
        ];
        let sorted = sort_sessions_desc(rows);
        let keys: Vec<&str> = sorted.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["new", "old", "none"]);
    }
}
