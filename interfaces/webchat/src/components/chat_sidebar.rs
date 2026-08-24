//
// Chat mode sidebar — agent dropdown + session list.
// Top dropdown selects agent, list shows that agent's sessions.
// Auto-refreshed via stream.session_updated Gateway events.
//
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use serde::Deserialize;
use std::sync::Arc;

use crate::api::chat::ChatApi;
use crate::api::team_chat
::{TeamChatApi, TeamMessageItem};
use crate::api::teams::{TeamSummary, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::layout::WorkspaceState;
use crate::state::sessions::SessionMap;
use crate::views::chat::agent_identity::agent_color_for_id;
use crate::views::chat::state::{
    ChatMessage, ChatState, ContextUsage, MemberStatus, TeamMemberView,
};
// The topic grammar is the protocol crate's, not a view module's: the server
// classifies delivery from the same parser (`event_visibility`).
use aleph_protocol::team_topic::{parse_team_topic, TeamTopicKind};
// The wire shape of the wait lane's `pending[]` entries — the same type the
// server serializes onto `chat.history`, so a field rename there is a
// compile error here rather than a client that silently reads an empty queue.
use aleph_protocol::queue::PendingRun;

use web_sys::HtmlInputElement;

/// A session entry returned by the backend (`sessions.list`).
///
/// One decoder for every Panel surface — the phone history reads the same rows
/// (`api::sessions::SessionRow`). It used to be two hand-written copies, and
/// they had diverged: the phone's carried no dials, so tapping a row there
/// restored the folder and dropped the tier and the mode while the server kept
/// enforcing them.
use crate::api::sessions::SessionRow as SessionEntry;

/// An agent entry returned by the backend (agents.list).
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct AgentEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    emoji: Option<String>,
    #[serde(default)]
    is_default: bool,
}

/// Should a `run.session_updated` frame re-hydrate the open transcript from
/// `chat.history`?
///
/// The question is "did somebody ELSE touch this session", and the only field
/// that can answer it is `origin_run_id`. `origin_channel` cannot: every Panel
/// connection hardcodes the literal `"gui:chat"` (`api/chat.rs`), so reading
/// that literal as "my own update" is a channel-CLASS test standing in for an
/// identity test — it says "mine" for a second tab of the same user and for
/// every other member of a project room, and those turns then never appear
/// until the viewer reselects the session.
///
/// Three answers, in the order the arms take them:
/// - **No origin at all** ⇒ no run caused this. A topic/title/state edit
///   (`SessionManager::emit_session_updated`); the sidebar row already
///   refreshed above and the transcript is untouched by it.
/// - **A run we started** ⇒ skip. The live `run.*` stream already built the
///   correct transcript, and reloading over it replaces clean tool/step rows
///   with raw fallback bubbles the instant the run completes.
/// - **Anything else** ⇒ re-hydrate. An external surface (Telegram, Slack…), a
///   second tab, another room member — all runs this Panel did not start.
///
/// A run id we cannot see (`None` with a real origin) counts as somebody
/// else's. That is only reachable against a core older than this field, and it
/// is the safer half of that skew: a needless reload is cosmetic and self-heals,
/// whereas silently never showing a room peer's message is the defect this
/// predicate exists to fix.
fn session_update_needs_rehydrate(
    origin_channel: &str,
    origin_run_id: Option<&str>,
    started_here: impl Fn(&str) -> bool,
) -> bool {
    if origin_channel.is_empty() {
        return false;
    }
    origin_run_id.is_none_or(|run| !started_here(run))
}

/// The server stores the user's own group-chat messages under this reserved
/// `from_agent` handle (mirror of `teams::broadcast::RESERVED_USER_HANDLE`). On
/// history replay they must render as right-aligned user bubbles, not as
/// attributed agent bubbles.
const RESERVED_USER_HANDLE: &str = "user";

/// Map one replayed `teams.chat.history` item to a chat bubble.
///
/// The render class comes from the server's `kind` (`user` | `agent` |
/// `system`) — one classification, derived once, next to the store that knows
/// the message's recipients and type. `from_agent` is only consulted as the
/// pre-`kind` fallback so a Panel pointed at an older core still splits its own
/// messages out of the agent bubbles. `index` seeds a stable dom id.
fn team_history_item_to_message(index: usize, item: TeamMessageItem) -> ChatMessage {
    let role = match item.kind.as_str() {
        "user" => "user",
        "system" => "system",
        // Legacy core (no `kind`, defaulted to "agent"): fall back to the
        // handle check so own messages don't replay as agent bubbles.
        _ if item.from_agent == RESERVED_USER_HANDLE => "user",
        _ => "assistant",
    };
    ChatMessage {
        id: format!("team-hist-{index}"),
        role: role.to_string(),
        content: item.content,
        tool_calls: Vec::new(),
        is_streaming: false,
        is_intermediate: false,
        error: None,
        model_info: None,
        timestamp: Some(item.created_at),
        iteration: None,
        is_final: true,
        text_finalized: true,
        // Only agent bubbles carry attribution; user and system rows must stay
        // out of the Telegram-style grouping pass.
        agent_id: (role == "assistant").then_some(item.from_agent),
        plan_archive: None,
        // `teams.chat.history` is the legacy group-broadcast surface (not a
        // P2 project room) — it carries no `author_user_id`.
        author_user_id: None,
    }
}

/// Pick the gauge occupancy for a freshly-loaded session: the most recent
/// assistant turn carrying a persisted occupancy (core only stamps it when a
/// real LLM call ran). `None` when no turn carries one, which correctly leaves
/// the gauge hidden. Pure + sync so it is unit-testable without a Leptos owner.
#[must_use]
pub(crate) fn occupancy_from_history(
    history: &[crate::api::chat::ChatMessage],
) -> Option<ContextUsage> {
    history.iter().rev().find_map(|m| {
        let used = m.context_tokens?;
        let window = m.context_window?;
        if used == 0 || window == 0 {
            return None;
        }
        Some(ContextUsage {
            used_tokens: used,
            window_tokens: window,
            total_tokens: m.total_tokens.unwrap_or(0),
            is_estimate: false,
        })
    })
}

