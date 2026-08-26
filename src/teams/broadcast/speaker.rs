//! The single derivation of "what label does this row speak under" (spec §6.2
//! humanization, P2) — row-type-agnostic at its core, so a caller with a different
//! row shape (e.g. a future room-session history projection, Task 11) can reuse it
//! directly rather than re-deriving the lookup.
//!
//! Every human-authored `TeamMessage` row is stored with `from_agent ==
//! RESERVED_USER_HANDLE` regardless of who actually spoke — the raw `from_agent`
//! cannot distinguish Alice from Bob, which is exactly why this module exists. The
//! real speaker lives in `author_user_id`; this module resolves it to a display name
//! and hands the caller the one string a transcript row (or any other per-speaker
//! rendering) should show.
//!
//! The core, row-type-agnostic shapes:
//! - [`speaker_label`] takes a row's raw `from_agent` and (optional)
//!   `author_user_id` directly — no `TeamMessage` in its signature.
//! - [`resolve_labels`] batch-resolves display names for an iterator of raw author
//!   ids — no `TeamMessage` in its signature either.
//! - [`resolve_display_name`] is the shared single-id lookup both of the above (and
//!   any other caller that only ever has one author in hand at a time, e.g.
//!   `teams.chat.send`'s realtime fan-out broadcast) ultimately go through.
//!
//! [`speaker_label_for_message`] and [`resolve_labels_for_messages`] are thin
//! `TeamMessage` convenience wrappers around the two core functions above, kept only
//! so `GroupChatBroadcaster::run_member`'s call site reads naturally against its
//! `Vec<TeamMessage>` history — they add no logic beyond projecting a `TeamMessage`
//! down to the `(from_agent, author_user_id)` / author-id shape the core consumes.
//!
//! Degradation (P7): a `None` store, a store error, or an author with no display name
//! set all fall back to the raw user id. A display-name lookup must never block or fail
//! a run — the group chat keeps working, just under raw ids instead of names.

use std::collections::HashMap;

use crate::gateway::security::SecurityStore;
use crate::sync_primitives::Arc;
use crate::teams::messages::TeamMessage;

/// The label a transcript row speaks under, given its raw `from_agent` and (for a
/// human-authored row) `author_user_id`. Row-type-agnostic — takes the two relevant
/// fields directly rather than a `TeamMessage`, so any row shape that carries an
/// equivalent "who really said this" fact can call it. See
/// [`speaker_label_for_message`] for the `TeamMessage` convenience wrapper.
///
/// `author_user_id: Some(_)` (human row): the resolved display name from `labels`,
/// or the raw user id when `labels` has no entry for it (store was absent/erred, or
/// the user has no display name). `author_user_id: None` (agent/system row):
/// `from_agent`, unchanged — this is the entire pre-humanization behavior,
/// preserved byte-for-byte.
#[must_use]
pub fn speaker_label(
    from_agent: &str,
    author_user_id: Option<&str>,
    labels: &HashMap<String, String>,
) -> String {
    match author_user_id {
        Some(user_id) => labels
            .get(user_id)
            .cloned()
            .unwrap_or_else(|| user_id.to_string()),
        None => from_agent.to_string(),
    }
}

