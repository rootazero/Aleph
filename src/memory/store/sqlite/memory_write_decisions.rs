//! `memory_write_decisions` — one row per curated-memory write ATTEMPT.
//!
//! "Why didn't you remember that?" used to be unanswerable from data. A
//! refused `remember` call told the model `rejected: …` inside that one turn
//! and persisted nothing; an hour later neither the user nor the model could
//! say what had happened, and the only remaining source was the model's
//! recollection. Every branch of the write path — including the branches that
//! change nothing — now lands one row here with a machine-readable reason.
//!
//! Diagnostic log, not an archive: capped at [`MAX_DECISIONS_PER_AGENT`] rows
//! per agent and pruned on every write, following the keep-newest-N idiom of
//! `prune_routing_experiences`.

use rusqlite::params;

use crate::error::AlephError;

use super::SqliteMemoryBackend;

/// Rows retained per agent. Sized to cover "the last few sessions of memory
/// writes" — enough to answer a question about something the user remembers
/// asking for, not enough to become a second copy of memory.
pub const MAX_DECISIONS_PER_AGENT: usize = 200;

/// Characters of the attempted subject kept on a row.
///
/// Deliberately an EXCERPT, never the full content: a refused entry may be
/// exactly the payload the threat scanner refused, and this log is read back
/// into a model's context by `memory_trace`. The excerpt is long enough to
/// recognise which fact a row is about and short enough that it is a label
/// rather than a delivery vehicle.
const SUBJECT_EXCERPT_CHARS: usize = 80;

/// Why a curated-memory write ended the way it did.
///
/// A closed enum, not free text: the whole value of the log is that a later
/// reader can separate "the scanner refused it" from "it was already there"
/// without parsing prose. `Written` is a member so the log is a complete
/// account of the write path rather than a failure list — "it never reached
/// memory" and "it was written and then later removed" are different answers
/// to the user's question, and only a log that records successes can tell
/// them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryWriteReason {
    /// The write landed on disk.
    Written,
    /// An identical entry was already present.
    Duplicate,
    /// The entry would push MEMORY.md past its character budget.
    OverBudget,
    /// `add` is blocked while the file is still in uncurated legacy form.
    LegacyBlocked,
    /// The content scanner refused the payload.
    ScannerRejected,
    /// The content carried the `\n§\n` record separator.
    DelimiterInContent,
    /// No entry matched the supplied substring.
    NoMatch,
    /// The substring matched several distinct entries.
    Ambiguous,
    /// Empty content / empty substring / empty batch.
    Empty,
    /// One operation of an atomic batch failed, so none were applied.
    BatchAborted,
    /// The per-turn consecutive-rejection cap was spent (see `remember.rs`).
    RetryCapReached,
}

impl MemoryWriteReason {
    /// The persisted token. The stored vocabulary is closed by this function —
    /// nothing else writes the `reason` column — so read paths can hand the
    /// raw string onward without parsing it back.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Duplicate => "duplicate",
            Self::OverBudget => "over_budget",
            Self::LegacyBlocked => "legacy_blocked",
            Self::ScannerRejected => "scanner_rejected",
            Self::DelimiterInContent => "delimiter_in_content",
            Self::NoMatch => "no_match",
            Self::Ambiguous => "ambiguous",
            Self::Empty => "empty",
            Self::BatchAborted => "batch_aborted",
            Self::RetryCapReached => "retry_cap_reached",
        }
    }
}

/// One decision as read back out of the table.
///
/// `reason` is a plain `String` on the read side on purpose: it is emitted by
/// [`MemoryWriteReason::as_str`] and consumed as a display/JSON token, so
/// parsing it back into the enum would add a fallible step that no caller
/// needs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryWriteDecisionRow {
    /// `add` / `replace` / `remove` / `batch`.
    pub action: String,
    /// One of [`MemoryWriteReason::as_str`].
    pub reason: String,
    /// Bounded excerpt of the attempted subject — never the full content.
    pub subject: String,
    /// Unix seconds.
    pub created_at: i64,
}

