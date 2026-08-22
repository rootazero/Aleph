//! `security.audit.query` — the read face of the security audit trail.
//!
//! # What was missing
//!
//! Five producers wrote to `security_audit_log`; nothing read it. The
//! `AuthorityChange` variant exists so that "what authority changed, in order"
//! is answerable with one `WHERE` clause, and until this handler there was
//! nowhere to run one. The same held for `ScopedContentRead`, which was added
//! precisely so an operator reading somebody else's transcript leaves a trace —
//! a trace that no operator, including the one being held accountable by it,
//! could look at.
//!
//! # Why admin-gated, and gated by prefix
//!
//! The trail names principals, sessions and source addresses across the whole
//! server; it is org-level accountability, not caller's-own-data, so it sits
//! behind the `security.` prefix in [`crate::gateway::method_admin`]. Prefix
//! rather than method so a future `security.audit.*` sibling is gated the day
//! it is registered instead of the day somebody notices.
//!
//! A second `UserRole::Admin` principal sees the same rows as the owner. That
//! is deliberate: an audit trail that hides an operator's own entries from
//! their peers is not a trail, and role — not ownership — is the axis this
//! surface narrows on (see `caller_identity::caller_is_member`'s doc for why
//! those two questions must not be answered with one predicate).

use crate::sync_primitives::Arc;

use aleph_protocol::audit::{AuditQueryParams, AuditQueryResult};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SecurityStore;
use crate::security::audit::DEFAULT_RETENTION_SECS;