/// Thin `TeamMessage` convenience wrapper around [`speaker_label`], for
/// `GroupChatBroadcaster::run_member`'s transcript-history mapping. Adds no logic —
/// just projects `msg.from_agent` / `msg.author_user_id` into the core function.
#[must_use]
pub fn speaker_label_for_message(msg: &TeamMessage, labels: &HashMap<String, String>) -> String {
    speaker_label(&msg.from_agent, msg.author_user_id.as_deref(), labels)
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

/// Batch-resolve display names for every distinct id in `author_ids`. Row-type-
/// agnostic — takes raw author ids, not `TeamMessage`s, so any row shape can supply
/// its own iterator of "who really said this" ids. See
/// [`resolve_labels_for_messages`] for the `TeamMessage` convenience wrapper.
///
/// `store: None` (unavailable to this caller) degrades to an empty map — every row's
/// label then falls back to its raw user id through [`speaker_label`], never
/// blocking the run. Repeated ids are only looked up once (the returned map has one
/// entry per distinct id).
#[must_use]
pub fn resolve_labels<'a>(
    store: Option<&Arc<SecurityStore>>,
    author_ids: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, String> {
    let Some(store) = store else {
        return HashMap::new();
    };
    let mut labels = HashMap::new();
    for user_id in author_ids {
        labels
            .entry(user_id.to_string())
            .or_insert_with(|| resolve_display_name(store, user_id));
    }
    labels
}

/// Thin `TeamMessage` convenience wrapper around [`resolve_labels`], for
/// `GroupChatBroadcaster::run_member`'s transcript history. Adds no logic beyond
/// projecting the history down to its distinct human `author_user_id`s (agent/system
/// rows, where `author_user_id` is `None`, are filtered out — they contribute no
/// entry, same as the core function's contract).
#[must_use]
pub fn resolve_labels_for_messages(
    store: Option<&Arc<SecurityStore>>,
    history: &[TeamMessage],
) -> HashMap<String, String> {
    resolve_labels(
        store,
        history.iter().filter_map(|m| m.author_user_id.as_deref()),
    )
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
        assert_eq!(speaker_label("user", Some("u-alice"), &labels), "Alice");
        assert_eq!(speaker_label("user", Some("u-bob"), &labels), "u-bob"); // 无名字回退 id
        assert_eq!(speaker_label("coder", None, &labels), "coder");
    }

    /// The `TeamMessage` wrapper adds no logic of its own — it must agree with the
    /// core function given the same fields.
    #[test]
    fn speaker_label_for_message_agrees_with_the_core_function() {
        let labels = HashMap::from([("u-alice".to_string(), "Alice".to_string())]);
        assert_eq!(
            speaker_label_for_message(&human_msg("u-alice"), &labels),
            "Alice"
        );
        assert_eq!(
            speaker_label_for_message(&human_msg("u-bob"), &labels),
            "u-bob"
        );
        assert_eq!(
            speaker_label_for_message(&agent_msg("coder"), &labels),
            "coder"
        );
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
        let labels = resolve_labels(None, ["u-alice"]);
        assert!(labels.is_empty(), "no store ⇒ no labels resolved");
        // speaker_label falls back through the empty map to the raw id — never blocks.
        assert_eq!(speaker_label("user", Some("u-alice"), &labels), "u-alice");
    }

    #[test]
    fn resolve_labels_batch_resolves_every_distinct_author_id() {
        let store = Arc::new(SecurityStore::in_memory().expect("security store"));
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .expect("create user");
        // u-bob deliberately has no user row: must fall back to the raw id, not panic
        // or drop the entry. u-alice repeated: must be looked up once, not error.
        let labels = resolve_labels(Some(&store), ["u-alice", "u-bob", "u-alice"]);
        assert_eq!(labels.get("u-alice").map(String::as_str), Some("Alice"));
        assert_eq!(labels.get("u-bob").map(String::as_str), Some("u-bob"));
        assert_eq!(
            labels.len(),
            2,
            "repeated id must not create a duplicate entry"
        );
    }

    /// The `TeamMessage` wrapper must project the history's distinct human authors
    /// (filtering out agent/system rows) exactly as the core function would given
    /// the same ids directly.
    #[test]
    fn resolve_labels_for_messages_agrees_with_the_core_function() {
        let store = Arc::new(SecurityStore::in_memory().expect("security store"));
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .expect("create user");
        let history = vec![human_msg("u-alice"), human_msg("u-bob"), agent_msg("coder")];
        let via_wrapper = resolve_labels_for_messages(Some(&store), &history);
        let via_core = resolve_labels(Some(&store), ["u-alice", "u-bob"]);
        assert_eq!(via_wrapper, via_core);
        assert_eq!(
            via_wrapper.len(),
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
        let labels = resolve_labels_for_messages(Some(&store), &raw);
        let history: Vec<(String, String)> = raw
            .into_iter()
            .map(|m| (speaker_label_for_message(&m, &labels), m.content))
            .collect();
        let transcript = crate::teams::broadcast::transcript::format_transcript(&history, 10_000);
        assert_eq!(transcript, "[Alice]: hi");
    }
}