/// Bound `subject` to [`SUBJECT_EXCERPT_CHARS`], char-safe (a byte slice would
/// panic mid-codepoint on non-ASCII memory entries).
fn excerpt(subject: &str) -> String {
    let mut out: String = subject.chars().take(SUBJECT_EXCERPT_CHARS).collect();
    if subject.chars().nth(SUBJECT_EXCERPT_CHARS).is_some() {
        out.push('…');
    }
    out
}

impl SqliteMemoryBackend {
    /// Append one write decision for `agent_id` and prune the agent's log back
    /// to [`MAX_DECISIONS_PER_AGENT`] rows.
    ///
    /// Callers treat failure as a warning, never as a write failure: an audit
    /// row that could not be stored must not take the memory write down with
    /// it (same contract as `AssemblyLogWriter::write`).
    pub fn record_write_decision(
        &self,
        agent_id: &str,
        action: &str,
        reason: MemoryWriteReason,
        subject: &str,
    ) -> Result<(), AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        conn.execute(
            "INSERT INTO memory_write_decisions \
             (id, agent_id, action, reason, subject, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                uuid::Uuid::new_v4().to_string(),
                agent_id,
                action,
                reason.as_str(),
                excerpt(subject),
                chrono::Utc::now().timestamp(),
            ],
        )
        .map_err(|e| AlephError::config(format!("record_write_decision insert: {e}")))?;

        // Prune on write — this is a diagnostic log, so it has a ceiling rather
        // than a retention policy. `rowid` breaks `created_at` ties: a retry
        // loop writes several rows within the same second, and without the
        // tiebreaker the prune could drop the newest of them.
        conn.execute(
            "DELETE FROM memory_write_decisions \
             WHERE agent_id = ?1 AND id IN ( \
                 SELECT id FROM memory_write_decisions \
                 WHERE agent_id = ?1 \
                 ORDER BY created_at DESC, rowid DESC \
                 LIMIT -1 OFFSET ?2 \
             )",
            params![agent_id, MAX_DECISIONS_PER_AGENT as i64],
        )
        .map_err(|e| AlephError::config(format!("record_write_decision prune: {e}")))?;

        Ok(())
    }

    /// Read an agent's most recent write decisions, newest first.
    ///
    /// `subject_filter` is a literal substring match over the stored excerpt
    /// (escaped through the shared [`super::escape_like`], so a user asking
    /// about "100%" does not match every row).
    pub fn recent_write_decisions(
        &self,
        agent_id: &str,
        subject_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryWriteDecisionRow>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let filter = subject_filter
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(|f| format!("%{}%", super::escape_like(f)));

        let sql = format!(
            "SELECT action, reason, subject, created_at \
             FROM memory_write_decisions \
             WHERE agent_id = ?1{} \
             ORDER BY created_at DESC, rowid DESC \
             LIMIT ?2",
            if filter.is_some() {
                r" AND subject LIKE ?3 ESCAPE '\'"
            } else {
                ""
            }
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("recent_write_decisions prepare: {e}")))?;

        let map_row = |row: &rusqlite::Row| {
            Ok(MemoryWriteDecisionRow {
                action: row.get("action")?,
                reason: row.get("reason")?,
                subject: row.get("subject")?,
                created_at: row.get("created_at")?,
            })
        };
        let rows = match filter.as_ref() {
            Some(f) => stmt.query_map(params![agent_id, limit as i64, f], map_row),
            None => stmt.query_map(params![agent_id, limit as i64], map_row),
        }
        .map_err(|e| AlephError::config(format!("recent_write_decisions query: {e}")))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(
                row.map_err(|e| AlephError::config(format!("recent_write_decisions row: {e}")))?,
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> SqliteMemoryBackend {
        SqliteMemoryBackend::in_memory().unwrap()
    }

    #[test]
    fn records_both_refusals_and_successes() {
        let db = backend();
        db.record_write_decision("a", "add", MemoryWriteReason::Duplicate, "User likes tabs")
            .unwrap();
        db.record_write_decision("a", "add", MemoryWriteReason::Written, "User likes tabs")
            .unwrap();

        let rows = db.recent_write_decisions("a", None, 10).unwrap();
        assert_eq!(rows.len(), 2);
        // A complete account, not a failure list: the success is in there too,
        // which is what separates "never saved" from "saved, then removed".
        let reasons: Vec<&str> = rows.iter().map(|r| r.reason.as_str()).collect();
        assert!(reasons.contains(&"written"));
        assert!(reasons.contains(&"duplicate"));
    }

    #[test]
    fn decisions_are_scoped_per_agent() {
        let db = backend();
        db.record_write_decision("a", "add", MemoryWriteReason::OverBudget, "fact a")
            .unwrap();
        db.record_write_decision("b", "add", MemoryWriteReason::OverBudget, "fact b")
            .unwrap();
        let rows = db.recent_write_decisions("a", None, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].subject, "fact a");
    }

    #[test]
    fn subject_is_a_bounded_excerpt_not_the_content() {
        let db = backend();
        // Non-ASCII on the truncation boundary: a byte slice would panic here.
        let long = "记".repeat(SUBJECT_EXCERPT_CHARS + 40);
        db.record_write_decision("a", "add", MemoryWriteReason::ScannerRejected, &long)
            .unwrap();
        let rows = db.recent_write_decisions("a", None, 10).unwrap();
        assert_eq!(rows[0].subject.chars().count(), SUBJECT_EXCERPT_CHARS + 1);
        assert!(rows[0].subject.ends_with('…'));
        assert!(rows[0].subject.chars().count() < long.chars().count());
    }

    #[test]
    fn short_subjects_are_stored_whole_without_a_marker() {
        let db = backend();
        db.record_write_decision("a", "remove", MemoryWriteReason::NoMatch, "ghost")
            .unwrap();
        assert_eq!(
            db.recent_write_decisions("a", None, 10).unwrap()[0].subject,
            "ghost"
        );
    }

    #[test]
    fn prune_on_write_keeps_the_newest_rows() {
        let db = backend();
        for i in 0..(MAX_DECISIONS_PER_AGENT + 25) {
            db.record_write_decision("a", "add", MemoryWriteReason::Duplicate, &format!("f{i}"))
                .unwrap();
        }
        let rows = db
            .recent_write_decisions("a", None, MAX_DECISIONS_PER_AGENT * 2)
            .unwrap();
        assert_eq!(rows.len(), MAX_DECISIONS_PER_AGENT);
        // Same-second inserts: only the rowid tiebreaker keeps the newest.
        assert_eq!(
            rows[0].subject,
            format!("f{}", MAX_DECISIONS_PER_AGENT + 24)
        );
        assert!(rows.iter().all(|r| r.subject != "f0"));
    }

    #[test]
    fn subject_filter_matches_literally() {
        let db = backend();
        db.record_write_decision(
            "a",
            "add",
            MemoryWriteReason::Duplicate,
            "prefers 100% tabs",
        )
        .unwrap();
        db.record_write_decision("a", "add", MemoryWriteReason::Duplicate, "prefers spaces")
            .unwrap();

        let hits = db.recent_write_decisions("a", Some("100%"), 10).unwrap();
        assert_eq!(hits.len(), 1, "`%` must match literally, not as a wildcard");
        assert_eq!(hits[0].subject, "prefers 100% tabs");
        // A blank filter browses rather than matching nothing.
        assert_eq!(
            db.recent_write_decisions("a", Some("  "), 10)
                .unwrap()
                .len(),
            2
        );
    }
}
