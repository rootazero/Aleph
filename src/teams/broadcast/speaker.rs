//! The single derivation of "what label does this row speak under" (spec §6.2
//! humanization, P2). Every human-authored `TeamMessage` row is stored with
//! `from_agent == RESERVED_USER_HANDLE` regardless of who actually spoke — the raw
//! `from_agent` cannot distinguish Alice from Bob, which is exactly why this module
//! exists. The real speaker lives in `author_user_id`; this module resolves it to a
//! display name and hands the caller the one string a transcript row (or any other
//! per-speaker rendering) should show.
//!
//! Two shapes share the same underlying lookup (`resolve_display_name`):
//! - [`resolve_labels`] batch-resolves every distinct human author present in a
//!   transcript history, for [`speaker_label`] to consult row-by-row
//!   (`GroupChatBroadcaster::run_member`'s use — a history, not a single author).
//! - [`resolve_display_name`] resolves one author id directly, for callers that only
//!   ever have one author in hand at a time (`teams.chat.send`'s realtime fan-out
//!   broadcast; a future room-session transcript projection is expected to reuse this
//!   same primitive rather than growing a second lookup).
//!
//! Degradation (P7): a `None` store, a store error, or an author with no display name
//! set all fall back to the raw user id. A display-name lookup must never block or fail
//! a run — the group chat keeps working, just under raw ids instead of names.

use std::collections::HashMap;

use crate::gateway::security::SecurityStore;
use crate::sync_primitives::Arc;
use crate::teams::messages::TeamMessage;

/// The label a transcript row speaks under.
///
/// Human row (`author_user_id` present): the resolved display name from `labels`, or
/// the raw user id when `labels` has no entry for it (store was absent/erred, or the
/// user has no display name). Agent/system row (`author_user_id: None`): `from_agent`,
/// unchanged — this is the entire pre-humanization behavior, preserved byte-for-byte.
#[must_use]
pub fn speaker_label(msg: &TeamMessage, labels: &HashMap<String, String>) -> String {
    match &msg.author_user_id {
        Some(user_id) => labels
            .get(user_id)
            .cloned()
            .unwrap_or_else(|| user_id.clone()),
        None => msg.from_agent.clone(),
    }
}

/// Resolve one human's display-name label, falling back to the raw user id when the
/// store has no row for it (or none was ever set). Never blocks a caller on identity
/// lookup (P7) — a `SecurityStore` read failure is not surfaced here, it degrades the
/// same way a miss does.
#[must_use]
pub fn resolve_display_name(store: &SecurityStore, user_id: &str) -> String {
    store
        .get_user(user_id)
        .ok()
        .flatten()
        .map(|u| u.display_name)
        .unwrap_or_else(|| user_id.to_string())
}

/// Batch-resolve display names for every distinct human author present in `history`.
///
/// `store: None` (unavailable to this caller) degrades to an empty map — every row's
/// label then falls back to its raw user id through [`speaker_label`], never blocking
/// the run. Agent/system rows (`author_user_id: None`) contribute no entry.
#[must_use]
pub fn resolve_labels(
    store: Option<&Arc<SecurityStore>>,
    history: &[TeamMessage],
) -> HashMap<String, String> {
    let Some(store) = store else {
        return HashMap::new();
    };
    let mut labels = HashMap::new();
    for msg in history {
        if let Some(user_id) = &msg.author_user_id {
            labels
                .entry(user_id.clone())
                .or_insert_with(|| resolve_display_name(store, user_id));
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::UserRole;
    use crate::teams::messages::MessageType;

    fn human_msg(author: &str) -> TeamMessage {
        TeamMessage {
            id: "m1".to_string(),
            team_id: "team-x".to_string(),
            from_agent: crate::teams::broadcast::RESERVED_USER_HANDLE.to_string(),
            msg_type: MessageType::Message,
            subject: String::new(),
            content: "hi".to_string(),
            recipients: Vec::new(),
            reply_to: None,
            thread_id: None,
            attachments: Vec::new(),
            created_at: chrono::Utc::now(),
            expires_at: None,
            author_user_id: Some(author.to_string()),
        }
    }

    fn agent_msg(from_agent: &str) -> TeamMessage {
        TeamMessage {
            id: "m2".to_string(),
            team_id: "team-x".to_string(),
            from_agent: from_agent.to_string(),
            msg_type: MessageType::Message,
            subject: String::new(),
            content: "hi".to_string(),
            recipients: Vec::new(),
            reply_to: None,
            thread_id: None,
            attachments: Vec::new(),
            created_at: chrono::Utc::now(),
            expires_at: None,
            author_user_id: None,
        }
    }

    #[test]
    fn human_rows_render_display_name_agent_rows_keep_agent_id() {
        let labels = HashMap::from([("u-alice".to_string(), "Alice".to_string())]);
        assert_eq!(speaker_label(&human_msg("u-alice"), &labels), "Alice");
        assert_eq!(speaker_label(&human_msg("u-bob"), &labels), "u-bob"); // 无名字回退 id
        assert_eq!(speaker_label(&agent_msg("coder"), &labels), "coder");
    }

    #[test]
    fn resolve_display_name_falls_back_to_raw_id_when_store_has_no_name() {
        let store = SecurityStore::in_memory().expect("security store");
        assert_eq!(resolve_display_name(&store, "u-ghost"), "u-ghost");
    }

    #[test]
    fn resolve_display_name_returns_the_stored_display_name() {
        let store = SecurityStore::in_memory().expect("security store");
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .expect("create user");
        assert_eq!(resolve_display_name(&store, "u-alice"), "Alice");
    }

    #[test]
    fn resolve_labels_degrades_to_empty_map_when_store_is_absent() {
        let history = vec![human_msg("u-alice")];
        let labels = resolve_labels(None, &history);
        assert!(labels.is_empty(), "no store ⇒ no labels resolved");
        // speaker_label falls back through the empty map to the raw id — never blocks.
        assert_eq!(speaker_label(&history[0], &labels), "u-alice");
    }

    #[test]
    fn resolve_labels_batch_resolves_every_distinct_human_author() {
        let store = Arc::new(SecurityStore::in_memory().expect("security store"));
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .expect("create user");
        // u-bob deliberately has no user row: must fall back to the raw id, not panic
        // or drop the entry.
        let history = vec![human_msg("u-alice"), human_msg("u-bob"), agent_msg("coder")];
        let labels = resolve_labels(Some(&store), &history);
        assert_eq!(labels.get("u-alice").map(String::as_str), Some("Alice"));
        assert_eq!(labels.get("u-bob").map(String::as_str), Some("u-bob"));
        assert_eq!(
            labels.len(),
            2,
            "agent/system row must not contribute a label entry"
        );
    }

    /// Step 3's deliberate shape change (spec A5): a single human's row now renders
    /// under their resolved display name instead of the reserved `user` handle.
    /// "Single-human zero change" (Global Constraint 1) is scoped to activation
    /// semantics, not transcript bytes — this IS the intended new shape, asserted
    /// explicitly here rather than preserving the old `[user]:` rendering.
    #[test]
    fn single_human_transcript_row_now_renders_under_display_name() {
        let store = Arc::new(SecurityStore::in_memory().expect("security store"));
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .expect("create user");
        let raw = vec![human_msg("u-alice")];
        let labels = resolve_labels(Some(&store), &raw);
        let history: Vec<(String, String)> = raw
            .into_iter()
            .map(|m| (speaker_label(&m, &labels), m.content))
            .collect();
        let transcript = crate::teams::broadcast::transcript::format_transcript(&history, 10_000);
        assert_eq!(transcript, "[Alice]: hi");
    }
}
