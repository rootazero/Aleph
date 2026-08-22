//! Security audit trail read contract — `security.audit.query`.
//!
//! # Why this module exists
//!
//! The `security_audit_log` table had five producers and **no reader**.
//! `AuthFailure`, `RateLimited`, `CommandPolicy`, `ScopedContentRead` and
//! `AuthorityChange` were all written to SQLite, indexed by timestamp and event
//! type, and purged on a 30-day horizon — and nothing in the repository ever
//! ran a `SELECT` against them outside the drain task's own tests. The
//! `AuthorityChange` variant's doc states its purpose as answering "what
//! authority changed, in order" *with one `WHERE` clause*; there was no surface
//! from which that clause could be run. A write-only accountability trail is
//! accountability nobody can collect: it is the same shape as a capability with
//! a complete server half and no client, except that here the missing half is
//! the one the feature was built for.
//!
//! Reading it out-of-band is not the fallback it looks like. The agent reaching
//! for `sqlite3 ~/.aleph/data/security.db` is a sandbox-wall crossing this
//! codebase has already removed once (the governance sensor, 2026-07-20), and
//! an operator doing it by hand gets no retention horizon, no severity
//! vocabulary, and no guarantee the column names they typed are the ones the
//! writer used.
//!
//! # Why the shape lives in this crate
//!
//! Same reason as [`crate::workspace`]: the client is `aleph-cli`, which
//! deliberately cannot depend on `alephcore`. A wire contract whose two halves
//! are hand-copied into two crates is the defect that made every
//! `aleph workspace create` fail with `INVALID_PARAMS` for months while both
//! sides' tests stayed green. One type, so a rename is a compile error on both
//! sides.

use serde::{Deserialize, Serialize};

/// How many entries a query returns when the caller does not say.
///
/// Shared rather than defaulted independently on each side: a CLI that pages by
/// 50 while the server caps at 100 gives "that was everything" a second,
/// quieter answer.
pub const DEFAULT_AUDIT_LIMIT: usize = 100;

/// The ceiling the server clamps `limit` to.
///
/// The trail is unbounded within the retention horizon and this response is
/// materialised whole, so an un-clamped `limit` is a memory-shaped request the
/// caller writes. Clamping is silent by value but never silent by *effect* —
/// [`AuditQueryResult::truncated`] says so.
pub const MAX_AUDIT_LIMIT: usize = 1000;

/// Parameters for `security.audit.query`. Every field is a narrowing filter;
/// a request with no params is "the most recent page of everything".
///
/// `deny_unknown_fields`: each field here *narrows* the result, so a misspelled
/// key would silently widen the answer to a question the caller did not ask —
/// and a wider audit answer reads exactly like a correct one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditQueryParams {
    /// Restrict to one event type, e.g. `"authority_change"`.
    ///
    /// Matched against the writer's own `Display` spelling. An unrecognised
    /// value is **not** an error: the vocabulary can grow, and refusing a name
    /// this build has not heard of would make a newer entry unqueryable from an
    /// older client. It simply matches nothing, and `entries` being empty is
    /// already qualified by the fields below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,

    /// Restrict to one actor — the `users.user_id` behind the request.
    ///
    /// Note that `None` here means "do not filter", which is a different
    /// question from "entries whose actor is unknown". The trail records
    /// `actor_user = NULL` for producers that predate the user model and for
    /// non-gateway callers, and those rows are returned by an unfiltered query
    /// like any other — they say "we do not know who", not "nobody".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_user: Option<String>,

    /// Only entries from the last N seconds.
    ///
    /// Relative rather than absolute on purpose: the stored timestamp is
    /// server-clock unix seconds, and a client sending an absolute instant
    /// would be asserting agreement about a clock the two sides have never
    /// compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_secs: Option<i64>,

    /// Page size; clamped to [`MAX_AUDIT_LIMIT`], defaults to
    /// [`DEFAULT_AUDIT_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// One row of the trail, newest first.
