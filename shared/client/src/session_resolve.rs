//! "Which conversation did I leave off in?" — one answer, shared by every thin
//! client.
//!
//! `aleph ask --last` and `aleph chat --continue` ask the same question of the
//! same RPC, and the tie-breaking rule (newest `last_active_at`, ties keep the
//! earlier candidate) has to be the same rule or the two commands will resume
//! different threads from the same terminal. It lives here rather than in
//! `aleph-cli` because `aleph-tui` cannot depend on that crate — both depend on
//! this one.

use aleph_protocol::SessionListRow;
use serde::Deserialize;

use crate::connection::AlephClient;
use crate::error::{CliError, CliResult};

/// The reply, as one type shared with the server.
///
/// This module used to declare its own two-field `SessionRow`. It parsed
/// correctly, which is the trap: a private struct naming a subset of a wire
/// contract can only ever prove it is a superset reader of what arrives, so a
/// server-side rename degrades to `#[serde(default)]` — "no sessions
/// available", a message that reads like an empty install (criterion #10).
/// [`SessionListRow`] is the type the server constructs, so the same rename is
/// a compile error here.
///
/// Only `key` and `last_active_at` are read; the rest ride along, which costs
/// nothing and means the next caller that needs `topic` does not write a third
/// partial copy.
#[derive(Debug, Deserialize)]
struct SessionListReply {
    #[serde(default)]
    sessions: Vec<SessionListRow>,
}

/// The key of the most recently active conversation.
///
/// RFC 3339 timestamps sort lexicographically, so a string comparison is the
/// chronological one as long as every row uses the same offset — which
/// `SessionListRow` guarantees by rendering them all through `to_rfc3339`.
///
/// # Errors
///
/// - the RPC itself failed, or
/// - the account has no sessions yet (a fresh install), which callers should
///   treat as "start a new one", not as a fatal error.
pub async fn resolve_last_session(client: &AlephClient) -> CliResult<String> {
    let listed: SessionListReply = client.call("sessions.list", None::<()>).await?;

    let mut best: Option<&SessionListRow> = None;
    for row in &listed.sessions {
        if row.key.is_empty() {
            continue;
        }
        // Strictly greater wins; ties keep the earlier candidate so the choice
        // is deterministic for sessions that share a timestamp.
        if best.is_none_or(|b| row.last_active_at > b.last_active_at) {
            best = Some(row);
        }
    }

    best.map(|r| r.key.clone())
        .ok_or_else(|| CliError::Other("no sessions available to resume".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(rows: &[(&str, &str)]) -> Option<String> {
        let reply = SessionListReply {
            sessions: rows
                .iter()
                .map(|(k, ts)| SessionListRow {
                    key: (*k).to_string(),
                    last_active_at: (*ts).to_string(),
                    ..SessionListRow::default()
                })
                .collect(),
        };
        let mut best: Option<&SessionListRow> = None;
        for row in &reply.sessions {
            if row.key.is_empty() {
                continue;
            }
            if best.is_none_or(|b| row.last_active_at > b.last_active_at) {
                best = Some(row);
            }
        }
        best.map(|r| r.key.clone())
    }

    #[test]
    fn newest_timestamp_wins() {
        assert_eq!(
            pick(&[
                ("agent:main:main", "2026-08-01T00:00:00+00:00"),
                ("agent:main:main:s3", "2026-08-11T09:00:00+00:00"),
                ("agent:main:main:s2", "2026-08-10T23:59:59+00:00"),
            ]),
            Some("agent:main:main:s3".to_string())
        );
    }

    #[test]
    fn ties_keep_the_earlier_candidate() {
        assert_eq!(
            pick(&[
                ("a", "2026-08-11T09:00:00+00:00"),
                ("b", "2026-08-11T09:00:00+00:00")
            ]),
            Some("a".to_string()),
            "a deterministic tie-break, so two commands cannot resume different threads"
        );
    }

    #[test]
    fn keyless_rows_are_skipped_not_selected() {
        assert_eq!(
            pick(&[
                ("", "2999-01-01T00:00:00+00:00"),
                ("real", "2026-01-01T00:00:00+00:00")
            ]),
            Some("real".to_string())
        );
    }

    #[test]
    fn an_empty_list_yields_nothing() {
        assert_eq!(pick(&[]), None);
    }

    /// The parse must survive the real `sessions.list` row, which carries a
    /// great many fields this decision does not care about. Serde ignores
    /// unknown keys, so what this pins is the two names it *does* need.
    #[test]
    fn a_full_session_row_parses() {
        let reply: SessionListReply = serde_json::from_str(
            r#"{"sessions":[{"key":"agent:main:main","agent_id":"main","session_type":"main",
                 "message_count":4,"created_at":"2026-08-01T00:00:00+00:00",
                 "last_active_at":"2026-08-11T09:00:00+00:00","input_tokens":10,
                 "output_tokens":5,"compaction_count":0,"updated_at":1}]}"#,
        )
        .expect("parse");
        assert_eq!(reply.sessions.len(), 1);
        assert_eq!(reply.sessions[0].key, "agent:main:main");
        assert_eq!(
            reply.sessions[0].last_active_at,
            "2026-08-11T09:00:00+00:00"
        );
    }
}
