//! Project `team.<id>.*` topic events onto team-chat ChatState: attributed
//! message bubbles, in-chat system notices, roster live status, task upserts,
//! and the fan-out tree's run lifecycle. Parallel to
//! `events.rs::subscribe_run_events` (single-agent), kept separate for
//! zero-regression of the single-agent path.
//!
//! The topic grammar itself is NOT this module's — it lives in
//! `aleph_protocol::team_topic`, which the server reads too (its
//! `event_visibility::session_identity_of` decides from the same team id
//! whether a frame reaches this connection at all). This module is the
//! rendering consumer of that grammar; the tests for it live beside it.

use aleph_protocol::team_topic::{parse_team_topic, TeamTopicKind};
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::state::{ChatMessage, ChatPhase, ChatState, MemberStatus};
use crate::api::teams::{TaskFilter, TeamsApi};
use crate::context::{DashboardState, GatewayEvent};

/// Subscribe to `team.*` events and project them onto team-chat state. Returns
/// the subscription id for cleanup (caller `unsubscribe_events` on teardown).
#[must_use]
pub fn subscribe_team_events(dashboard: &DashboardState, chat: ChatState) -> usize {
    let dash = *dashboard;
    let refetch_gen = StoredValue::new(0u32);
    dashboard.subscribe_events(move |event: GatewayEvent| {
        let Some((topic_team, kind)) = parse_team_topic(&event.topic) else {
            return;
        };
        // Hard scope to the team the user is actually looking at. The Gateway
        // subscription is the `team.*` wildcard, so WITHOUT this every team the
        // daemon runs — a Telegram-triggered group, a second Panel tab, a
        // background dispatcher round — pushed its bubbles and roster
        // transitions into whatever conversation happened to be open,
        // single-agent chats included.
        if chat.team_id.get_untracked().as_deref() != Some(topic_team) {
            return;
        }
        let data = &event.data;
        let agent_id = data
            .get("agent_id")
            .and_then(|a| a.as_str())
            .unwrap_or("")
            .to_string();

        match kind {
            TeamTopicKind::Message => {
                let Some(text) = data.get("text").and_then(|t| t.as_str()) else {
                    return;
                };
                push_bubble(chat, "assistant", text, Some(agent_id));
            }
            TeamTopicKind::System => {
                let Some(text) = data.get("text").and_then(|t| t.as_str()) else {
                    return;
                };
                // No `agent_id`: a system notice is nobody's turn, so it must
                // not join the Telegram-style attribution grouping.
                push_bubble(chat, "system", text, None);
            }
            TeamTopicKind::Activity => {
                let status = match data.get("status").and_then(|s| s.as_str()) {
                    Some("working") => MemberStatus::Working,
                    Some("done") => MemberStatus::Done,
                    Some("error") => MemberStatus::Error,
                    _ => MemberStatus::Idle,
                };
                chat.team_members.update(|members| {
                    if let Some(m) = members.iter_mut().find(|m| m.agent_id == agent_id) {
                        m.status = status;
                    }
                });
            }
            TeamTopicKind::Fanout => {
                // The tree's run id is the handle `teams.chat.cancel` takes, so
                // parking it in `active_run_id` is what gives group chat a Stop
                // button — and, for free, the busy→idle edge the composer's
                // queue auto-drain already watches.
                let run_id = data.get("run_id").and_then(|v| v.as_str()).unwrap_or("");
                match data.get("status").and_then(|s| s.as_str()) {
                    Some("started") if !run_id.is_empty() => {
                        chat.active_run_id.set(Some(run_id.to_string()));
                        chat.phase.set(ChatPhase::Thinking);
                    }
                    Some("settled") => {
                        // Only the tree we are actually tracking may clear the
                        // slot: a stale settle from a previous fan-out would
                        // otherwise blank the Stop button of the live one.
                        if chat.active_run_id.get_untracked().as_deref() == Some(run_id) {
                            chat.active_run_id.set(None);
                            chat.phase.set(ChatPhase::Idle);
                        }
                        // Every member of a settled tree is idle by definition;
                        // `RunComplete` only marks the ones that produced a
                        // deliverable, so without this a silent member keeps a
                        // stale amber dot forever.
                        chat.team_members.update(|members| {
                            for m in members.iter_mut() {
                                if m.status == MemberStatus::Working {
                                    m.status = MemberStatus::Idle;
                                }
                            }
                        });
                    }
                    _ => {}
                }
            }
            TeamTopicKind::Task => {
                // Payload carries {task_id, status, ...} but NO subject. Upsert
                // status in place for known tasks; refetch the list for an
                // unknown id (a new task needs its subject). Idempotent +
                // order-independent: unknown/terminal/deleted ids are tolerated.
                let task_id = data
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if task_id.is_empty() {
                    return;
                }
                let status = data
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let known = chat
                    .team_tasks
                    .with_untracked(|ts| ts.iter().any(|t| t.id == task_id));
                if known {
                    chat.team_tasks.update(|ts| {
                        if let Some(t) = ts.iter_mut().find(|t| t.id == task_id) {
                            if let Some(s) = status {
                                t.status = s;
                            }
                        }
                    });
                    return;
                }
                let team_id = topic_team.to_string();
                // Debounce: coalesce a burst of unknown-id task events into ONE
                // refetch after the burst settles (~250ms). Each event bumps the
                // generation; only the latest generation's delayed task fetches.
                let my_gen = refetch_gen.with_value(|g| g.wrapping_add(1));
                refetch_gen.set_value(my_gen);
                let chat2 = chat;
                spawn_local(async move {
                    gloo_timers::future::TimeoutFuture::new(250).await;
                    if refetch_gen.with_value(|g| *g) != my_gen {
                        return; // superseded by a newer task event
                    }
                    if let Ok(tasks) =
                        TeamsApi::list_tasks(&dash, &team_id, TaskFilter::default()).await
                    {
                        chat2.team_tasks.set(tasks);
                    }
                });
            }
        }
    })
}

/// Append one non-streaming bubble to the transcript. `agent_id` is `Some` for
/// attributed member replies and `None` for system notices.
fn push_bubble(chat: ChatState, role: &str, text: &str, agent_id: Option<String>) {
    let seq = chat.messages.with_untracked(|m| m.len());
    chat.messages.update(|msgs| {
        msgs.push(ChatMessage {
            id: format!("team-{role}-{seq}"),
            role: role.to_string(),
            content: text.to_string(),
            tool_calls: Vec::new(),
            is_streaming: false,
            is_intermediate: false,
            error: None,
            model_info: None,
            timestamp: Some(super::timeline::now_millis()),
            iteration: None,
            is_final: true,
            text_finalized: true,
            agent_id,
            plan_archive: None,
            // Legacy group broadcast (not a P2 project room).
            author_user_id: None,
        });
    });
}
