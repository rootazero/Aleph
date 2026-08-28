use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: String,
    pub role: String,
    pub content: String,
    /// When the message was recorded — **in an ambiguous unit**. Read it with
    /// [`MessageRecord::instant`], never directly.
    ///
    /// The store's trait documents unix seconds. What actually reaches disk
    /// depends on the backend, and the difference is not that one of them
    /// "writes milliseconds" — it is WHO decides:
    ///
    /// - The **file** backend preserves whatever the producer put here
    ///   (`append_transcript` serializes the record verbatim). There are two
    ///   producers: [`MessageProjector`] stamps `created_at_ms`
    ///   (milliseconds) and direct writers such as `agent_instance` stamp
    ///   `timestamp()` (seconds). That is why both spellings can appear inside
    ///   a SINGLE transcript — measured on a real install, 2026-08-28: 1745 of
    ///   3030 rows in milliseconds, and two sessions carrying both.
    /// - The **SQLite** backend never stores a caller's stamp at all
    ///   (`add_message_full` overwrites it with its own
    ///   `now().timestamp()`), so its column is uniformly seconds. A separate
    ///   consequence, out of scope here: those rows record INSERT time rather
    ///   than event time.
    ///
    /// The unit cannot simply be corrected at the source: the value doubles as
    /// the `before` pagination cursor, and every session already on disk would
    /// have to be migrated with it. So the resolution lives here, at the type,
    /// where both readers and the cursor see the same interpretation — via
    /// [`stamp_millis`], which is the crate's single application of the
    /// boundary. That last clause used to be a promise the cursor did not
    /// keep: it compared the raw column against a seconds number, so no
    /// millisecond row was ever "strictly older" than any cursor and
    /// `chat.history?before=…` answered `{count: 0}` — which a client can only
    /// read as "you have reached the beginning".
    ///
    /// [`MessageProjector`]: crate::gateway::session_projector
    pub timestamp: i64,
    pub metadata: Option<Value>,
    /// Tokens the LLM call that produced this message was billed for. Zero on
    /// user / tool / system rows, which no call produced.
    ///
    /// (There were `model` / `model_provider` fields here too. The `messages`
    /// table has never had those columns — no INSERT wrote them and every SELECT
    /// filled them with `None` — so they were struct-shaped decoration. The
    /// serving model is recorded where it has a column: `sessions.model`.)
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// Above this a raw [`MessageRecord::timestamp`] is read as milliseconds,
/// below it as seconds.
///
/// `1e11` seconds is the year 5138 and `1e11` milliseconds is 1973-03-03, so
/// no conversation Aleph could have recorded falls in the ambiguous gap.
/// Reading a millisecond value as seconds is what dated exported messages to
/// the year 58536 and put "2026-03-02" beside a conversation from July.
///
/// `pub(crate)` so the startup repair pass
/// ([`crate::gateway::session_store::migration::repair_session_metadata`])
/// normalizes legacy ms-stamped `SessionMetadata::last_active_at` against the
/// same boundary the readers use — a second literal here would be a second
/// definition of the same rule.
pub(crate) const SECONDS_MILLIS_BOUNDARY: i64 = 100_000_000_000;

/// One raw [`MessageRecord::timestamp`]-shaped value, in milliseconds.
///
/// The single place [`SECONDS_MILLIS_BOUNDARY`] is applied. Total — every
/// `i64` has an answer — because its second caller is a *comparison*, and a
/// comparison that can decline to answer either drops the row it could not
/// read or keeps it on every page.
///
/// A free function rather than only a method on the record, because the
/// `before` pagination cursor has to resolve the SAME way as the rows it is
/// ranked against and it arrives as a bare number with no record around it.
/// That was the gap: [`MessageRecord::timestamp`]'s own doc uses the cursor's
/// existence to argue the stored unit must not be migrated, and promises in
/// return that "the resolution lives here, at the type, where both readers and
/// the cursor see the same interpretation" — while the cursor compared raw
/// values. On a real install (2026-08-28, 327 sessions / 3030 rows) 58% of
/// rows are millisecond-stamped and two sessions carry both spellings, so a
/// seconds cursor silently excluded the majority of the transcript and
/// `chat.history?before=…` answered `{count: 0}` — "you have reached the
/// beginning" — for the largest sessions on disk.
///
/// Milliseconds, not seconds, so nothing is truncated on the way through: a
/// millisecond row keeps its precision and a seconds row is exact either way.
/// The `* 1000` cannot overflow — this branch is only reached for
/// `|raw| < 1e11`, i.e. at most `1e14`.
#[must_use]
pub(crate) fn stamp_millis(raw: i64) -> i64 {
    // `checked_abs`, not `abs`: `i64::MIN` has no positive counterpart, so
    // `abs()` panics on it in a debug build and wraps back to `i64::MIN` in a
    // release one — a stored stamp that crashed one profile and silently took
    // the *seconds* branch in the other. `None` means "larger in magnitude
    // than anything", which is the milliseconds branch, and
    // `from_timestamp_millis` then correctly reports it as unrepresentable.
    // (This predates the cursor work; it was reachable from any reader through
    // `instant()`, and only surfaced because a test finally passed `i64::MIN`.)
    if raw.checked_abs().is_none_or(|a| a >= SECONDS_MILLIS_BOUNDARY) {
        raw
    } else {
        raw * 1000
    }
}

