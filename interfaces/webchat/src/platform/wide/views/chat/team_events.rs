//! Project `team.<id>.*` topic events onto team-chat ChatState: attributed
//! message bubbles, in-chat system notices, roster live status, task upserts,
//! and the fan-out tree's run lifecycle. Parallel to
//! `events.rs::subscribe_run_events` (single-agent), kept separate for
//! zero-regression of the single-agent path.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::state::{ChatMessage, ChatPhase, ChatState, MemberStatus};
use crate::api::teams::{TaskFilter, TeamsApi};
use crate::context::{DashboardState, GatewayEvent};

/// Which `team.<id>.<kind>` event arrived. `Task` folds the 4-segment
/// `team.<id>.task.<verb>` family — the verb is redundant with the payload's
/// `status`, which is what the upsert actually reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamTopicKind {
    /// A member's deliverable reply → an attributed bubble.
    Message,
    /// A broadcaster notice (guard explanation, member failure) → system chip.
    System,
    /// Member presence transition → roster status dot.
    Activity,
    /// Coordination-task create/update → task strip + drawer.
    Task,
    /// Fan-out tree lifecycle (`started` / `settled`) → run id + phase.
    Fanout,
}

/// Split a `team.<team_id>.<kind>` topic into its team id and kind.
///
/// Team ids are opaque and may contain dots, so the kind is matched as a
/// **suffix** rather than by positional split. Returns `None` for anything that
/// is not a per-team topic — notably `team.changed`, the global 2-segment
/// team-list invalidation the sidebar listens to.
#[must_use]
pub fn parse_team_topic(topic: &str) -> Option<(&str, TeamTopicKind)> {
    let rest = topic.strip_prefix("team.")?;
    const SUFFIXES: [(&str, TeamTopicKind); 4] = [
        (".message", TeamTopicKind::Message),
        (".system", TeamTopicKind::System),
        (".activity", TeamTopicKind::Activity),
        (".fanout", TeamTopicKind::Fanout),
    ];
    for (suffix, kind) in SUFFIXES {
        if let Some(team_id) = rest.strip_suffix(suffix) {
            return (!team_id.is_empty()).then_some((team_id, kind));
        }
    }
    // `team.<id>.task.<verb>` — the verb is free-form, so anchor on the
    // `.task.` separator instead of a fixed suffix.
    let idx = rest.rfind(".task.")?;
    let team_id = &rest[..idx];
    (!team_id.is_empty()).then_some((team_id, TeamTopicKind::Task))
}

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
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_per_team_topic_kind() {
        assert_eq!(
            parse_team_topic("team.t1.message"),
            Some(("t1", TeamTopicKind::Message))
        );
        assert_eq!(
            parse_team_topic("team.t1.system"),
            Some(("t1", TeamTopicKind::System))
        );
        assert_eq!(
            parse_team_topic("team.t1.activity"),
            Some(("t1", TeamTopicKind::Activity))
        );
        assert_eq!(
            parse_team_topic("team.t1.fanout"),
            Some(("t1", TeamTopicKind::Fanout))
        );
        assert_eq!(
            parse_team_topic("team.t1.task.created"),
            Some(("t1", TeamTopicKind::Task))
        );
    }

    #[test]
    fn ignores_the_global_team_changed_topic() {
        // The sidebar's team-list invalidation shares the `team.` prefix but is
        // not per-team — projecting it would push an empty bubble.
        assert_eq!(parse_team_topic("team.changed"), None);
    }

    #[test]
    fn ignores_foreign_and_malformed_topics() {
        assert_eq!(parse_team_topic("stream.response_chunk"), None);
        assert_eq!(parse_team_topic("team."), None);
        assert_eq!(parse_team_topic("team..message"), None);
        assert_eq!(parse_team_topic("team.t1.unknown"), None);
    }

    #[test]
    fn team_id_may_contain_dots() {
        // Ids are opaque; suffix matching (not positional split) keeps a dotted
        // id addressable instead of silently misrouting its events.
        assert_eq!(
            parse_team_topic("team.acme.eu.west.message"),
            Some(("acme.eu.west", TeamTopicKind::Message))
        );
        assert_eq!(
            parse_team_topic("team.acme.eu.west.task.updated"),
            Some(("acme.eu.west", TeamTopicKind::Task))
        );
    }
}