/// Fetch a session's history (+ persisted run traces) and rebuild the
/// transcript in `chat.messages`. Shared by session selection and the
/// external-update live refresh (`run.session_updated` with an
/// `origin_channel`); callers must already have `chat.session_key`
/// pointing at `key`.
///
/// Returns the run in flight on this session at fetch time, if any, plus the
/// snapshot's wait lane. Callers that own a [`SessionMap`] should hand both to
/// [`hydrate_and_follow`] rather than calling this directly — a transcript
/// loaded while a turn is running is only half the answer, and the lane is
/// what lets a client that attaches mid-wait repaint the queued phase that a
/// live client would have learned from `RunQueued` frames it never saw.
pub(crate) async fn hydrate_session_history(
    dash: DashboardState,
    chat: ChatState,
    workspace: Option<WorkspaceState>,
    key: String,
    locale: crate::i18n::Locale,
) -> (Option<String>, Vec<PendingRun>) {
    match ChatApi::history(&dash, &key, Some(50)).await {
        Ok(loaded) => {
            let active_run = loaded.active_run;
            let pending = loaded.pending;
            let history = loaded.messages;
            // Distinct assistant run_ids → fetch their persisted traces.
            let run_ids: Vec<String> = {
                let mut seen = std::collections::HashSet::new();
                history
                    .iter()
                    .filter(|m| m.role == "assistant")
                    .filter_map(|m| m.run_id.clone())
                    .filter(|r| seen.insert(r.clone()))
                    .collect()
            };

            let traces: std::collections::HashMap<String, Vec<serde_json::Value>> =
                if run_ids.is_empty() {
                    std::collections::HashMap::new()
                } else {
                    match crate::api::trace::TraceApi::by_runs(&dash, &key, run_ids).await {
                        Ok(runs) => runs,
                        Err(e) => {
                            web_sys::console::warn_1(&format!("trace.by_runs failed: {e}").into());
                            std::collections::HashMap::new()
                        }
                    }
                };

            // Build the transcript in order: replay traced assistant
            // runs into the (already-cleared) real chat; push user rows
            // and trace-less assistant rows as plain bubbles.
            chat.messages.set(Vec::new());
            for (i, m) in history.iter().enumerate() {
                let ts = m
                    .timestamp
                    .as_deref()
                    .and_then(crate::views::chat::timeline::parse_wire_timestamp);

                let traced = m.role == "assistant"
                    && m.run_id
                        .as_deref()
                        .and_then(|r| traces.get(r))
                        .map(|evs| !evs.is_empty())
                        .unwrap_or(false);

                let replayed = if traced {
                    if let (Some(run), Some(ws)) = (m.run_id.as_deref(), workspace) {
                        let evs = traces.get(run).cloned().unwrap_or_default();
                        crate::views::chat::events::replay_run(
                            chat, ws, run, &evs, &m.content, locale,
                        );
                        // Stamp the final bubble's timestamp from history
                        // so day separators stay correct.
                        let target = format!("assistant-{run}");
                        chat.messages.update(|msgs| {
                            if let Some(b) = msgs.iter_mut().rev().find(|b| b.id == target) {
                                b.timestamp = ts;
                            }
                        });
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };

                // Fall back to a plain bubble whenever replay did NOT
                // run — including the unreachable "traced but no
                // workspace" case, so a row is never silently dropped.
                // Skip the empty assistant placeholder a tool-call turn leaves
                // behind (text-less turn) so a trace-less reload doesn't render
                // blank bubbles. `role="tool"` rows keep their content and
                // render compactly via the timeline's tool-fallback row.
                if !replayed && !m.content.trim().is_empty() {
                    chat.messages.update(|msgs| {
                        msgs.push(crate::views::chat::state::ChatMessage {
                            timestamp: ts,
                            id: m.run_id.clone().unwrap_or_else(|| format!("hist-{i}")),
                            role: m.role.clone(),
                            content: m.content.clone(),
                            tool_calls: vec![],
                            is_streaming: false,
                            is_intermediate: false,
                            error: None,
                            model_info: None,
                            iteration: None,
                            is_final: false,
                            text_finalized: false,
                            agent_id: None,
                            plan_archive: None,
                            // The room member who typed this turn (P2 Task
                            // 6/8), when it was sent inside a project room.
                            author_user_id: m.author_user_id.clone(),
                        });
                    });
                }
            }

            // Loading an existing session = all activity already "seen";
            // clear the live-only badge + active-iteration marker that
            // replay set.
            if let Some(ws) = workspace {
                ws.unseen_artifacts.set(0);
            }

            // Re-project the gauge from the persisted occupancy. Hydration only
            // runs for a conversation with no local transcript to preserve, and
            // for that case the persisted occupancy is the authoritative source
            // (None = no real occupancy yet ⇒ fall back to a core estimate
            // below; the gauge stays hidden only if that estimate also fails).
            // A conversation restored from its background state keeps the
            // occupancy its snapshot carried, which is fresher than this.
            match occupancy_from_history(&history) {
                Some(real) => chat.context_usage.set(Some(real)),
                None => {
                    // No real occupancy recorded → ask core for a next-prompt
                    // estimate so a freshly-opened conversation still shows a
                    // `≈N%` gauge. Null/err ⇒ leave it hidden.
                    let est = ChatApi::context_estimate(&dash, &key).await.ok().flatten();
                    chat.context_usage.set(est.map(|e| ContextUsage {
                        used_tokens: e.used_tokens,
                        window_tokens: e.window_tokens,
                        total_tokens: u64::from(e.used_tokens),
                        is_estimate: true,
                    }));
                }
            }

            // The durable execution list, applied AFTER the replay loop.
            //
            // Order is the whole point. `replay_run` feeds every persisted
            // `tool_call_completed` back through the same projection the live
            // stream uses, so the Todo strip gets rebuilt from the trace — and
            // that trace is the deliberately lossy mirror, replayed with none
            // of the `settle_plan` reconciliation the live path has. Whatever
            // it produced is a guess; this is the file the model works, so it
            // speaks last.
            //
            // Applied only when the server actually sent one. `None` is
            // ambiguous — no list, or a core too old to have the field — and
            // clearing on ambiguity would take the strip away from the very
            // clients that just got it back.
            // `settle_plan`, not a hand-rolled `apply_plan_update` — the two
            // are not the same and the difference is visible. Settling also
            // SINKS a finished plan into the transcript, which is the terminal
            // state the live client is already in; showing it without sinking
            // pins a 100%-done checklist above this client's composer, and the
            // next turn then sinks it a second time — a duplicate archive
            // capsule, observed on a real machine (2026-08-10). Two clients of
            // one conversation disagreeing is the exact defect this whole
            // wiring exists to remove, so the cold path has to land on the same
            // state, not merely on the same data.
            if let Some(plan) = loaded.plan {
                chat.settle_plan(Some(&plan));
            }

            (active_run, pending)
        }
        Err(e) => {
            web_sys::console::error_1(&format!("Failed to load history: {e}").into());
            (None, Vec::new())
        }
    }
}

/// Restore the queued phase for a run this client is now following.
///
/// Live clients reach it through `RunQueued`; a client that attached mid-wait
/// never saw those frames — they fired before its socket existed — so the
/// snapshot it already fetched is the only place the fact survives.
///
/// `mark_queued` is scoped to `active_run_id`, so a lane entry for a sibling
/// run repaints nothing, and replaying the same value is idempotent.
fn restore_queued_phase(chat: &ChatState, run_id: &str, pending: &[PendingRun]) {
    if let Some(entry) = pending.iter().find(|p| p.run_id == run_id) {
        chat.mark_queued(run_id, entry.ahead);
    }
}

/// Hydrate `key`'s transcript and, when a turn is already in flight on it,
/// **join** that turn instead of watching a frozen transcript.
///
/// # The gap this closes
///
/// Opening a conversation someone else is currently driving — a second Panel
/// tab, another member of a project room, a run started from the CLI, a
/// channel, or cron — used to render a complete-looking transcript that then
/// never moved. Nothing was wrong with the stream: every `stream.*` frame for
/// that run reached this client. They were dropped one layer up, because
/// `resolve_target` routes by `run_id` and this client never saw the
/// `run_accepted` that would have bound it. Meanwhile the sidebar's re-hydrate
/// is deliberately suppressed while a session is running, so nothing else
/// filled the gap either: the viewer waited out the whole turn and only saw it
/// appear at the end.
///
/// Binding the run id here restores the route, so the rest of the turn renders
/// live from the join point on, and `run_complete` finishes with the
/// history-authoritative answer — the joiner loses the *animation* of the part
/// that already happened, never the content.
///
/// # Deliberate non-goals
///
/// - **No replay of the already-streamed part.** The core does not buffer it,
///   and inventing a buffer to animate the past would be a second source of
///   truth for text the transcript already owns.
/// - **A run that ends between the fetch and the bind leaves a placeholder.**
///   Self-healing, not ignored: that run's terminal
///   `run.session_updated` — which since 2026-08-10 fires on the failure path
///   too — finds the session no longer running and re-hydrates over it.
pub(crate) async fn hydrate_and_follow(
    dash: DashboardState,
    chat: ChatState,
    workspace: Option<WorkspaceState>,
    sessions: SessionMap,
    key: String,
    locale: crate::i18n::Locale,
) {
    let (run_id, pending) =
        hydrate_session_history(dash, chat, workspace, key.clone(), locale).await;
    let Some(run_id) = run_id else {
        return;
    };
    // Already following it — our own send bound it, or `run_accepted` did.
    // A second bind would leave a residue in the double-bind witness that one
    // `settle_run` cannot clear (see `SessionMap::running`), and the route is
    // the thing being established here, so having it already IS the answer.
    if sessions.route_lookup(&run_id).is_some() {
        // `active_run_id` is already set on this path, so the phase can be
        // restored right here — an early return is still a follow path.
        restore_queued_phase(&chat, &run_id, &pending);
        return;
    }
    // `hydrate_session_history` wrote into the singleton `ChatState`, which is
    // the ACTIVE conversation's projection. Only bind when the session we just
    // loaded is in fact the active one, so the route and the bubbles cannot
    // point at two different conversations.
    let Some(conv) = sessions.conv_for_session_key(&key) else {
        return;
    };
    if sessions.active_conv() != Some(conv) {
        return;
    }
    sessions.bind_run(&run_id, conv, Some(&key));
    chat.start_assistant_message(&run_id);
    restore_queued_phase(&chat, &run_id, &pending);
}

#[component]
#[must_use]
pub fn ChatSidebar() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let session_map = expect_context::<SessionMap>();
    // Workspace pane state — used to reset the tool-detail view and evict
    // captured tool payloads whenever the chat session changes (switch /
    // new / delete). `Option` + `Copy` so it can be captured into every
    // session-gesture closure without panicking if the pane isn't mounted.
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();
    // Router handle for the Aleph Hub launcher in the advanced-features zone.
    let navigate = use_navigate();

    let agents = RwSignal::new(Vec::<AgentEntry>::new());
    let sessions = RwSignal::new(Vec::<SessionEntry>::new());
    let is_loading = RwSignal::new(false);
    // Which agent is selected in the dropdown (synced with chat.agent_id)
    let selected_agent = RwSignal::new(Option::<String>::None);
    // Agent picker popover visibility — closes on mouse-leave to match the
    // model picker / project menu affordances (see model_picker.rs).
    let agent_picker_open = RwSignal::new(false);
    // Team compose popover visibility
    let show_compose = RwSignal::new(false);

    // Session action states (edit/delete/menu — mutually exclusive)
    let editing_key = RwSignal::new(Option::<String>::None);
    let deleting_key = RwSignal::new(Option::<String>::None);
    let edit_text = RwSignal::new(String::new());
    let menu_open_key = RwSignal::new(Option::<String>::None);
    let is_saving = RwSignal::new(false);
    let edit_input_ref = NodeRef::<leptos::html::Input>::new();

    // Group-chat row action state — SEPARATE from session-row signals so the
    // single-chat state machine stays untouched. Keyed by team id.
    let group_editing_id = RwSignal::new(Option::<String>::None);
    let group_deleting_id = RwSignal::new(Option::<String>::None);
    let group_edit_text = RwSignal::new(String::new());
    let group_menu_id = RwSignal::new(Option::<String>::None);
    // Team ids that spoke while the user was looking somewhere else. The chat
    // view projects `team.<id>.message` only for the ACTIVE team (otherwise a
    // background group's bubbles land in whatever conversation is open), so
    // without this marker a group talking in the background is completely
    // invisible until the user happens to click it.
    let unread_groups: RwSignal<std::collections::HashSet<String>> =
        RwSignal::new(std::collections::HashSet::new());

    // Client-side session filter (R4 pure I/O — no backend search).
    let search_query = RwSignal::new(String::new());

    // Groups (teams) the selected agent belongs to — drives the group-chat section.
    let groups: RwSignal<Vec<TeamSummary>> = RwSignal::new(Vec::new());
    // Collapsible state for the group section (default: expanded).
    let groups_expanded = RwSignal::new(true);

    // Reusable closure: fetch both agents and sessions from the backend.
    let reload_data = Arc::new(move |dash: DashboardState| {
        is_loading.set(true);
        leptos::task::spawn_local(async move {
            // Fetch agents
            match dash.rpc_call("agents.list", serde_json::json!({})).await {
                Ok(result) => {
                    if let Some(arr) = result.get("agents") {
                        if let Ok(list) = serde_json::from_value::<Vec<AgentEntry>>(arr.clone()) {
                            // Auto-select default agent if none selected.
                            // Routing through SessionMap.activate opens the
                            // first tab — Cmd+1 will focus it.
                            // Bail if the component was disposed while the
                            // request was in flight (cold-start remount race):
                            // `selected_agent` is owned by this component, so a
                            // plain `get_untracked` panics on a disposed signal.
                            // `try_*` + early return turns the late async task
                            // into a no-op instead.
                            let Some(sel) = selected_agent.try_get_untracked() else {
                                return;
                            };
                            if sel.is_none() {
                                let default_id = list
                                    .iter()
                                    .find(|a| a.is_default)
                                    .or(list.first())
                                    .map(|a| a.id.clone());
                                if let Some(id) = default_id {
                                    selected_agent.set(Some(id.clone()));
                                    session_map.start_new(
                                        chat,
                                        &id,
                                        t_string!(i18n, chat.new_chat).to_string(),
                                    );

                                }
                            }
                            agents.set(list);
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to list agents: {e}").into());
                }
            }

            // Fetch sessions
            match dash.rpc_call("sessions.list", serde_json::json!({})).await {
                Ok(result) => {
                    if let Some(arr) = result.get("sessions") {
                        if let Ok(list) = serde_json::from_value::<Vec<SessionEntry>>(arr.clone()) {
                            sessions.set(list);
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to list sessions: {e}").into());
                }
            }

            // The running-set seed used to be taken here as well. It was a
            // third round trip to `gateway.metrics.run_concurrency` per
            // reconnect and a fourth on every `run.session_updated` — and a
            // no-op on all of them but the first, because `seed_server_running`
            // only applies while no live frame has advanced the baseline. The
            // one place that both needs the answer and acts on it
            // (`state::reattach`) now takes it once.

            // Fetch teams for the selected agent (drives the group-chat section).

            // `try_get_untracked`: outer `None` = component disposed, inner
            // `None` = no agent selected — either way skip.
            if let Some(Some(agent_id)) = selected_agent.try_get_untracked() {
                match TeamsApi::agent_teams(&dash, &agent_id).await {
                    Ok(team_list) => groups.set(team_list),
                    Err(e) => {
                        web_sys::console::warn_1(
                            &format!("Failed to list agent teams: {e}").into(),
                        );
                    }
                }
            }

            is_loading.set(false);
        });
    });

    // Fetch data on mount, and again on every (re)connect.
    let dash = dashboard;
    let reload_for_mount = reload_data.clone();
    Effect::new(move || {
        // Tracked for the dependency, not for the value: `connection_epoch`
        // ticks once per successful handshake, so this effect re-runs even for
        // a socket that was replaced without `is_connected` visibly flipping.
        let _epoch = dash.connection_epoch.get();
        if dash.is_connected.get() {
            // The list reload only. Re-basing the running-set sequence,
            // settling routes the server no longer confirms and re-joining the
            // turn it still does are one repair against one snapshot, and they
            // live at the app root (`state::reattach`) so the phone — which
            // never mounts this component — inherits them too.
            reload_for_mount(dash);
        }
    });

    // M1: keep open tab labels in sync with backend-assigned session topics.
    // When the session list reloads (topic generated after the first exchange,
    // or a rename via `sessions.set_topic`), mirror each session's topic onto
    // its open tab so the strip shows the conversation subject instead of the
    // raw session_key / "New conversation". Reads only `sessions`; `conv_for_session_key`
    // and `set_label` are untracked writes, so there is no reactive loop.
    Effect::new(move || {
        for s in &sessions.get() {
            if let (Some(topic), Some(conv)) =
                (s.topic.as_deref(), session_map.conv_for_session_key(&s.key))
            {
                session_map.set_label(conv, topic);
            }
        }
    });

    // Subscribe to session_updated events so the list refreshes automatically.
    // Frames carrying an `origin_channel` mean another surface (Telegram,
    // Slack, …) touched the session: if it's the one currently open and no
    // local run is in flight, re-hydrate the transcript so the Panel mirrors
    // the channel conversation live. Panel-originated runs publish no origin
    // and never trigger a self-refresh (no clobbering of streaming state).
    let reload_for_event = reload_data.clone();
    let sub_dash = dashboard;
    // Keep the composer pills honest against the store: the model's
    // `session_set_mode` (or a patch from another surface) emits
    // `run.session_updated` → `reload_for_event` refreshes `sessions` → this
    // effect writes the refreshed per-session overrides back into the signals
    // the pills and the right-rail mode dispatch read. A local pick is not
    // clobbered: the pill's own `sessions.patch` write triggers the same
    // refresh, which round-trips the just-picked value.
    Effect::new(move |_| {
        let list = sessions.get();
        let Some(key) = chat.session_key.get_untracked() else {
            return;
        };
        let Some(row) = list.iter().find(|s| s.key == key) else {
            return;
        };
        let knobs = row.knobs();
        if chat.session_knobs() != knobs {
            chat.apply_session_knobs(knobs);
        }
    });

    let subscription_id = dashboard.subscribe_events(move |event| {
        if event.topic == "run.running_set_changed" {
            let seq = event
                .data
                .get("seq")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let running: std::collections::HashSet<String> = event
                .data
                .get("running")
                .and_then(serde_json::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            session_map.set_server_running(seq, running);
            return;
        }
        if event.topic == "team.changed" {
            reload_for_event(sub_dash);
            return;
        }
        // A background group spoke — badge its row. Scoped the same way the
        // chat view scopes its projection, so the team you are already reading
        // never marks itself unread.
        if let Some((team_id, TeamTopicKind::Message)) = parse_team_topic(&event.topic) {
            if chat.team_id.get_untracked().as_deref() != Some(team_id) {
                let id = team_id.to_string();
                unread_groups.update(|s| {
                    s.insert(id);
                });
            }
            return;
        }
        if event.topic != "run.session_updated" {
            return;
        }
        reload_for_event(sub_dash);

        let origin = event
            .data
            .get("origin_channel")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let origin_run = event.data.get("origin_run_id").and_then(|v| v.as_str());
        if !session_update_needs_rehydrate(origin, origin_run, crate::api::chat::is_own_run) {
            return;
        }
        let Some(sk) = event.data.get("session_key").and_then(|v| v.as_str()) else {
            return;
        };
        if chat.session_key.get_untracked().as_deref() != Some(sk) {
            return;
        }
        // "Is this session running?" is asked here, by the sidebar dot, and by
        // the active-run counter — and it must be the SAME answer. This one
        // used to read the client-side per-conversation refcount while the
        // other two read the server-authoritative set, which is a second
        // source of truth for one fact and, worse, a leaky one: the refcount
        // is decremented only by `run_complete` / `run_error`, so a bind whose
        // terminal frame never arrives (the run ended between
        // `hydrate_and_follow`'s fetch and its bind, a socket drop, a core
        // restart) pins it above zero and suppresses re-hydration for this
        // conversation **permanently**. The server's set has no such failure
        // mode: it is re-seeded on every reconnect and reconciled against.
        if session_map.is_running_session_key(sk) {
            return;
        }
        leptos::task::spawn_local(hydrate_and_follow(
            sub_dash,
            chat,
            workspace,
            session_map,
            sk.to_string(),
            i18n.get_locale_untracked(),
        ));
    });

    // Ask the Gateway to push stream.session_updated events to this client.
    let dash_for_topic = dashboard;
    leptos::task::spawn_local(async move {
        // No local wait for the socket: `rpc_call` parks the request itself
        // until the handshake is done (see `DashboardState::rpc_call`). The
        // 50×100 ms poll that used to live here was one of five hand-rolled
        // answers to that question, and the shortest — it gave up after 5 s and
        // subscribed anyway, which is the failure it was written to prevent.

        // Run lifecycle topics drive the per-session running dot;
        // team.changed drives live group-chat name refresh after async auto-naming.
        for topic in [
            "stream.run_accepted",
            "stream.run_complete",
            "stream.run_error",
            "stream.running_set_changed",
            "team.changed",
        ] {
            if let Err(e) = dash_for_topic.subscribe_topic(topic).await {
                web_sys::console::error_1(&format!("Failed to subscribe to {topic}: {e}").into());
            }
        }
    });

    // Cleanup: unsubscribe event handler when the component unmounts.
    let dash_for_cleanup = dashboard;
    on_cleanup(move || {
        dash_for_cleanup.unsubscribe_events(subscription_id);
    });

    // Select a session and load its history.
    let on_select_session = move |key: String, agent_id: String| {
        let dash = dashboard;
        let current = chat.session_key.get_untracked();
        if current.as_deref() == Some(&key) {
            return;
        }
        // Switch tabs first: this snapshots the outgoing conversation and
        // restores the incoming one, so the draft, the prompt queue and the
        // live `active_run_id` all come back with it. Only the state no
        // snapshot carries (team roster / tasks) still needs clearing — the
        // full `clear_session()` used to run here and undid the restore one
        // line after it happened. The history load below overwrites
        // `messages` either way.
        // Reuse-or-open + activate + register, in one writer shared with the
        // project-room entry and the phone's history list. The label closure
        // only runs when a tab actually has to be opened: the session's topic
        // (M1), falling back to the raw key while the backend has not assigned
        // one yet.
        session_map.adopt_session(chat, &agent_id, &key, || {
            sessions
                .get_untracked()
                .iter()
                .find(|s| s.key == key)
                .and_then(|s| s.topic.clone())
                .unwrap_or_else(|| key.clone())
        });
        chat.clear_team_context();

        if let Some(ws) = workspace {
            ws.reset();
        }
        selected_agent.set(Some(agent_id));
        chat.session_key.set(Some(key.clone()));

        // Restore the session's persisted project folder (G3) so the composer
        // keeps running inside it and the project pill reflects it. Set the
        // signals directly rather than via `set_active_project`, which would
        // clear the session we're about to load. `None` reverts to the default
        // workspace.
        let restored_root = sessions
            .get_untracked()
            .iter()
            .find(|s| s.key == key)
            .and_then(|s| s.project_root.clone());
        let restored_name = restored_root.as_deref().map(|p| {
            p.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(p)
                .to_string()
        });
        chat.active_project_root.set(restored_root);
        chat.active_project_name.set(restored_name);

        // Same treatment for the session's dials: the server's stored values are
        // authoritative because the run loop resolves the STORED ones every
        // turn, so a pill showing anything else — a stale snapshot value, or
        // "follow global" after a blanket clear — would under-report what is
        // actually being enforced. Set the signals directly from what the
        // session list reports; going through a picker's `select` would
        // re-issue a `sessions.patch` write on every selection.
        //
        // A row we cannot find leaves the dials at their defaults, which is
        // also what `clear_session` just set them to — the same answer the
        // surface gave before any of them existed.
        chat.apply_session_knobs(
            sessions
                .get_untracked()
                .iter()
                .find(|s| s.key == key)
                .map(SessionEntry::knobs)
                .unwrap_or_default(),
        );

        // A conversation that is already open keeps a background `ChatState`
        // which the global dispatcher feeds the entire time it is backgrounded,
        // so what `activate` just restored is at least as fresh as the server's
        // history — and strictly fresher while a run is still streaming, since
        // those rows are not persisted until it completes. Hydrating anyway
        // replaced a live transcript with an empty one: the message list went
        // blank while the composer correctly still showed Stop. Load history
        // only when there is nothing to preserve, i.e. a conversation being
        // opened here for the first time.
        if chat.messages.with_untracked(Vec::is_empty) {
            leptos::task::spawn_local(hydrate_and_follow(
                dash,
                chat,
                workspace,
                session_map,
                key,
                i18n.get_locale_untracked(),
            ));
        }
    };

    // Enter team chat mode: fetch detail, build roster, replay history.
    // The team.* subscription and its Gateway topic are already established
    // permanently in ChatView (view.rs) — no double-subscribe needed here.
    let on_open_group = move |team_id: String| {
        let dash = dashboard;
        // Opening the group IS reading it — drop the badge up front so the
        // marker never outlives the reason for it (the async hydrate below can
        // fail; the user still opened the room).
        unread_groups.update(|s| {
            s.remove(&team_id);
        });
        leptos::task::spawn_local(async move {
            // 1. Fetch team detail (members list).
            let detail = match TeamsApi::get(&dash, &team_id).await {
                Ok(d) => d,
                Err(e) => {
                    web_sys::console::error_1(&format!("teams.get failed: {e}").into());
                    return;
                }
            };
            // 2. Build id→AgentEntry map from current agents signal for name/emoji resolution.
            //    `agents` is owned by this component; if it was disposed while
            //    `teams.get` was in flight, bail rather than panic.
            let Some(agent_list) = agents.try_get_untracked() else {
                return;
            };
            let agent_map: std::collections::HashMap<String, AgentEntry> =
                agent_list.into_iter().map(|a| (a.id.clone(), a)).collect();
            let roster: Vec<TeamMemberView> = detail
                .members
                .iter()
                .map(|m| {
                    let entry = agent_map.get(&m.agent_id);
                    let name = entry
                        .and_then(|a| a.name.clone())
                        .unwrap_or_else(|| m.agent_id.clone());
                    let emoji = entry.and_then(|a| a.emoji.clone());
                    TeamMemberView {
                        agent_id: m.agent_id.clone(),
                        name,
                        emoji,
                        role: m.role.clone(),
                        is_leader: m.role == "leader",
                        status: MemberStatus::Idle,
                    }
                })
                .collect();
            // 3. Enter team mode.
            chat.clear_session();
            chat.team_id.set(Some(team_id.clone()));
            chat.team_members.set(roster);
            // 4. Replay durable chat history as bubbles.
            match TeamChatApi::history(&dash, &team_id).await {
                Ok(items) => {
                    let messages: Vec<ChatMessage> = items
                        .into_iter()
                        .enumerate()
                        .map(|(i, it)| team_history_item_to_message(i, it))
                        .collect();
                    chat.messages.set(messages);
                }
                Err(e) => {
                    web_sys::console::warn_1(&format!("teams.chat.history failed: {e}").into());
                }
            }
        });
    };

    // New conversation: open a new ConvId under the selected agent and activate it (new tab),
    // without clearing / replacing the currently running conversation. session_key=None -> first send triggers a new epoch.
    let on_new_chat = move |_: web_sys::MouseEvent| {
        if let Some(agent_id) = selected_agent.get_untracked() {
            session_map.start_new(chat, &agent_id, t_string!(i18n, chat.new_chat).to_string());

            if let Some(ws) = workspace {
                ws.reset();
            }
            // activate already restored the singleton to empty state; explicit clear ensures cleanliness.
            chat.clear_session();
            chat.agent_id.set(Some(agent_id));
        }
    };

    // --- Session action helpers ---

    let clear_action_states = move || {
        editing_key.set(None);
        deleting_key.set(None);
        menu_open_key.set(None);
        edit_text.set(String::new());
    };

    let reload_for_rename = reload_data.clone();
    let do_rename = Arc::new(move |session_key: String, topic: String| {
        if is_saving.get_untracked() {
            return;
        }
        let topic = topic.trim().to_string();
        if topic.is_empty() {
            editing_key.set(None);
            edit_text.set(String::new());
            return;
        }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_for_rename.clone();
        leptos::task::spawn_local(async move {
            let params = serde_json::json!({
                "session_key": session_key,
                "topic": topic,
            });
            match dash.rpc_call("sessions.set_topic", params).await {
                Ok(_) => {
                    reload(dash);
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to rename session: {e}").into());
                }
            }
            is_saving.set(false);
            editing_key.set(None);
            edit_text.set(String::new());
        });
    });

    let reload_for_delete = reload_data.clone();
    let do_delete = Arc::new(move |session_key: String| {
        if is_saving.get_untracked() {
            return;
        }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_for_delete.clone();
        leptos::task::spawn_local(async move {
            let params = serde_json::json!({
                "session_key": session_key,
            });
            match dash.rpc_call("sessions.delete", params).await {
                Ok(_) => {
                    // If deleting the active session, clear it.
                    //
                    // `try_get_untracked` past every `.await` in this file: the
                    // sidebar unmounts with the phone history drawer, and a
                    // plain read of a disposed signal panics the whole panel.
                    // `ChatState` is root-owned (`app.rs` provides it), so this
                    // particular `None` arm is unreachable today — the rule is
                    // kept uniform so the guard in `disposed_reads` needs no
                    // allowlist to rot, and so scoping `ChatState` per-tab later
                    // cannot reintroduce the crash silently. `.flatten()` leaves
                    // the comparison and the control flow byte-identical.
                    if chat.session_key.try_get_untracked().flatten().as_deref()
                        == Some(&session_key)
                    {
                        chat.clear_session();
                        if let Some(ws) = workspace {
                            ws.reset();
                        }
                    }
                    reload(dash);
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to delete session: {e}").into());
                }
            }
            is_saving.set(false);
            deleting_key.set(None);
        });
    });

    let reload_for_grename = reload_data.clone();
    let do_rename_group = Arc::new(move |team_id: String, name: String| {
        if is_saving.get_untracked() {
            return;
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            group_editing_id.set(None);
            group_edit_text.set(String::new());
            return;
        }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_for_grename.clone();
        leptos::task::spawn_local(async move {
            if let Err(e) = TeamsApi::rename(&dash, &team_id, &name).await {
                web_sys::console::error_1(&format!("Failed to rename team: {e}").into());
            } else {
                reload(dash);
            }
            is_saving.set(false);
            group_editing_id.set(None);
            group_edit_text.set(String::new());
        });
    });

    let reload_for_gdelete = reload_data.clone();
    let do_delete_group = Arc::new(move |team_id: String| {
        if is_saving.get_untracked() {
            return;
        }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_for_gdelete.clone();
        leptos::task::spawn_local(async move {
            if let Err(e) = TeamsApi::disband(&dash, &team_id).await {
                web_sys::console::error_1(&format!("Failed to delete team: {e}").into());
            } else {
                // Root-owned like `session_key` above; uniform rule.
                if chat.team_id.try_get_untracked().flatten().as_deref() == Some(&team_id) {
                    chat.clear_session();
                }
                reload(dash);
            }
            is_saving.set(false);
            group_deleting_id.set(None);
        });
    });

    // Auto-focus edit input when entering edit mode. Both the session-row and
    // group-row edit inputs share `edit_input_ref` (only one row edits at a
    // time), so focus/select on EITHER signal turning Some is correct.
    Effect::new(move || {
        let _key = editing_key.get();
        let _g_key = group_editing_id.get();
        if _key.is_some() || _g_key.is_some() {
            leptos::task::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(10).await;
                if let Some(el) = edit_input_ref.get() {
                    let input: &HtmlInputElement = &el;
                    let _ = input.focus();
                    input.select();
                }
            });
        }
    });

    // Auto-dismiss delete confirmation after 5 seconds
    Effect::new(move || {
        let key = deleting_key.get();
        if let Some(k) = key {
            leptos::task::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(5000).await;
                // Component-owned signal + a 5 s timer: closing the drawer
                // inside that window is the ordinary case, not the edge case.
                if deleting_key.try_get_untracked().flatten().as_deref() == Some(&k) {
                    deleting_key.set(None);
                }
            });
        }
    });

    // Auto-dismiss group delete confirmation after 5 seconds (session parity).
    Effect::new(move || {
        let key = group_deleting_id.get();
        if let Some(k) = key {
            leptos::task::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(5000).await;
                // Same 5 s window as the session row above.
                if group_deleting_id.try_get_untracked().flatten().as_deref() == Some(&k) {
                    group_deleting_id.set(None);
                }
            });
        }
    });

    view! {
        <div class="flex flex-col h-full">
            // Top action area
            <div class="p-3 space-y-2">
                // ── Advanced features zone ──────────────────────────────
                // Team chat + Project management (placeholder). Each entry is
                // a full-width row, stacked vertically; future advanced entries
                // (e.g. workflows) keep getting appended into this block.
                <div class="flex flex-col gap-1.5">
                    <button
                        class="w-full inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg
                               bg-surface-sunken border border-border text-sm
                               hover:border-primary transition-colors"
                        title=move || t_string!(i18n, chat.team_chat).to_string()
                        on:click=move |_| show_compose.set(true)
                    >
                        {move || format!("👥 {}", t_string!(i18n, chat.team_chat))}
                    </button>
                    <button
                        class="w-full inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg
                               bg-surface-sunken border border-border text-sm
                               opacity-70 cursor-not-allowed"
                        title=move || t_string!(i18n, chat.project_management).to_string()
                        disabled=true
                    >
                        {move || format!("📁 {}", t_string!(i18n, chat.project_management))}
                        <span class="ml-auto text-[10px] px-1.5 py-0.5 rounded
                                     bg-surface-raised text-text-tertiary">
                            {move || t_string!(i18n, chat.coming_soon).to_string()}
                        </span>
                    </button>
                    <button
                        class="w-full inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg
                               bg-surface-sunken border border-border text-sm
                               hover:border-primary transition-colors"
                        title=move || t_string!(i18n, nav.extensions).to_string()
                        on:click={
                            let navigate = navigate.clone();
                            move |_| navigate("/extensions", Default::default())
                        }
                    >
                        {move || format!("🧩 {}", t_string!(i18n, nav.extensions))}
                    </button>
                </div>

                // Click-outside catcher: while the team compose popover is open,
                // this transparent full-screen layer (z-40) collapses it on any
                // click elsewhere — including the trigger button above, which
                // gives toggle-to-close for free. The popover sits at z-50, above
                // this catcher, so it stays interactive. Mirrors the session ⋯
                // menu dismiss pattern below.
                <Show when=move || show_compose.get()>
                    <div
                        class="fixed inset-0 z-40"
                        on:click=move |_| show_compose.set(false)
                    />
                </Show>
                // Team compose popover — pops up right below the advanced zone
                <Show when=move || show_compose.get()>
                    <crate::views::chat::team_compose::TeamCompose
                        on_close=Callback::new(move |()| show_compose.set(false))
                    />
                </Show>

                // Faint divider: advanced features zone ↔ normal chat
                <div class="border-t border-border/50"></div>

                // ── Normal chat: agent picker + new chat ────────────────
                <div class="flex items-center gap-2">
                    // Agent picker — custom popover that closes on mouse-leave,
                    // mirroring the model picker / section switcher affordances
                    // (see model_picker.rs / nav_menu.rs). Replaces the former
                    // native <select> so its dismissal matches the rest of the
                    // composer/sidebar pickers.
                    <div class="relative flex-1 min-w-0">
                        <button
                            type="button"
                            class="w-full flex items-center gap-2 px-3 py-1.5 rounded-lg bg-surface-sunken \
                                   border border-border text-sm text-text-primary hover:border-primary/60 \
                                   focus:outline-none focus:ring-2 focus:ring-primary/30 transition-colors"
                            on:click=move |_| agent_picker_open.update(|v| *v = !*v)
                        >
                            <span class="flex-1 min-w-0 truncate text-left">
                                {move || {
                                    let sel = selected_agent.get();
                                    let list = agents.get();
                                    match sel.as_deref() {
                                        Some(id) => list
                                            .iter()
                                            .find(|a| a.id == id)
                                            .map(|a| a.name.clone().unwrap_or_else(|| a.id.clone()))
                                            .unwrap_or_else(|| id.to_string()),
                                        None => "—".to_string(),
                                    }
                                }}
                            </span>
                            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                 stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"
                                 class=move || {
                                     if agent_picker_open.get() {
                                         "flex-shrink-0 text-text-tertiary rotate-180 transition-transform"
                                     } else {
                                         "flex-shrink-0 text-text-tertiary transition-transform"
                                     }
                                 }
                            >
                                <polyline points="18 15 12 9 6 15" />
                            </svg>
                        </button>

                        <Show when=move || agent_picker_open.get()>
                            // Anchored to the sidebar's top action area, so the
                            // picker opens DOWNWARD (`top-full` + `mt-2`). `max-h-[60vh]`
                            // caps it to the viewport and `overflow-y-auto` scrolls the
                            // overflow, so a long agent list never runs off the window.
                            <div class="glass animate-pop-in absolute top-full left-0 right-0 mt-2 z-50 \
                                        max-h-[60vh] overflow-y-auto rounded-xl border border-border \
                                        bg-surface-overlay/85 shadow-xl p-1.5 space-y-0.5"
                                on:mouseleave=move |_| agent_picker_open.set(false)>
                                {move || {
                                    let sel = selected_agent.get();
                                    agents
                                        .get()
                                        .into_iter()
                                        .map(|agent| {
                                            let id = agent.id.clone();
                                            let id_for_click = id.clone();
                                            let name = agent
                                                .name
                                                .clone()
                                                .unwrap_or_else(|| agent.id.clone());
                                            let emoji = agent.emoji.clone().unwrap_or_default();
                                            let is_selected = sel.as_deref() == Some(&id);
                                            view! {
                                                <button
                                                    type="button"
                                                    class=move || {
                                                        let base = "w-full flex items-center gap-2 px-3 py-2 \
                                                                    rounded-lg text-sm text-left";
                                                        if is_selected {
                                                            format!("{base} nav-tile-active")
                                                        } else {
                                                            format!("{base} nav-tile")
                                                        }
                                                    }
                                                    // Switch to that agent's tab. Don't clear the session:
                                                    // SessionMap.activate restores the tab's snapshot so the
                                                    // user resumes where they left off. The workspace pane is
                                                    // global — drop its stale tool-detail on switch.
                                                    on:click=move |_| {
                                                        let val = id_for_click.clone();
                                                        agent_picker_open.set(false);
                                                        if val.is_empty() {
                                                            return;
                                                        }
                                                        selected_agent.set(Some(val.clone()));
                                                        session_map.start_new(
                                                            chat,
                                                            &val,
                                                            t_string!(i18n, chat.new_chat).to_string(),
                                                        );

                                                        if let Some(ws) = workspace {
                                                            ws.reset();
                                                        }
                                                    }
                                                >
                                                    <span class="text-base">{emoji}</span>
                                                    <span class="flex-1 min-w-0 truncate">{name}</span>
                                                    {is_selected.then(|| view! {
                                                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none"
                                                             stroke="currentColor" stroke-width="3"
                                                             stroke-linecap="round" stroke-linejoin="round"
                                                             class="flex-shrink-0 text-primary">
                                                            <polyline points="20 6 9 17 4 12" />
                                                        </svg>
                                                    })}
                                                </button>
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                }}
                            </div>
                        </Show>
                    </div>
                    <button
                        class="w-9 h-9 shrink-0 flex items-center justify-center rounded-lg bg-primary text-white hover:bg-primary/90 transition-colors"
                        title=move || t_string!(i18n, chat.new).to_string()
                        aria-label=move || t_string!(i18n, chat.new).to_string()
                        on:click=on_new_chat
                    >
                        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <line x1="12" y1="5" x2="12" y2="19" />
                            <line x1="5" y1="12" x2="19" y2="12" />
                        </svg>
                    </button>
                </div>

                // Search — client-side filter over the session list.
                <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/30">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-text-tertiary flex-shrink-0">
                        <circle cx="11" cy="11" r="8" />
                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                    </svg>
                    <input
                        type="text"
                        class="flex-1 min-w-0 bg-transparent outline-none text-text-primary placeholder:text-text-tertiary"
                        placeholder=move || t_string!(i18n, chat.search_placeholder).to_string()
                        prop:value=move || search_query.get()
                        on:input=move |ev| search_query.set(event_target_value(&ev))
                    />
                </div>
            </div>

            // Click-outside overlay for session dropdown menu
            {move || {
                if menu_open_key.get().is_some() {
                    view! { <div class="fixed inset-0 z-40" on:click=move |_| menu_open_key.set(None) /> }.into_any()
                } else {
                    view! { <span /> }.into_any()
                }
            }}

            // Click-outside overlay for group dropdown menu
            {move || {
                if group_menu_id.get().is_some() {
                    view! { <div class="fixed inset-0 z-40" on:click=move |_| group_menu_id.set(None) /> }.into_any()
                } else {
                    view! { <span /> }.into_any()
                }
            }}

            // Session list + group section (single scroll container)
            <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">

                // ── Group Chat collapsible section ────────────────────
                {move || {
                    // Only show active teams; disbanded ones disappear immediately.
                    // Newest activity first: most recent message timestamp, falling
                    // back to team creation time when the team has no transcript yet.
                    let mut group_list: Vec<_> = groups.get().into_iter().filter(|g| g.status == "active").collect();
                    group_list.sort_by_key(|g| std::cmp::Reverse(g.last_message_at.unwrap_or(g.created_at)));
                    if group_list.is_empty() {
                        return view! { <span /> }.into_any();
                    }
                    let count = group_list.len();
                    // Read action states here to track reactivity for the whole section.
                    let _g_editing = group_editing_id.get();
                    let _g_deleting = group_deleting_id.get();
                    let _g_menu = group_menu_id.get();
                    let do_rename_group = do_rename_group.clone();
                    let do_delete_group = do_delete_group.clone();
                    let is_expanded = groups_expanded.get();
                    view! {
                        <div class="mb-1">
                            // Section header with chevron toggle
                            <button
                                class="w-full flex items-center gap-1 px-1 py-1 text-[11px]
                                       font-semibold text-text-tertiary hover:text-text-primary
                                       transition-colors select-none"
                                on:click=move |_| groups_expanded.update(|e| *e = !*e)
                            >
                                <span class="transition-transform"
                                    style=move || if groups_expanded.get() { "" } else { "transform: rotate(-90deg); display: inline-block;" }
                                >
                                    "▾"
                                </span>
                                {t!(i18n, chat.group_chats_count, count = move || count)}
                            </button>
                            // Group rows (shown only when expanded) — plain if/else, no <Show>,
                            // so the inner iterator closure stays in the outer reactive block.
                            {if is_expanded {
                                let rows = group_list.into_iter().map(move |group| {
                                    let group_id = group.id.clone();
                                    let group_name = group.name.clone();
                                    let last_msg = group.last_message.clone();
                                    let previews = group.members_preview.clone();
                                    let is_g_editing = _g_editing.as_deref() == Some(&group_id);
                                    let is_g_deleting = _g_deleting.as_deref() == Some(&group_id);
                                    let is_g_menu = _g_menu.as_deref() == Some(&group_id);
                                    let do_rename_group = do_rename_group.clone();
                                    let do_delete_group = do_delete_group.clone();

                                    if is_g_editing {
                                        let id_save = group_id.clone();
                                        let id_blur = group_id.clone();
                                        let r_key = do_rename_group.clone();
                                        let r_blur = do_rename_group;
                                        view! {
                                            <div class="w-full px-3 py-2 rounded-lg bg-surface-sunken border border-primary/40">
                                                <input
                                                    node_ref=edit_input_ref
                                                    class="w-full bg-transparent text-xs text-text-primary outline-none disabled:opacity-50"
                                                    prop:value=move || group_edit_text.get()
                                                    prop:disabled=move || is_saving.get()
                                                    maxlength=100
                                                    on:input=move |ev| group_edit_text.set(event_target_value(&ev))
                                                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                        match ev.key().as_str() {
                                                            "Enter" => {
                                                                let t = group_edit_text.get_untracked();
                                                                if t.trim().is_empty() {
                                                                    group_editing_id.set(None);
                                                                    group_edit_text.set(String::new());
                                                                } else {
                                                                    r_key(id_save.clone(), t);
                                                                }
                                                            }
                                                            "Escape" => {
                                                                group_editing_id.set(None);
                                                                group_edit_text.set(String::new());
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                    on:blur=move |_| {
                                                        let id = id_blur.clone();
                                                        let r = r_blur.clone();
                                                        leptos::task::spawn_local(async move {
                                                            gloo_timers::future::TimeoutFuture::new(100).await;
                                                            // Blur is very often the *last* thing that
                                                            // happens before this row goes away, so the
                                                            // 100 ms delay lands after disposal routinely.
                                                            let Some(cur) = group_editing_id.try_get_untracked()
                                                            else {
                                                                return;
                                                            };
                                                            if cur.as_deref() == Some(&id) {
                                                                let Some(t) = group_edit_text.try_get_untracked()
                                                                else {
                                                                    return;
                                                                };
                                                                if t.trim().is_empty() {
                                                                    group_editing_id.set(None);
                                                                    group_edit_text.set(String::new());
                                                                } else {
                                                                    r(id, t);
                                                                }
                                                            }
                                                        });
                                                    }
                                                />
                                            </div>
                                        }.into_any()
                                    } else if is_g_deleting {
                                        let id_del = group_id.clone();
                                        view! {
                                            <div class="w-full px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 flex items-center justify-between text-xs">
                                                <span class="text-red-400 font-medium">{move || t_string!(i18n, common.confirm_dissolve).to_string()}</span>
                                                <div class="flex items-center gap-1.5">
                                                    <button
                                                        class="px-2 py-0.5 rounded bg-red-500 text-white text-[10px] font-medium hover:bg-red-600 transition-colors disabled:opacity-50"
                                                        prop:disabled=move || is_saving.get()
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            do_delete_group(id_del.clone());
                                                        }
                                                    >
                                                        {move || t_string!(i18n, common.confirm).to_string()}
                                                    </button>
                                                    <button
                                                        class="px-2 py-0.5 rounded bg-surface-sunken text-text-secondary text-[10px] hover:bg-surface-raised transition-colors"
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            group_deleting_id.set(None);
                                                        }
                                                    >
                                                        {move || t_string!(i18n, common.cancel).to_string()}
                                                    </button>
                                                </div>
                                            </div>
                                        }.into_any()
                                    } else {
                                        // Normal mode: avatar cluster + name + last_message + hover ⋯ menu
                                        let id_click = group_id.clone();
                                        let id_menu = group_id.clone();
                                        let id_edit = group_id.clone();
                                        let id_del_menu = group_id.clone();
                                        let name_for_edit = group_name.clone();
                                        view! {
                                            <div class="relative group">
                                                <button
                                                    class="w-full text-left px-2 py-1.5 rounded-lg text-sm nav-tile flex items-center gap-2"
                                                    on:click=move |_| on_open_group(id_click.clone())
                                                >
                                                    // Avatar cluster (overlapping discs, up to 3)
                                                    <div class="flex items-center flex-shrink-0">
                                                        {previews.iter().take(3).enumerate().map(|(i, mp)| {
                                                            let color = agent_color_for_id(&mp.id);
                                                            let glyph = mp.emoji.clone()
                                                                .filter(|e| !e.is_empty())
                                                                .or_else(|| mp.name.as_ref()
                                                                    .and_then(|n| n.chars().next())
                                                                    .map(|c| c.to_uppercase().to_string()))
                                                                .or_else(|| mp.id.chars().next()
                                                                    .map(|c| c.to_uppercase().to_string()))
                                                                .unwrap_or_else(|| "?".to_string());
                                                            let margin = if i == 0 { "" } else { "-ml-2" };
                                                            view! {
                                                                <span
                                                                    class=format!(
                                                                        "{margin} w-6 h-6 rounded-full flex items-center justify-center \
                                                                         text-[10px] font-bold text-white \
                                                                         ring-2 ring-surface-sunken"
                                                                    )
                                                                    style=format!("background-color: {color};")
                                                                >
                                                                    {glyph}
                                                                </span>
                                                            }
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                    // Group name + last message
                                                    <div class="flex-1 min-w-0">
                                                        <div class="flex items-center gap-1.5 min-w-0">
                                                            <span class="truncate text-xs font-medium text-text-primary">
                                                                {group_name.clone()}
                                                            </span>
                                                            // Unread marker — this group spoke while
                                                            // the user was reading something else.
                                                            <Show when={
                                                                let id = group_id.clone();
                                                                move || unread_groups.with(|s| s.contains(&id))
                                                            }>
                                                                <span
                                                                    class="w-1.5 h-1.5 rounded-full bg-primary flex-shrink-0"
                                                                    title=move || t_string!(i18n, chat.team_unread).to_string()
                                                                ></span>
                                                            </Show>
                                                        </div>
                                                        {last_msg.clone().map(|m| view! {
                                                            <div class="truncate text-[10px] text-text-tertiary mt-0.5">
                                                                {m}
                                                            </div>
                                                        })}
                                                    </div>
                                                    // ⋯ button (visible on hover)
                                                    <button
                                                        class="opacity-0 group-hover:opacity-100 ml-1 px-1.5 py-0.5
                                                               rounded text-text-tertiary hover:text-text-primary
                                                               hover:bg-surface-raised transition-all text-xs flex-shrink-0"
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            let cur = group_menu_id.get_untracked();
                                                            if cur.as_deref() == Some(&id_menu) {
                                                                group_menu_id.set(None);
                                                            } else {
                                                                group_menu_id.set(Some(id_menu.clone()));
                                                            }
                                                        }
                                                    >"⋯"</button>
                                                </button>
                                                // Dropdown menu
                                                {if is_g_menu {
                                                    let name_e = name_for_edit.clone();
                                                    view! {
                                                        <div class="glass absolute right-0 top-full mt-1 z-50 min-w-[120px]
                                                                    bg-surface-overlay/85 border border-border rounded-lg shadow-xl
                                                                    py-1 text-xs">
                                                            <button
                                                                class="w-full text-left px-3 py-1.5 text-text-secondary
                                                                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                                                                on:click=move |ev: web_sys::MouseEvent| {
                                                                    ev.stop_propagation();
                                                                    group_menu_id.set(None);
                                                                    group_edit_text.set(name_e.clone());
                                                                    group_editing_id.set(Some(id_edit.clone()));
                                                                }
                                                            >{move || t_string!(i18n, chat.rename).to_string()}</button>
                                                            <button
                                                                class="w-full text-left px-3 py-1.5 text-red-400
                                                                       hover:bg-red-500/10 transition-colors"
                                                                on:click=move |ev: web_sys::MouseEvent| {
                                                                    ev.stop_propagation();
                                                                    group_menu_id.set(None);
                                                                    group_deleting_id.set(Some(id_del_menu.clone()));
                                                                }
                                                            >{move || t_string!(i18n, teams.disband).to_string()}</button>
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    view! { <span /> }.into_any()
                                                }}
                                            </div>
                                        }.into_any()
                                    }
                                }).collect::<Vec<_>>();
                                view! { <div class="space-y-0.5">{rows}</div> }.into_any()
                            } else {
                                view! { <span /> }.into_any()
                            }}
                        </div>
                    }.into_any()
                }}

                // ── Single-agent sessions ──────────────────────────
                {move || {
                    let session_list = sessions.get();
                    let sel_agent = selected_agent.get();
                    let _active_key = chat.session_key.get(); // track for reactivity
                    // Track action states for reactivity
                    let _editing = editing_key.get();
                    let _deleting = deleting_key.get();
                    let _menu = menu_open_key.get();

                    if is_loading.get() && session_list.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                {move || t_string!(i18n, common.loading).to_string()}
                            </p>
                        }.into_any();
                    }

                    // Filter by selected agent AND the search query, sorted by
                    // updated_at desc. Empty query → behaves exactly as before.
                    let needle = search_query.get().trim().to_lowercase();
                    let mut filtered: Vec<SessionEntry> = session_list
                        .into_iter()
                        .filter(|s| sel_agent.as_deref() == Some(&s.agent_id))
                        .filter(|s| {
                            if needle.is_empty() {
                                true
                            } else {
                                let hay = s
                                    .topic
                                    .as_deref()
                                    .unwrap_or(&s.key)
                                    .to_lowercase();
                                hay.contains(&needle)
                            }
                        })
                        .collect();
                    filtered.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

                    if filtered.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                {move || t_string!(i18n, chat.no_conversations).to_string()}
                            </p>
                        }.into_any();
                    }

                    let on_select = on_select_session;
                    let do_rename = do_rename.clone();
                    let do_delete = do_delete.clone();
                    view! {
                        <div class="space-y-0.5">
                            {filtered
                                .into_iter()
                                .map(|session| {
                                    let key = session.key.clone();
                                    let session_agent_id = session.agent_id.clone();
                                    let is_active = {
                                        let key = key.clone();
                                        move || {
                                            chat.session_key.get().as_deref() == Some(&key)
                                        }
                                    };
                                    let sk_for_dot = session.key.clone();
                                    // Dot = pure server-authoritative: reads server_running only
                                    // (fed by RunningSetChanged; runs from any interface included).
                                    // See `SessionMap::is_running_session_key`.
                                    let is_running_row =
                                        move || session_map.is_running_session_key(&sk_for_dot);
                                    let label = session
                                        .topic
                                        .clone()
                                        .unwrap_or_else(|| t_string!(i18n, chat.new_chat).to_string());
                                    let subtitle = format_session_subtitle(&session);
                                    // Mode facet: only an explicit per-session
                                    // override earns a badge — follow-global
                                    // rows stay clean (the global default is
                                    // not a per-row fact worth repeating).
                                    let mode_badge = session.mode.clone();
                                    let do_rename = do_rename.clone();
                                    let do_delete = do_delete.clone();

                                    // Determine which mode this session row is in
                                    let is_editing = _editing.as_deref() == Some(&key);
                                    let is_deleting = _deleting.as_deref() == Some(&key);
                                    let is_menu_open = _menu.as_deref() == Some(&key);

                                    if is_editing {
                                        // --- Edit mode ---
                                        let key_for_save = key.clone();
                                        let key_for_save2 = key;
                                        let do_rename_keydown = do_rename.clone();
                                        let do_rename_blur = do_rename;
                                        view! {
                                            <div class="w-full px-3 py-2 rounded-lg bg-surface-sunken border border-primary/40">
                                                <input
                                                    node_ref=edit_input_ref
                                                    class="w-full bg-transparent text-xs text-text-primary outline-none disabled:opacity-50"
                                                    prop:value=move || edit_text.get()
                                                    prop:disabled=move || is_saving.get()
                                                    maxlength=100
                                                    on:input=move |ev| {
                                                        edit_text.set(event_target_value(&ev));
                                                    }
                                                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                        let k = ev.key();
                                                        if k == "Enter" {
                                                            let text = edit_text.get_untracked();
                                                            if text.trim().is_empty() {
                                                                editing_key.set(None);
                                                                edit_text.set(String::new());
                                                            } else {
                                                                do_rename_keydown(key_for_save.clone(), text);
                                                            }
                                                        } else if k == "Escape" {
                                                            editing_key.set(None);
                                                            edit_text.set(String::new());
                                                        }
                                                    }
                                                    on:blur=move |_| {
                                                        // Small delay to allow Enter keydown to fire first
                                                        let key_c = key_for_save2.clone();
                                                        let do_rename_c = do_rename_blur.clone();
                                                        leptos::task::spawn_local(async move {
                                                            gloo_timers::future::TimeoutFuture::new(100).await;
                                                            // Same blur-then-unmount ordering as the group
                                                            // row above.
                                                            let Some(cur) = editing_key.try_get_untracked()
                                                            else {
                                                                return;
                                                            };
                                                            if cur.as_deref() == Some(&key_c) {
                                                                let Some(text) = edit_text.try_get_untracked()
                                                                else {
                                                                    return;
                                                                };
                                                                if text.trim().is_empty() {
                                                                    editing_key.set(None);
                                                                    edit_text.set(String::new());
                                                                } else {
                                                                    do_rename_c(key_c, text);
                                                                }
                                                            }
                                                        });
                                                    }
                                                />
                                            </div>
                                        }.into_any()
                                    } else if is_deleting {
                                        // --- Delete-confirm mode ---
                                        let key_for_del = key;
                                        view! {
                                            <div
                                                tabindex=0
                                                class="w-full px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30
                                                        flex items-center justify-between text-xs outline-none"
                                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                    if ev.key() == "Escape" {
                                                        clear_action_states();
                                                    }
                                                }
                                            >
                                                <span class="text-red-400 font-medium">{move || t_string!(i18n, chat.confirm_delete).to_string()}</span>
                                                <div class="flex items-center gap-1.5">
                                                    <button
                                                        class="px-2 py-0.5 rounded bg-red-500 text-white text-[10px] font-medium
                                                               hover:bg-red-600 transition-colors disabled:opacity-50"
                                                        prop:disabled=move || is_saving.get()
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            do_delete(key_for_del.clone());
                                                        }
                                                    >
                                                        {move || t_string!(i18n, common.confirm).to_string()}
                                                    </button>
                                                    <button
                                                        class="px-2 py-0.5 rounded bg-surface-sunken text-text-secondary text-[10px]
                                                               hover:bg-surface-raised transition-colors"
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            clear_action_states();
                                                        }
                                                    >
                                                        {move || t_string!(i18n, common.cancel).to_string()}
                                                    </button>
                                                </div>
                                            </div>
                                        }.into_any()
                                    } else {
                                        // --- Normal mode ---
                                        let key_for_click = key.clone();
                                        let key_for_menu = key.clone();
                                        let key_for_edit = key.clone();
                                        let key_for_del_menu = key.clone();
                                        let label_for_edit = label.clone();
                                        view! {
                                            <div class="relative group">
                                                <button
                                                    class=move || format!(
                                                        "w-full text-left px-3 py-1.5 rounded-lg text-sm flex items-center justify-between {}",
                                                        if is_active() {
                                                            "nav-tile-active"
                                                        } else {
                                                            "nav-tile"
                                                        }
                                                    )
                                                    on:click=move |_| {
                                                        clear_action_states();
                                                        on_select(
                                                            key_for_click.clone(),
                                                            session_agent_id.clone(),
                                                        );
                                                    }
                                                >
                                                    <div class="flex-1 min-w-0">
                                                        <div class="flex items-center gap-1.5">
                                                            <Show when=is_running_row>
                                                                <span class="w-1.5 h-1.5 rounded-full bg-danger animate-pulse shrink-0 mr-1.5" />
                                                            </Show>
                                                            <div class="truncate font-medium text-xs">
                                                                {label}
                                                            </div>
                                                            {mode_badge.map(|m| view! {
                                                                <span class="shrink-0 px-1 py-px rounded border border-border
                                                                             text-[9px] font-mono text-text-tertiary uppercase">
                                                                    {m}
                                                                </span>
                                                            })}
                                                        </div>
                                                        <div class="truncate text-[10px] text-text-tertiary mt-0.5">
                                                            {subtitle}
                                                        </div>
                                                    </div>
                                                    // ⋯ button (visible on hover)
                                                    <button
                                                        class="opacity-0 group-hover:opacity-100 ml-1 px-1.5 py-0.5
                                                               rounded text-text-tertiary hover:text-text-primary
                                                               hover:bg-surface-raised transition-all text-xs flex-shrink-0"
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            let current = menu_open_key.get_untracked();
                                                            if current.as_deref() == Some(&key_for_menu) {
                                                                menu_open_key.set(None);
                                                            } else {
                                                                clear_action_states();
                                                                menu_open_key.set(Some(key_for_menu.clone()));
                                                            }
                                                        }
                                                    >
                                                        "⋯"
                                                    </button>
                                                </button>
                                                // Dropdown menu
                                                {if is_menu_open {
                                                    view! {
                                                        <div class="glass absolute right-0 top-full mt-1 z-50 min-w-[120px]
                                                                    bg-surface-overlay/85 border border-border rounded-lg shadow-xl
                                                                    py-1 text-xs">
                                                            <button
                                                                class="w-full text-left px-3 py-1.5 text-text-secondary
                                                                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                                                                on:click=move |ev: web_sys::MouseEvent| {
                                                                    ev.stop_propagation();
                                                                    menu_open_key.set(None);
                                                                    edit_text.set(label_for_edit.clone());
                                                                    editing_key.set(Some(key_for_edit.clone()));
                                                                }
                                                            >
                                                                {move || t_string!(i18n, chat.rename).to_string()}
                                                            </button>
                                                            <button
                                                                class="w-full text-left px-3 py-1.5 text-red-400
                                                                       hover:bg-red-500/10 transition-colors"
                                                                on:click=move |ev: web_sys::MouseEvent| {
                                                                    ev.stop_propagation();
                                                                    menu_open_key.set(None);
                                                                    deleting_key.set(Some(key_for_del_menu.clone()));
                                                                }
                                                            >
                                                                {move || t_string!(i18n, common.delete).to_string()}
                                                            </button>
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    view! { <span /> }.into_any()
                                                }}
                                            </div>
                                        }.into_any()
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </div>
                    }
                    .into_any()
                }}
            </div>

            // Bottom status bar — gateway state + active run count.
            <crate::components::sidebar::SessionStatusBar />
        </div>
    }
}

fn format_session_subtitle(session: &SessionEntry) -> String {
    let msg_count = session.message_count;
    match session.updated_at {
        Some(ts) => {
            // Format Unix epoch seconds as MM-DD using js_sys::Date (WASM-safe)
            let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
            let month = date.get_month() + 1; // 0-based in JS
            let day = date.get_date();
            format!("{msg_count} msgs - {month:02}-{day:02}")
        }
        None => format!("{msg_count} messages"),
    }
}

#[cfg(test)]
mod team_history_tests {
    use super::{team_history_item_to_message, RESERVED_USER_HANDLE};
    use crate::api::team_chat::TeamMessageItem;

    fn item(from_agent: &str) -> TeamMessageItem {
        kinded(from_agent, "agent")
    }

    fn kinded(from_agent: &str, kind: &str) -> TeamMessageItem {
        TeamMessageItem {
            from_agent: from_agent.to_string(),
            content: "hello".to_string(),
            msg_type: "message".to_string(),
            kind: kind.to_string(),
            created_at: 0,
        }
    }

    #[test]
    fn user_handle_replays_as_right_aligned_user_bubble() {
        // Regression: the user's own group-chat messages were replayed as
        // left-aligned agent bubbles (role "assistant" + agent_id Some("user")).
        // They must mirror single chat: role "user", no agent_id → right-aligned
        // accent bubble.
        let m = team_history_item_to_message(0, kinded(RESERVED_USER_HANDLE, "user"));
        assert_eq!(m.role, "user");
        assert_eq!(m.agent_id, None);
    }

    #[test]
    fn legacy_core_without_kind_still_splits_own_messages() {
        // `kind` defaults to "agent" against an older core; the handle fallback
        // is what keeps the user's own replayed rows right-aligned.
        let m = team_history_item_to_message(0, item(RESERVED_USER_HANDLE));
        assert_eq!(m.role, "user");
        assert_eq!(m.agent_id, None);
    }

    #[test]
    fn agent_handle_replays_as_attributed_agent_bubble() {
        let m = team_history_item_to_message(3, item("risk_analyst"));
        assert_eq!(m.role, "assistant");
        assert_eq!(m.agent_id.as_deref(), Some("risk_analyst"));
        assert_eq!(m.id, "team-hist-3");
    }

    #[test]
    fn system_kind_replays_as_unattributed_notice_row() {
        // A broadcaster notice ("depth cap reached, your turn") must replay as
        // the same centered chip it showed live — not as a bubble from an agent
        // literally named "system".
        let m = team_history_item_to_message(1, kinded("system", "system"));
        assert_eq!(m.role, "system");
        assert_eq!(m.agent_id, None);
    }
}

#[cfg(test)]
mod gauge_tests {
    use super::occupancy_from_history;
    use crate::api::chat::ChatMessage;

    fn user(content: &str) -> ChatMessage {
        ChatMessage {
            role: "user".into(),
            content: content.into(),
            run_id: None,
            timestamp: None,
            metadata: None,
            context_tokens: None,
            context_window: None,
            total_tokens: None,
            author_user_id: None,
        }
    }

    fn assistant(used: Option<u32>, window: Option<u32>, total: Option<u64>) -> ChatMessage {
        ChatMessage {
            role: "assistant".into(),
            content: "ok".into(),
            run_id: Some("r".into()),
            timestamp: None,
            metadata: None,
            context_tokens: used,
            context_window: window,
            total_tokens: total,
            author_user_id: None,
        }
    }

    #[test]
    fn none_when_no_turn_carries_occupancy() {
        // Pre-change sessions (or no-LLM turns) leave the gauge hidden.
        let h = vec![user("hi"), assistant(None, None, None)];
        assert!(occupancy_from_history(&h).is_none());
    }

    #[test]
    fn picks_latest_assistant_occupancy() {
        // Two completed turns → the most recent occupancy wins.
        let h = vec![
            user("a"),
            assistant(Some(10_000), Some(200_000), Some(12_000)),
            user("b"),
            assistant(Some(42_000), Some(200_000), Some(55_000)),
        ];
        let u = occupancy_from_history(&h).expect("gauge");
        assert_eq!(u.used_tokens, 42_000);
        assert_eq!(u.window_tokens, 200_000);
        assert_eq!(u.total_tokens, 55_000);
    }

    #[test]
    fn real_occupancy_is_not_marked_estimate() {
        let h = vec![assistant(Some(10_000), Some(200_000), Some(12_000))];
        let u = occupancy_from_history(&h).expect("real occupancy present");
        assert!(
            !u.is_estimate,
            "history-persisted occupancy is real, not an estimate"
        );
    }

    #[test]
    fn skips_partial_or_zero_rows() {
        // A trailing row missing the window, or with a zero, falls back to the
        // last fully-populated turn rather than showing a broken denominator.
        let h = vec![
            assistant(Some(30_000), Some(200_000), Some(31_000)),
            assistant(Some(40_000), None, Some(41_000)),
            assistant(Some(0), Some(200_000), Some(0)),
        ];
        let u = occupancy_from_history(&h).expect("gauge");
        assert_eq!(u.used_tokens, 30_000);
        assert_eq!(u.window_tokens, 200_000);
    }
}

#[cfg(test)]
mod rehydrate_tests {
    use super::session_update_needs_rehydrate;

    const MINE: &str = "run-this-tab-sent-it";
    const THEIRS: &str = "run-someone-else-sent-it";

    fn started_here(run: &str) -> bool {
        run == MINE
    }

    /// The defect this predicate replaces: `origin_channel` is `"gui:chat"` for
    /// BOTH of these frames, so the old channel-class test answered "mine" to
    /// both and a second member's turn never reached the transcript. The two
    /// assertions must disagree while the channel is held constant — that is the
    /// whole claim, and either one alone would still pass under the old rule.
    #[test]
    fn gui_chat_alone_no_longer_decides_it() {
        assert!(
            !session_update_needs_rehydrate("gui:chat", Some(MINE), started_here),
            "our own run must not reload over the transcript we just streamed"
        );
        assert!(
            session_update_needs_rehydrate("gui:chat", Some(THEIRS), started_here),
            "another Panel's run on this session must reload — same channel literal"
        );
    }

    /// An update no run caused (topic rename, title, lifecycle) leaves the
    /// transcript alone. This arm is deliberately KEPT: those frames publish
    /// with no origin at all.
    #[test]
    fn an_update_no_run_caused_is_not_a_rehydrate() {
        assert!(!session_update_needs_rehydrate("", None, started_here));
        // Belt and braces: an absent channel wins even if a run id leaked in.
        assert!(!session_update_needs_rehydrate(
            "",
            Some(THEIRS),
            started_here
        ));
    }

    /// External surfaces keep working: they carry their real channel id, and
    /// their run was never started here.
    #[test]
    fn an_external_surface_still_mirrors() {
        assert!(session_update_needs_rehydrate(
            "telegram",
            Some(THEIRS),
            started_here
        ));
    }

    /// Against a core that predates `origin_run_id`, an unidentifiable run
    /// counts as somebody else's — the safer half of that version skew.
    #[test]
    fn an_unidentified_run_counts_as_someone_elses() {
        assert!(session_update_needs_rehydrate(
            "gui:chat",
            None,
            started_here
        ));
    }
}