impl MessageRecord {
    /// The instant this message was recorded, resolving the store's mixed
    /// units. `None` for a value no calendar can represent.
    ///
    /// Every reader goes through this. Formatting the raw field directly is the
    /// bug that was independently repeated at five call sites.
    ///
    /// Expressed via [`stamp_millis`] so the boundary has exactly one
    /// application in the crate; the two used to be spelled separately, and the
    /// cursor's copy was the one that was never written.
    #[must_use]
    pub fn instant(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::from_timestamp_millis(stamp_millis(self.timestamp))
    }

    /// The recorded instant as RFC 3339, or an empty string when the stored
    /// value is unrepresentable.
    ///
    /// Empty rather than a placeholder date: a caption that is missing reads as
    /// missing, while a fabricated one reads as fact.
    #[must_use]
    pub fn rfc3339(&self) -> String {
        self.instant().map(|dt| dt.to_rfc3339()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub key: String,
    pub agent_id: String,
    pub session_type: String,
    pub created_at: i64,
    pub last_active_at: i64,
    pub message_count: i64,
    pub total_tokens: i64,
    pub auto_reset_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::gateway::session_manager::SessionState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_meta: Option<crate::gateway::session_manager::SessionIdentityMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_key: Option<String>,
    /// Authenticated user who created this session (`users.user_id`). `None` on
    /// rows created before P1 or outside any dispatch scope — read as owned by
    /// `OWNER_USER_ID` (adoption-by-absence; single predicate:
    /// `gateway::visibility::effective_owner`). Stamped once at creation, immutable
    /// thereafter (spec §10: 会话 scope 不可变).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
    /// Rendered `scope::ScopeId` ("personal:u-…"); `None` = legacy = org-era owner session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub compaction_count: i64,
    /// Derived title from first user message (computed lazily on append).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derived_title: Option<String>,
    /// Preview of the last message content (first N chars, updated on append).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_preview: Option<String>,
    /// Cumulative runtime in milliseconds (updated on session close).
    #[serde(default)]
    pub runtime_ms: i64,
    /// Estimated cost in USD (updated on session close / usage update).
    #[serde(default)]
    pub estimated_cost_usd: f64,
    /// List of compaction checkpoints (file backend only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<CheckpointSummary>,
}

impl SessionMetadata {
    /// Parse topic and status from a raw metadata JSON string for backward compatibility.
    pub fn parse_legacy_metadata_json(
        json: Option<&str>,
    ) -> (
        Option<String>,
        Option<String>,
        Option<crate::gateway::session_manager::SessionIdentityMeta>,
    ) {
        let Some(s) = json else {
            return (None, None, None);
        };
        if let Ok(identity) =
            serde_json::from_str::<crate::gateway::session_manager::SessionIdentityMeta>(s)
        {
            let topic = identity
                .custom
                .get("topic")
                .and_then(|v| v.as_str())
                .map(String::from);
            let status = identity
                .custom
                .get("status")
                .and_then(|v| v.as_str())
                .map(String::from);
            (topic, status, Some(identity))
        } else if let Ok(val) = serde_json::from_str::<Value>(s) {
            let topic = val.get("topic").and_then(|v| v.as_str()).map(String::from);
            let status = val.get("status").and_then(|v| v.as_str()).map(String::from);
            (topic, status, None)
        } else {
            (None, None, None)
        }
    }

    /// Origin channel of this session derived from identity metadata.
    ///
    /// Returns `None` for the synthetic `""`/`"unknown"` sentinel so callers
    /// (`sessions.list`, `sessions.changed`) omit a meaningless origin badge.
    /// Single source of truth for the "what counts as a real origin" rule,
    /// shared by the `SessionInfo` builder and the session-changed event.
    #[must_use]
    pub fn origin_channel(&self) -> Option<String> {
        let im = self.identity_meta.as_ref()?;
        let c = im.source_channel.trim();
        (!c.is_empty() && c != "unknown").then(|| c.to_string())
    }

    /// Origin conversation id captured alongside the origin channel on the
    /// first inbound message (e.g. the Telegram chat id). Drives cross-surface
    /// reply fan-out (sub-gap (b)): a run continued from the Panel can deliver
    /// its final reply back to `(origin_channel, origin_conversation)`.
    pub fn origin_conversation(&self) -> Option<String> {
        self.identity_meta
            .as_ref()?
            .custom
            .get(ORIGIN_CONVERSATION_KEY)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    /// Stamp `owner_user_id`/`scope_id` from the active
    /// [`crate::scope::current_scope`] attribution. No-op when already
    /// stamped — stamping is create-only (spec §10: session scope is
    /// immutable once set). Call sites: both `SessionStore::get_or_create`
    /// CREATE branches only, never on the existing-session read path.
    pub fn stamp_attribution(&mut self) {
        if self.owner_user_id.is_some() {
            return;
        }
        if let Some(attr) = crate::scope::current_scope() {
            self.owner_user_id = Some(attr.owner_user_id);
            self.scope_id = Some(attr.scope.render());
        }
    }
}

/// Identity-metadata custom key under which a session's origin conversation id
/// is persisted. Written by `SessionManager::set_source_channel`, read by
/// `SessionMetadata::origin_conversation`.
pub const ORIGIN_CONVERSATION_KEY: &str = "origin_conversation";

#[cfg(test)]
mod tests {
    use super::*;

    fn record_at(timestamp: i64) -> MessageRecord {
        MessageRecord {
            id: "m1".to_string(),
            role: "user".to_string(),
            content: String::new(),
            timestamp,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// Regression: a millisecond timestamp read as seconds dated exported
    /// messages to the year 58536 and printed "03-02" beside a July
    /// conversation in the Panel's session list. Both spellings on disk must
    /// resolve to the same real instant.
    #[test]
    fn a_record_reads_the_same_instant_from_either_unit() {
        // 2026-07-26T10:37:12Z, spelled both ways.
        let secs = record_at(1_785_062_232);
        let millis = record_at(1_785_062_232_000);
        assert_eq!(secs.instant(), millis.instant());
        assert!(
            millis.rfc3339().starts_with("2026-07-26T"),
            "got {}",
            millis.rfc3339()
        );
    }

    /// The property the `before` cursor is built on, stated on its own: two
    /// rows of the SAME conversation written in different units must ORDER
    /// correctly against each other and against a cursor.
    ///
    /// Comparing the raw fields does not — a millisecond stamp is ~1000x a
    /// seconds one, so every millisecond row ranks as newer than every seconds
    /// row regardless of when it happened, and newer than any seconds cursor.
    /// This is not hypothetical: a real install had 58% of its rows in
    /// milliseconds, and two sessions carried both spellings inside a single
    /// transcript.
    #[test]
    fn mixed_units_order_against_each_other_and_a_cursor() {
        // 2026-07-26T10:37:12Z (seconds) is EARLIER than
        // 2026-07-26T10:37:13Z (milliseconds).
        let older_secs = 1_785_062_232_i64;
        let newer_millis = 1_785_062_233_000_i64;

        // Raw, they compare backwards — the bug.
        assert!(
            older_secs < newer_millis,
            "raw comparison happens to agree here only because the older row \
             is the seconds one; reverse the roles and it flips"
        );
        let newer_secs = 1_785_062_233_i64;
        let older_millis = 1_785_062_232_000_i64;
        assert!(
            older_millis > newer_secs,
            "raw: the OLDER message ranks as newer, which is what made a \
             seconds cursor exclude every millisecond row"
        );

        // Normalized, both pairs order by real time.
        assert!(stamp_millis(older_secs) < stamp_millis(newer_millis));
        assert!(stamp_millis(older_millis) < stamp_millis(newer_secs));

        // And the two spellings of one instant collapse to one number, which
        // is what lets `instant()` be expressed through this.
        assert_eq!(stamp_millis(1_785_062_232), stamp_millis(1_785_062_232_000));
    }

    #[test]
    fn an_unrepresentable_timestamp_yields_no_caption() {
        // Never panic and never print a fabricated date: a missing caption
        // reads as missing, an invented one reads as fact.
        assert_eq!(record_at(i64::MAX).instant(), None);
        assert_eq!(record_at(i64::MAX).rfc3339(), "");
        // `i64::MIN` is the one value with no positive counterpart. `abs()`
        // panicked on it in debug and wrapped in release, so this reader used
        // to behave differently per profile on the same stored byte.
        assert_eq!(record_at(i64::MIN).instant(), None);
        assert_eq!(record_at(i64::MIN).rfc3339(), "");
        assert_eq!(record_at(0).rfc3339(), "1970-01-01T00:00:00+00:00");
    }

    #[test]
    fn message_record_tool_fields_default_none_and_roundtrip() {
        // Old JSON (no tool fields) deserializes → None
        let legacy =
            r#"{"id":"1","role":"assistant","content":"hi","timestamp":1,"metadata":null}"#;
        let rec: MessageRecord = serde_json::from_str(legacy).unwrap();
        assert!(rec.tool_call_id.is_none());
        assert!(rec.tool_name.is_none());
        // With tool fields round-trip
        let tool = MessageRecord {
            id: "2".into(),
            role: "tool".into(),
            content: "{}".into(),
            timestamp: 2,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: Some("call_1".into()),
            tool_name: Some("bash_exec".into()),
        };
        let back: MessageRecord =
            serde_json::from_str(&serde_json::to_string(&tool).unwrap()).unwrap();
        assert_eq!(back.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(back.tool_name.as_deref(), Some("bash_exec"));
    }
}

#[cfg(test)]
mod origin_channel_tests {
    use super::*;
    use crate::gateway::session_manager::SessionIdentityMeta;

    fn meta_with_channel(channel: &str) -> SessionMetadata {
        SessionMetadata {
            identity_meta: Some(SessionIdentityMeta::owner(channel)),
            ..Default::default()
        }
    }

    #[test]
    fn origin_channel_none_when_no_identity() {
        assert_eq!(SessionMetadata::default().origin_channel(), None);
    }

    #[test]
    fn origin_channel_none_for_unknown_sentinel() {
        assert_eq!(meta_with_channel("unknown").origin_channel(), None);
        assert_eq!(meta_with_channel("  ").origin_channel(), None);
    }

    #[test]
    fn origin_channel_some_for_real_channel() {
        assert_eq!(
            meta_with_channel("telegram").origin_channel(),
            Some("telegram".to_string())
        );
        assert_eq!(
            meta_with_channel("gui:chat").origin_channel(),
            Some("gui:chat".to_string())
        );
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPreview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<SessionMetadata>,
    pub messages: Vec<MessageRecord>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    pub agent_id: Option<String>,
    pub limit: Option<usize>,
    pub active_minutes: Option<u32>,
    /// When `Some`, only sessions whose effective owner
    /// (`gateway::visibility::effective_owner`) equals this user id are
    /// returned. `None` = unfiltered (every owner).
    pub owner_visible_to: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeleteResult {
    pub deleted: bool,
}

// `CompactStrategy` / `CompactResult` / `SESSION_COMPACT_KEEP_LAST_N` are gone
// with the `SessionStore::compact` they described. Their story was
// "keep the 50 most recent messages, the memory layer distilled the rest" —
// but they operated on the `messages` READ PROJECTION, not on `session_events`,
// which is what the prompt is rebuilt from. So the trim deleted the user's
// scrollback and freed zero context. User-driven `/compact` now lives in
// `context::compact::manual`, keyed off a token budget instead of a message
// count, and soft-retires the event log rather than deleting rows.

/// Outcome of `SessionStore::truncate_messages`.
#[derive(Debug, Clone, Default)]
pub struct TruncateResult {
    /// Number of messages that were removed from the session.
    pub messages_removed: usize,
    /// Rough estimate of the prompt+completion tokens that were dropped.
    pub tokens_removed_estimate: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSummary {
    pub checkpoint_id: String,
    pub created_at: i64,
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub retained_message_count: i64,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub session_key: String,
    pub agent_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub topic: Option<String>,
}

/// Event payload broadcast when a session changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionChangedEvent {
    pub session_key: String,
    pub reason: String,
    pub ts: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub compacted: bool,
}
