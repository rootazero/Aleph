//! Entering a team's group chat — the one path that turns a `team_id` into
//! the singleton [`ChatState`] showing that team's transcript.
//!
//! This used to be a closure inside `ChatSidebar`, reachable only from the
//! sidebar's group list. The project room's Kanban tab needs the same action
//! for a room-scoped team, and the alternative to extracting it was a second
//! copy of the same four steps — which is how two surfaces end up disagreeing
//! about what "open the group chat" means (does it clear the session? does it
//! replay history? does it drop the unread badge?).
//!
//! The badge is deliberately NOT part of this function. Dropping an unread
//! marker is the *sidebar's* business — it owns that signal, and the Kanban
//! tab has no badge to drop — so the caller does it before calling in. What
//! lives here is only what every entry point must do identically.

use std::collections::HashMap;

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::team_chat::{TeamChatApi, TeamMessageItem};
use crate::api::teams::TeamsApi;
use crate::context::DashboardState;
use crate::platform::wide::views::chat::state::{
    ChatMessage, ChatState, MemberStatus, TeamMemberView,
};

/// An agent entry returned by the backend (`agents.list`).
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AgentEntry {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) emoji: Option<String>,
    #[serde(default)]
    pub(crate) is_default: bool,
}

/// The `from_agent` handle a human's own group message carries (mirror of
/// `teams::broadcast::RESERVED_USER_HANDLE`).
pub(crate) const RESERVED_USER_HANDLE: &str = "user";

/// Map one replayed `teams.chat.history` item to a chat bubble.
///
/// The render class comes from the server's `kind` (`user` | `agent` |
/// `system`) — one classification, derived once, next to the store that knows
/// the message's recipients and type. `from_agent` is only consulted as the
/// pre-`kind` fallback so a Panel pointed at an older core still splits its own
/// messages out of the agent bubbles. `index` seeds a stable dom id.
pub(crate) fn team_history_item_to_message(index: usize, item: TeamMessageItem) -> ChatMessage {
    let role = match item.kind.as_str() {
        "user" => "user",
        "system" => "system",
        // Legacy core (no `kind`, defaulted to "agent"): fall back to the
        // handle check so own messages don't replay as agent bubbles.
        _ if item.from_agent == RESERVED_USER_HANDLE => "user",
        _ => "assistant",
    };
    // Only a "user" row can carry a human author (spec §6.2 P3 — `None` for
    // agent/system rows, and for a Panel pointed at an older core that never
    // sent this field). Reuses the SAME `ChatMessage.author_user_id` field
    // and `MessageBubble` rendering a P2 project-room peer message gets:
    // `messages.rs`'s `author_label` suppresses the label for the viewer's
    // own id and otherwise resolves it via `UserDirectoryState`.
    let author_user_id = (role == "user").then_some(item.author_user_id).flatten();
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
        // `teams.chat.history` is a legacy group-broadcast surface, but as of
        // P3 §6.2 humanization a "user" row now carries the real speaker's
        // `author_user_id` (server-resolved from `TeamMessage::author_user_id`
        // — see `map_history`), not a P2 project-room author.
        author_user_id,
    }
}

/// Enter team-chat mode for `team_id`: fetch the roster, switch the singleton
/// [`ChatState`] into team mode, and replay the durable transcript.
///
/// `known_agents` is the caller's already-loaded `agents.list` result, used
/// only to resolve display names and emoji. `None` means "I don't have one" —
/// the roster still renders, falling back to raw agent ids, rather than this
/// function issuing a second list call on a path that may not need it. A name
/// that renders as its id is a cosmetic loss; a failed extra round trip on
/// every board click is not.
///
/// The `team.*` subscription is NOT established here: `ChatView` holds it
/// permanently, and a second subscriber would unsubscribe the shared topic on
/// unmount and take the always-mounted view's event stream down with it.
pub(crate) async fn enter_team_chat(
    dash: DashboardState,
    chat: ChatState,
    known_agents: Option<Vec<AgentEntry>>,
    team_id: String,
) {
    // 1. Fetch team detail (members list).
    let detail = match TeamsApi::get(&dash, &team_id).await {
        Ok(d) => d,
        Err(e) => {
            web_sys::console::error_1(&format!("teams.get failed: {e}").into());
            return;
        }
    };
    // 2. Build id→AgentEntry map for name/emoji resolution.
    let agent_map: HashMap<String, AgentEntry> = known_agents
        .unwrap_or_default()
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();
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
            author_user_id: None,
            author_display_name: None,
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
    fn user_kind_carries_the_human_authors_id_for_the_label() {
        // Spec §6.2 humanization (P3): a "user" history row now carries the
        // real speaker's raw id, so `MessageBubble` can resolve and show it
        // — same field, same styling, a P2 room peer message gets.
        let mut it = kinded(RESERVED_USER_HANDLE, "user");
        it.author_user_id = Some("u-alice".to_string());
        it.author_display_name = Some("Alice".to_string());
        let m = team_history_item_to_message(0, it);
        assert_eq!(m.role, "user");
        assert_eq!(m.author_user_id, Some("u-alice".to_string()));
    }

    #[test]
    fn agent_kind_never_carries_an_author_user_id() {
        // Even if a future core somehow set it, an agent bubble must not
        // pick up human attribution — that field means something entirely
        // different for the two roles.
        let mut it = item("risk_analyst");
        it.author_user_id = Some("u-alice".to_string());
        let m = team_history_item_to_message(0, it);
        assert_eq!(m.role, "assistant");
        assert_eq!(m.author_user_id, None);
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