/// `security.audit.query { event_type?, actor_user?, since_secs?, limit? }`
/// → [`AuditQueryResult`].
///
/// The response is **built from** the contract type rather than assembled as a
/// `json!` literal beside it. That is what makes over-sending a compile-time
/// impossibility instead of an assertion somebody has to remember to write —
/// the `workspace.get` leak (four fields on the wire with no reader and no
/// writer anywhere) got there through a literal that parsed fine.
pub async fn handle_query(request: JsonRpcRequest, store: Arc<SecurityStore>) -> JsonRpcResponse {
    // Absent params is the default query, not a malformed one: `security.audit.query`
    // with no arguments is the most useful call this surface has.
    let params: AuditQueryParams = match request.params.clone() {
        None | Some(serde_json::Value::Null) => AuditQueryParams::default(),
        Some(v) => match serde_json::from_value(v) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("invalid params: {e}"),
                )
            }
        },
    };

    match store.query_audit_entries(&params) {
        Ok((entries, truncated)) => {
            let result = AuditQueryResult {
                entries,
                // The horizon the drain task deletes behind. Sent on every
                // response because an empty page is otherwise three answers
                // wearing one face — see the field's doc.
                retention_secs: DEFAULT_RETENTION_SECS,
                truncated,
            };
            match serde_json::to_value(&result) {
                Ok(v) => JsonRpcResponse::success(request.id, v),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("failed to encode audit result: {e}"),
                ),
            }
        }
        // A store failure is "I could not read the trail", which must never
        // render as "the trail is empty" — the one reading that would let a
        // broken query pass for a clean window.
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to read audit trail: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::audit::{AuditEntry, AuditEventType, AuditSeverity};
    use serde_json::json;

    fn store() -> Arc<SecurityStore> {
        Arc::new(SecurityStore::in_memory().unwrap())
    }

    fn write(store: &SecurityStore, event_type: AuditEventType, actor: Option<&str>, detail: &str) {
        store
            .insert_audit_entry(&AuditEntry {
                event_type,
                severity: AuditSeverity::Warn,
                source_ip: None,
                session_id: None,
                actor_user: actor.map(str::to_string),
                detail: detail.to_string(),
            })
            .unwrap();
    }

    async fn query(store: &Arc<SecurityStore>, params: serde_json::Value) -> AuditQueryResult {
        let req = JsonRpcRequest::with_id("security.audit.query", Some(params), json!(1));
        let resp = handle_query(req, store.clone()).await;
        assert!(resp.is_success(), "{resp:?}");
        serde_json::from_value(resp.result.unwrap()).unwrap()
    }

    /// The question the `AuthorityChange` variant was written to answer, asked
    /// through the surface that had never existed.
    #[tokio::test]
    async fn one_filter_answers_what_authority_changed() {
        let s = store();
        write(&s, AuditEventType::AuthFailure, None, "bad token");
        write(
            &s,
            AuditEventType::AuthorityChange,
            Some("u-owner"),
            "users.update: role u-alice member→admin",
        );

        let out = query(&s, json!({"event_type": "authority_change"})).await;
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.entries[0].actor_user.as_deref(), Some("u-owner"));
        assert!(out.entries[0].detail.contains("member→admin"));
    }

    #[tokio::test]
    async fn an_actor_filter_narrows_to_one_principal() {
        let s = store();
        write(&s, AuditEventType::AuthorityChange, Some("u-owner"), "a");
        write(&s, AuditEventType::AuthorityChange, Some("u-alice"), "b");

        let out = query(&s, json!({"actor_user": "u-alice"})).await;
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.entries[0].detail, "b");
    }

    /// A page that stopped at the limit must not read like a clean window.
    #[tokio::test]
    async fn a_full_page_says_there_is_more_behind_it() {
        let s = store();
        for i in 0..5 {
            write(&s, AuditEventType::AuthorityChange, None, &format!("e{i}"));
        }

        let capped = query(&s, json!({"limit": 2})).await;
        assert_eq!(capped.entries.len(), 2);
        assert!(capped.truncated, "2 of 5 must not report a complete window");

        let whole = query(&s, json!({"limit": 5})).await;
        assert_eq!(whole.entries.len(), 5);
        assert!(
            !whole.truncated,
            "an exactly-full page with nothing behind it is not truncated"
        );
    }

    /// An empty result is only readable next to the horizon it was deleted
    /// against; without this an operator cannot tell "quiet" from "purged".
    #[tokio::test]
    async fn every_response_carries_the_retention_horizon() {
        let s = store();
        let out = query(&s, json!({})).await;
        assert!(out.entries.is_empty());
        assert_eq!(out.retention_secs, DEFAULT_RETENTION_SECS);
    }

    /// Ordering is the point of a trail; ties inside one clock second are
    /// broken by insert order rather than left to the planner.
    #[tokio::test]
    async fn entries_come_back_newest_first_even_within_one_second() {
        let s = store();
        for i in 0..4 {
            write(&s, AuditEventType::AuthorityChange, None, &format!("e{i}"));
        }
        let out = query(&s, json!({})).await;
        let details: Vec<_> = out.entries.iter().map(|e| e.detail.as_str()).collect();
        assert_eq!(details, vec!["e3", "e2", "e1", "e0"]);
    }

    /// A vocabulary this build has not heard of must be askable, not refused —
    /// otherwise an older client cannot query a newer producer's rows.
    #[tokio::test]
    async fn an_unknown_event_type_matches_nothing_rather_than_erroring() {
        let s = store();
        write(&s, AuditEventType::AuthorityChange, None, "a");
        let out = query(&s, json!({"event_type": "invented_in_a_later_build"})).await;
        assert!(out.entries.is_empty());
    }

    #[tokio::test]
    async fn a_missing_params_object_is_the_default_query_not_an_error() {
        let s = store();
        write(&s, AuditEventType::AuthorityChange, None, "a");
        let req = JsonRpcRequest::with_id("security.audit.query", None, json!(1));
        let resp = handle_query(req, s).await;
        assert!(resp.is_success(), "{resp:?}");
        let out: AuditQueryResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(out.entries.len(), 1);
    }

    /// A negative window is a typo. Answering it with `now + n` would return
    /// the future's emptiness and read as "nothing happened".
    #[tokio::test]
    async fn a_negative_window_does_not_silently_answer_about_the_future() {
        let s = store();
        write(&s, AuditEventType::AuthorityChange, None, "a");
        let out = query(&s, json!({"since_secs": -86_400})).await;
        assert_eq!(
            out.entries.len(),
            1,
            "clamped to now, which still contains the entry just written"
        );
    }

    #[tokio::test]
    async fn a_misspelled_filter_is_refused_rather_than_widening_the_query() {
        let s = store();
        let req = JsonRpcRequest::with_id(
            "security.audit.query",
            Some(json!({"actor": "u-alice"})),
            json!(1),
        );
        let resp = handle_query(req, s).await;
        assert!(
            resp.error.is_some(),
            "an unknown key must not fall through to an unfiltered answer"
        );
    }
}