///
/// The field set is the table's column set minus the autoincrement id: a
/// projection that dropped a column would be a question the surface silently
/// cannot answer, and `source_ip`/`session_id` are precisely the two that turn
/// "somebody read something" into "from where, about what".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntryRow {
    /// Server-clock unix **seconds** (the column's own unit — `strftime('%s')`).
    /// Not milliseconds; the rest of this repo's timestamps are milliseconds,
    /// which is exactly why it is stated here rather than inferred.
    pub timestamp: i64,
    /// The writer's `Display` spelling, e.g. `"authority_change"`.
    pub event_type: String,
    /// `"critical"` or `"warn"`.
    pub severity: String,
    /// WHO acted, when a principal was attached to the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_user: Option<String>,
    /// FROM WHERE, for the network-origin producers (auth failures, rate
    /// limiting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ip: Option<String>,
    /// ABOUT WHAT — for `scoped_content_read`, the session whose ownership was
    /// checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The producer's own sentence. Never carries credential material.
    pub detail: String,
}

/// Response for `security.audit.query`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditQueryResult {
    /// Matching entries, newest first.
    pub entries: Vec<AuditEntryRow>,

    /// The horizon the drain task purges behind, in seconds.
    ///
    /// Carried on **every** response because without it an empty result is
    /// three different answers wearing one face: nothing happened, nothing
    /// matched, or it happened and was deleted. Only the reader can tell those
    /// apart, and only if the surface hands them the horizon it was deleted
    /// against.
    pub retention_secs: i64,

    /// Whether the page stopped at `limit` with more rows behind it.
    ///
    /// "The window is clean" and "I stopped counting" must not render the same,
    /// which is the whole failure mode of a silent cap.
    pub truncated: bool,
}

impl AuditQueryParams {
    /// The effective page size for these params: the caller's, clamped, or the
    /// shared default.
    ///
    /// A method rather than a field so both sides reach the same number through
    /// the same code — the clamp is part of the contract, not of whichever
    /// handler happened to implement it.
    #[must_use]
    pub fn effective_limit(&self) -> usize {
        self.limit
            .unwrap_or(DEFAULT_AUDIT_LIMIT)
            .clamp(1, MAX_AUDIT_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_limit_is_the_shared_default() {
        assert_eq!(
            AuditQueryParams::default().effective_limit(),
            DEFAULT_AUDIT_LIMIT
        );
    }

    #[test]
    fn an_oversized_limit_is_clamped_not_refused() {
        // Refusing would make a well-meaning `--limit 100000` an error the
        // operator has to decode; clamping plus `truncated` tells them what
        // actually happened.
        let p = AuditQueryParams {
            limit: Some(usize::MAX),
            ..Default::default()
        };
        assert_eq!(p.effective_limit(), MAX_AUDIT_LIMIT);
    }

    #[test]
    fn a_zero_limit_cannot_produce_an_always_empty_page() {
        // `--limit 0` is a typo, not a request for a page that can never
        // contain evidence. One row beats a result that reads as "nothing
        // happened".
        let p = AuditQueryParams {
            limit: Some(0),
            ..Default::default()
        };
        assert_eq!(p.effective_limit(), 1);
    }

    #[test]
    fn a_misspelled_filter_key_is_refused_rather_than_widening_the_answer() {
        let err = serde_json::from_value::<AuditQueryParams>(serde_json::json!({
            "event_typ": "authority_change"
        }));
        assert!(
            err.is_err(),
            "an unknown key must not deserialize into a broader query than the caller asked for"
        );
    }

    #[test]
    fn absent_filters_are_omitted_from_the_wire_rather_than_sent_as_null() {
        let wire = serde_json::to_value(AuditQueryParams::default()).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({}),
            "a default query must serialise to an empty object; `deny_unknown_fields` \
             tolerates nulls but a null filter reads as an assertion about the value"
        );
    }
}
