//! Project `team.<id>.*` topic events onto team-chat ChatState: attributed
//! message bubbles + roster live status. Parallel to `events.rs::subscribe_run_events`
//! (single-agent), kept separate for zero-regression of the single-agent path.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::state::{ChatMessage, ChatState, MemberStatus};
use crate::api::teams::{TaskFilter, TeamsApi};
use crate::context::{DashboardState, GatewayEvent};

/// Stable per-agent color by roster slot index. Used by the workspace
/// deliverables panel (`workspace_panel.rs`) where only the roster position is
/// available. Attribution bubbles in `messages.rs` now use `agent_color_for_id`
/// (id-hashed, session-stable).
#[must_use]
pub fn agent_color(index: usize) -> &'static str {
    const PALETTE: [&str; 6] = [
        "#7c9cff", "#4ec9b0", "#e0a458", "#c586c0", "#4fc1ff", "#d16969",
    ];
    PALETTE[index % PALETTE.len()]
}

/// Subscribe to `team.*` events and project them onto team-chat state. Returns
/// the subscription id for cleanup (caller `unsubscribe_events` on teardown).
#[must_use]
pub fn subscribe_team_events(dashboard: &DashboardState, chat: ChatState) -> usize {
    let dash = *dashboard;
    let refetch_gen = StoredValue::new(0u32);
    dashboard.subscribe_events(move |event: GatewayEvent| {
        if !event.topic.starts_with("team.") {
            return;
        }
        let data = &event.data;
        let agent_id = data
            .get("agent_id")
            .and_then(|a| a.as_str())
            .unwrap_or("")
            .to_string();

        if event.topic.ends_with(".message") {
            let Some(text) = data.get("text").and_then(|t| t.as_str()) else {
                return;
            };
            let seq = chat.messages.with_untracked(|m| m.len());
            chat.messages.update(|msgs| {
                msgs.push(ChatMessage {
                    id: format!("team-{seq}"),
                    role: "assistant".to_string(),
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
                    agent_id: Some(agent_id),
                    plan_archive: None,
                });
            });
        } else if event.topic.ends_with(".activity") {
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
        } else if event.topic.contains(".task.") {
            // 4-segment topic `team.<id>.task.<verb>`; payload carries
            // {task_id, status, ...} but NO subject. Upsert status in place for
            // known tasks; refetch the list for an unknown id (new task needs
            // its subject). Idempotent + order-independent: unknown/terminal/
            // deleted ids are tolerated.
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
            } else if let Some(team_id) = chat.team_id.get_untracked() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_color_is_stable_per_index() {
        assert_eq!(agent_color(0), agent_color(0));
        assert_ne!(agent_color(0), agent_color(1));
    }

    #[test]
    fn agent_color_wraps_palette() {
        // index beyond palette length wraps (no panic, deterministic).
        assert_eq!(agent_color(0), agent_color(6));
    }
}
