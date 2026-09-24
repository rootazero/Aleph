//! CRUD operations for `memory_events` table
//!
//! Memory event persistence methods on `StateDatabase`.
//! Follows the same pattern as events.rs (`agent_events`).

use super::StateDatabase;
use crate::error::AlephError;
use crate::memory::events::{EventActor, MemoryEvent, MemoryEventEnvelope, UnpartitionedRows};
use rusqlite::params;

/// The column list every reader selects, in the order
/// [`MemoryEventRow::from_row`] reads it. One copy, so a new column cannot be
/// selected by one reader and mis-indexed by another.
const EVENT_COLUMNS: &str =
    "id, fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id, partition";

impl StateDatabase {
    // =========================================================================
    // Memory Events CRUD
    // =========================================================================

    /// Append a single memory event. Returns the assigned global row ID.
    ///
    /// `seq` is **atomically allocated by SQLite inside the INSERT**, not by the
    /// caller. The caller-supplied `envelope.seq` is ignored — supplying a
    /// pre-computed value would re-introduce a TOCTOU race (two writers
    /// reading `MAX(seq)` back-to-back would both compute the same `seq+1`
    /// and the second `INSERT` would fail with `UNIQUE constraint failed`).
    /// The atomic form (`SELECT COALESCE(MAX(seq),0)+1 ... FROM memory_events
    /// WHERE fact_id = ?1`) evaluates the sub-query and writes the new row in
    /// a single SQLite statement, so concurrent writers serialize through
    /// SQLite's statement-level lock and never collide.
    ///
    /// Callers that need the post-insert `seq` must re-read via
    /// [`get_memory_events_for_fact_unscoped`] (or [`get_memory_event_latest_seq`]);
    /// the append path itself does not surface it.
    pub async fn append_memory_event(
        &self,
        envelope: &MemoryEventEnvelope,
    ) -> Result<i64, AlephError> {
        let envelope = envelope.clone();
        self.with_conn(move |conn| {
            let event_json = serde_json::to_string(&envelope.event)
                .map_err(|e| AlephError::other(format!("Failed to serialize event: {e}")))?;
            let tier = if envelope.event.is_skeleton() {
                "skeleton"
            } else {
                "pulse"
            };

            // Single statement: compute next seq for this fact_id and INSERT in
            // one go. SQLite serializes this through its per-connection write
            // lock, so two concurrent appends on the same fact_id cannot both
            // observe the same MAX(seq) and then collide on UNIQUE(fact_id, seq).
            conn.execute(
                r#"
                INSERT INTO memory_events (fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id, partition)
                SELECT ?1,
                       COALESCE((SELECT MAX(seq) FROM memory_events WHERE fact_id = ?1), 0) + 1,
                       ?3, ?4, ?5, ?6, ?7, ?8, ?9
                "#,
                params![
                    envelope.fact_id,
                    envelope.fact_id, // fact_id appears twice: once for the row, once for the sub-query scope
                    envelope.event.event_type_tag(),
                    event_json,
                    envelope.actor.to_string(),
                    tier,
                    envelope.timestamp,
                    envelope.correlation_id,
                    envelope.partition,
                ],
            )
            .map_err(|e| AlephError::other(format!("Failed to append memory event: {e}")))?;

            Ok(conn.last_insert_rowid())
        })
        .await
    }

    /// Every event for `fact_id` filed under one of `partitions`, ordered by
    /// seq — the read every per-caller face goes through.
    ///
    /// `partitions` is the caller's read set (`project_scope::session_read_ids`
    /// — the org tier plus the caller's own partition, the same derivation on
    /// the tool face and the gateway face). An event filed under any other
    /// partition is absent exactly as if it did not exist, so handing Bob
    /// Alice's fact id yields what a never-written id yields. `unpartitioned`
    /// decides the rows the backfill could not attribute.
    pub async fn get_memory_events_for_fact(
        &self,
        fact_id: &str,
        partitions: &[String],
        unpartitioned: UnpartitionedRows,
    ) -> Result<Vec<MemoryEventEnvelope>, AlephError> {
        let fact_id = fact_id.to_string();
        let partitions_json = serde_json::to_string(partitions)
            .map_err(|e| AlephError::other(format!("Failed to encode partitions: {e}")))?;
        let admit_unpartitioned = i64::from(unpartitioned == UnpartitionedRows::Admit);
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {EVENT_COLUMNS} FROM memory_events \
                     WHERE fact_id = ?1 \
                       AND (partition IN (SELECT value FROM json_each(?2)) \
                            OR (?3 = 1 AND partition IS NULL)) \
                     ORDER BY seq ASC"
                ))
                .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;
            let rows = stmt
                .query_map(
                    params![fact_id, partitions_json, admit_unpartitioned],
                    MemoryEventRow::from_row,
                )
                .map_err(|e| AlephError::other(format!("Failed to query events: {e}")))?;
            collect_envelopes(rows)
        })
        .await
    }

    /// Every event for `fact_id` across ALL partitions — the WRITE side's view
    /// only: `MemoryCommandHandler` folds a fact's whole stream to project it
    /// and scans every stream to reconcile. Any read made on a caller's behalf
    /// goes through [`Self::get_memory_events_for_fact`]; the census
    /// `each_memory_event_reader_has_only_the_consumers_it_was_built_for`
    /// pins that this has no other production caller.
    pub async fn get_memory_events_for_fact_unscoped(
        &self,
        fact_id: &str,
    ) -> Result<Vec<MemoryEventEnvelope>, AlephError> {
        let fact_id = fact_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {EVENT_COLUMNS} FROM memory_events WHERE fact_id = ?1 ORDER BY seq ASC"
                ))
                .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;
            let rows = stmt
                .query_map(params![fact_id], MemoryEventRow::from_row)
                .map_err(|e| AlephError::other(format!("Failed to query events: {e}")))?;
            collect_envelopes(rows)
        })
        .await
    }

    /// The partition a fact's stream is filed under: that of its earliest
    /// stamped row, or `None` when no row carries one (a legacy fact the
    /// backfill could not attribute, or no such fact).
    ///
    /// Exact for ids minted by `MemoryCommandHandler::create_fact` /
    /// `consolidate_facts` (UUIDs — one partition each). A note-path-keyed
    /// stream can span partitions (two users' `preferences/lang` share the
    /// id), which is why the `log_note_*` path passes its partition in and
    /// never asks this.
    pub async fn memory_fact_partition(&self, fact_id: &str) -> Result<Option<String>, AlephError> {
        use rusqlite::OptionalExtension;
        let fact_id = fact_id.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT partition FROM memory_events \
                 WHERE fact_id = ?1 AND partition IS NOT NULL \
                 ORDER BY seq ASC LIMIT 1",
                params![fact_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AlephError::other(format!("Failed to read fact partition: {e}")))
        })
        .await
    }

    /// Get the latest sequence number for a fact.
    pub async fn get_memory_event_latest_seq(&self, fact_id: &str) -> Result<u64, AlephError> {
        let fact_id = fact_id.to_string();
        self.with_conn(move |conn| {
            let result: Option<i64> = conn
                .query_row(
                    "SELECT MAX(seq) FROM memory_events WHERE fact_id = ?1",
                    params![fact_id],
                    |row| row.get(0),
                )
                .map_err(|e| AlephError::other(format!("Failed to get latest seq: {e}")))?;

            // Surface a negative MAX(seq) instead of silently clamping to 0:
            // a corrupt row returning a negative value would otherwise look
            // like "no events" to the caller.
            super::i64_to_u64_count(result.unwrap_or(0), "max_seq")
        })
        .await
    }

    /// List every distinct `fact_id` along with its latest seq.
    ///
    /// Used by `MemoryCommandHandler::reconcile_once` to scan the event log
    /// for divergence against the filesystem projection. Returns pairs in
    /// ascending `fact_id` order so the caller can take a deterministic
    /// prefix of the result and reproduce a partial report across runs.
    ///
    /// No filtering by actor: the reconciler operates across all agents
    /// because divergence is a system-wide invariant (one agent's
    /// filesystem vs the global event log).
    pub async fn list_memory_fact_ids(&self) -> Result<Vec<(String, u64)>, AlephError> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT fact_id, MAX(seq) FROM memory_events GROUP BY fact_id ORDER BY fact_id",
                )
                .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;
            let rows = stmt
                .query_map([], |row| {
                    let fact_id: String = row.get(0)?;
                    let seq: i64 = row.get(1)?;
                    // Negative seq would mean a corrupt page; surface it
                    // rather than silently clamping to 0.
                    Ok((fact_id, u64::try_from(seq).unwrap_or(0)))
                })
                .map_err(|e| AlephError::other(format!("Failed to list fact_ids: {e}")))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| AlephError::other(format!("Row error: {e}")))?);
            }
            Ok(out)
        })
        .await
    }

    /// Check if any migration events exist (indicates migration has been run).
    ///
    /// Recognizes both the new `NoteMigrated` tag and the legacy `FactMigrated`
    /// tag for backward compatibility with pre-R2.2 event stores.
    pub async fn has_migration_events(&self) -> Result<bool, AlephError> {
        let new_count = self.count_memory_events(Some("NoteMigrated")).await?;
        let legacy_count = self.count_memory_events(Some("FactMigrated")).await?;
        Ok(new_count + legacy_count > 0)
    }

    /// Count total memory events, optionally filtered by event type.
    pub async fn count_memory_events(
        &self,
        event_type_filter: Option<&str>,
    ) -> Result<usize, AlephError> {
        let event_type_filter: Option<String> = event_type_filter.map(str::to_string);
        self.with_conn(move |conn| {
            let count: i64 = match event_type_filter.as_deref() {
                Some(et) => conn
                    .query_row(
                        "SELECT COUNT(*) FROM memory_events WHERE event_type = ?1",
                        params![et],
                        |row| row.get(0),
                    )
                    .map_err(|e| AlephError::other(format!("Failed to count events: {e}")))?,
                None => conn
                    .query_row("SELECT COUNT(*) FROM memory_events", [], |row| row.get(0))
                    .map_err(|e| AlephError::other(format!("Failed to count events: {e}")))?,
            };
            // Negative COUNT(*) is impossible in SQLite, but routing through
            // the same helper as the rest of the layer keeps the conversion
            // shape uniform and surfaces any future drift.
            super::i64_to_u64_count(count, "memory_events_count").map(|n| n as usize)
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Internal helper for row mapping
// ---------------------------------------------------------------------------

struct MemoryEventRow {
    id: i64,
    fact_id: String,
    seq: u64,
    event_type: String,
    event_json: String,
    actor: String,
    _tier: String,
    timestamp: i64,
    correlation_id: Option<String>,
    partition: Option<String>,
}

impl MemoryEventRow {
    /// Construct from a rusqlite row.
    /// Expected column order: id, `fact_id`, seq, `event_type`, `event_json`, actor, tier, timestamp, `correlation_id`, `partition`
    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        let seq_i64: i64 = row.get(2)?;
        let seq = u64::try_from(seq_i64).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("memory_events seq must be non-negative, got {seq_i64}"),
                )),
            )
        })?;
        Ok(Self {
            id: row.get(0)?,
            fact_id: row.get(1)?,
            seq,
            event_type: row.get::<_, String>(3)?,
            event_json: row.get(4)?,
            actor: row.get(5)?,
            _tier: row.get::<_, String>(6)?,
            timestamp: row.get(7)?,
            correlation_id: row.get(8)?,
            partition: row.get(9)?,
        })
    }
}

impl MemoryEventRow {
    /// Convert a DB row into an envelope.
    ///
    /// Forward-compat: when `event_json` fails to deserialize as a known
    /// `MemoryEvent` variant (e.g. the variant has been removed in a later
    /// schema), the row is **logged and skipped** rather than raising an
    /// error. This keeps replay resilient to retired event variants.
    ///
    /// - `Ok(Some(envelope))` → row parsed successfully.
    /// - `Ok(None)` → unknown/retired event variant; caller should skip.
    /// - `Err(_)` → unrecoverable (malformed actor, etc.).
    fn into_envelope(self) -> Result<Option<MemoryEventEnvelope>, AlephError> {
        let event: MemoryEvent = match serde_json::from_str(&self.event_json) {
            Ok(event) => event,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    note_path = %self.fact_id,
                    seq = self.seq,
                    event_type = %self.event_type,
                    "skipping unrecognized memory event during replay"
                );
                return Ok(None);
            }
        };
        let actor: EventActor = self
            .actor
            .parse()
            .map_err(|e: String| AlephError::other(e))?;
        Ok(Some(MemoryEventEnvelope {
            id: self.id,
            fact_id: self.fact_id,
            seq: self.seq,
            event,
            actor,
            timestamp: self.timestamp,
            correlation_id: self.correlation_id,
            partition: self.partition,
        }))
    }
}

/// Map query rows to envelopes, skipping retired variants (see
/// [`MemoryEventRow::into_envelope`]). The one body every reader shares.
fn collect_envelopes(
    rows: impl Iterator<Item = rusqlite::Result<MemoryEventRow>>,
) -> Result<Vec<MemoryEventEnvelope>, AlephError> {
    let mut envelopes = Vec::new();
    for row in rows {
        let row = row.map_err(|e| AlephError::other(format!("Row error: {e}")))?;
        if let Some(envelope) = row.into_envelope()? {
            envelopes.push(envelope);
        }
    }
    Ok(envelopes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactSource, NoteType};
    use crate::resilience::database::StateDatabase;

    fn make_test_db() -> StateDatabase {
        StateDatabase::in_memory().unwrap()
    }

    fn make_created_event(fact_id: &str) -> MemoryEvent {
        MemoryEvent::NoteCreated {
            note_path: fact_id.into(),
            content: "User prefers Rust".into(),
            note_type: NoteType::Preference,
            path: "aleph://user/preferences/language".into(),
            namespace: "owner".into(),
            agent: "default".into(),
            source: FactSource::Extracted,
            source_memory_ids: vec!["mem-001".into()],
        }
    }

    #[tokio::test]
    async fn test_append_and_retrieve_event() {
        let db = make_test_db();
        let event = make_created_event("fact-001");
        let envelope =
            MemoryEventEnvelope::new("fact-001".into(), 1, event, EventActor::Agent, None);

        let id = db.append_memory_event(&envelope).await.unwrap();
        assert!(id > 0);

        let events = db
            .get_memory_events_for_fact_unscoped("fact-001")
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].fact_id, "fact-001");
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].id, id);
    }

    #[tokio::test]
    async fn test_latest_seq() {
        let db = make_test_db();
        assert_eq!(
            db.get_memory_event_latest_seq("nonexistent").await.unwrap(),
            0
        );

        for i in 1..=3 {
            let envelope = MemoryEventEnvelope::new(
                "fact-seq".into(),
                i,
                make_created_event("fact-seq"),
                EventActor::Agent,
                None,
            );
            db.append_memory_event(&envelope).await.unwrap();
        }
        assert_eq!(db.get_memory_event_latest_seq("fact-seq").await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_count_events() {
        let db = make_test_db();
        let e1 = MemoryEventEnvelope::new(
            "f1".into(),
            1,
            make_created_event("f1"),
            EventActor::Agent,
            None,
        );
        let e2 = MemoryEventEnvelope::new(
            "f2".into(),
            1,
            MemoryEvent::NoteAccessed {
                note_path: "f2".into(),
                query: None,
                relevance_score: None,
                used_in_response: false,
                new_access_count: 1,
            },
            EventActor::Agent,
            None,
        );
        db.append_memory_event(&e1).await.unwrap();
        db.append_memory_event(&e2).await.unwrap();

        assert_eq!(db.count_memory_events(None).await.unwrap(), 2);
        assert_eq!(
            db.count_memory_events(Some("NoteCreated")).await.unwrap(),
            1
        );
        assert_eq!(
            db.count_memory_events(Some("NoteAccessed")).await.unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn replay_skips_unknown_event_variants() {
        // Insert a raw row with an event_json that no MemoryEvent variant matches,
        // simulating a retired variant written by an older schema version.
        let db = make_test_db();

        {
            let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute(
                r#"INSERT INTO memory_events
                   (fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
                params![
                    "fact-orphan",
                    1u64,
                    "AbsolutelyNotAVariant",
                    r#"{"AbsolutelyNotAVariant":{"foo":1}}"#,
                    "agent",
                    "pulse",
                    1_700_000_000i64,
                    Option::<String>::None,
                ],
            )
            .unwrap();
        }

        // Replay must not error — unknown variant is logged and skipped.
        let events = db
            .get_memory_events_for_fact_unscoped("fact-orphan")
            .await
            .expect("unknown variant must not error replay");
        assert!(
            events.is_empty(),
            "unknown variant row must be skipped (got {} events)",
            events.len()
        );

        // Also verify that known events still replay alongside skipped unknowns.
        let known = MemoryEventEnvelope::new(
            "fact-orphan".into(),
            2,
            make_created_event("fact-orphan"),
            EventActor::Agent,
            None,
        );
        db.append_memory_event(&known).await.unwrap();

        let events = db
            .get_memory_events_for_fact_unscoped("fact-orphan")
            .await
            .expect("mixed replay must succeed");
        assert_eq!(events.len(), 1, "only the known event should survive");
        assert_eq!(events[0].seq, 2);
    }

    #[tokio::test]
    async fn test_atomic_seq_allocation_is_monotonic() {
        // Two sequential appends on the same `fact_id` must produce strictly
        // increasing seq values (1, 2) — even though both envelopes supply
        // the same caller-side seq=1. The DB's atomic
        // `SELECT COALESCE(MAX(seq),0)+1` overrides the caller-supplied value,
        // so `UNIQUE(fact_id, seq)` cannot fire through the public append
        // path (the original race window that motivated the constraint).
        let db = make_test_db();
        let e1 = MemoryEventEnvelope::new(
            "fact-dup".into(),
            1,
            make_created_event("fact-dup"),
            EventActor::Agent,
            None,
        );
        db.append_memory_event(&e1).await.unwrap();

        // Same fact_id + caller-supplied seq=1 should NOT cause failure:
        // the atomic allocation ignores envelope.seq and assigns the next
        // monotonic value.
        let e2 = MemoryEventEnvelope::new(
            "fact-dup".into(),
            1,
            make_created_event("fact-dup"),
            EventActor::Agent,
            None,
        );
        db.append_memory_event(&e2).await.unwrap();

        let events = db
            .get_memory_events_for_fact_unscoped("fact-dup")
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
        assert!(
            events[1].seq > events[0].seq,
            "atomic allocation must produce strictly monotonic seq"
        );
    }

    #[tokio::test]
    async fn test_unique_constraint_remains_for_direct_sql_bypass() {
        // The `UNIQUE(fact_id, seq)` schema constraint must still trigger when
        // a raw `INSERT` bypasses `append_memory_event` (e.g. a future bug,
        // ad-hoc migration script, or out-of-band writer). Defense-in-depth:
        // even if the application-level atomic allocation regresses, the
        // schema itself rejects duplicate (fact_id, seq) pairs.
        let db = make_test_db();
        let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());

        conn.execute(
            "INSERT INTO memory_events (fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id) \
             VALUES ('fact-x', 1, 'Test', '{}', 'Agent', 'pulse', 0, NULL)",
            [],
        )
        .expect("first direct INSERT must succeed");

        let result = conn.execute(
            "INSERT INTO memory_events (fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id) \
             VALUES ('fact-x', 1, 'Test', '{}', 'Agent', 'pulse', 0, NULL)",
            [],
        );
        assert!(
            result.is_err(),
            "UNIQUE(fact_id, seq) must fire on direct SQL bypass; got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_concurrent_append_assigns_unique_seqs() {
        // Spawn N concurrent tasks appending to the SAME fact_id. Each task
        // submits an envelope with caller-side seq=1 (the worst case for the
        // old TOCTOU race). The atomic allocation must produce N distinct
        // monotonic seqs, with no UNIQUE-constraint failures and no lost
        // writes.
        use std::sync::Arc;
        use tokio::task::JoinSet;

        let db = Arc::new(make_test_db());
        const N: usize = 32;

        let mut set = JoinSet::new();
        for i in 0..N {
            let db = Arc::clone(&db);
            set.spawn(async move {
                let event = make_created_event("fact-race");
                let envelope = MemoryEventEnvelope::new(
                    "fact-race".into(),
                    1, // all callers race on the same value
                    event,
                    EventActor::Agent,
                    None,
                );
                db.append_memory_event(&envelope)
                    .await
                    .map_err(|e| (i, e.to_string()))
            });
        }

        let mut errors = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Err((i, msg)) = res.unwrap() {
                errors.push(format!("task {i}: {msg}"));
            }
        }
        assert!(
            errors.is_empty(),
            "concurrent appends must not fail: {errors:?}"
        );

        let events = db
            .get_memory_events_for_fact_unscoped("fact-race")
            .await
            .unwrap();
        assert_eq!(
            events.len(),
            N,
            "all {N} concurrent appends must persist; got {} events",
            events.len()
        );
        // Strictly monotonic, 1..=N
        for (i, e) in events.iter().enumerate() {
            assert_eq!(e.seq as usize, i + 1, "event {i} must have seq {}", i + 1);
        }
    }

    /// Both open paths run the partition migration: `in_memory()` used to keep
    /// its own copy of the migration list, and a new migration added to only
    /// one of them is a test database that disagrees with production.
    #[test]
    fn the_partition_index_exists_on_a_freshly_opened_database() {
        let db = make_test_db();
        let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());
        let columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_index_info('idx_me_partition_fact')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            columns, 2,
            "idx_me_partition_fact(partition, fact_id) must exist"
        );
    }

    #[tokio::test]
    async fn an_appended_event_round_trips_its_partition() {
        let db = make_test_db();
        let stamped = MemoryEventEnvelope::new(
            "fact-p".into(),
            1,
            make_created_event("fact-p"),
            EventActor::Agent,
            None,
        )
        .in_partition(Some("main__u-alice".into()));
        db.append_memory_event(&stamped).await.unwrap();

        let events = db
            .get_memory_events_for_fact_unscoped("fact-p")
            .await
            .unwrap();
        assert_eq!(events[0].partition.as_deref(), Some("main__u-alice"));
        assert_eq!(
            db.memory_fact_partition("fact-p").await.unwrap().as_deref(),
            Some("main__u-alice")
        );
        assert_eq!(
            db.memory_fact_partition("never-written").await.unwrap(),
            None
        );
    }

    fn stamped(fact_id: &str, partition: Option<&str>) -> MemoryEventEnvelope {
        MemoryEventEnvelope::new(
            fact_id.into(),
            1,
            make_created_event(fact_id),
            EventActor::Agent,
            None,
        )
        .in_partition(partition.map(str::to_string))
    }

    fn read_set(principal: &str) -> Vec<String> {
        vec!["main".to_string(), format!("main__{principal}")]
    }

    /// D7, the two-principal case at the store: a note-path id both Alice and
    /// Bob wrote is two histories, and a third principal holding the id reads
    /// exactly what a never-written id reads.
    #[tokio::test]
    async fn a_fact_id_from_another_partition_reads_as_absent() {
        let db = make_test_db();
        db.append_memory_event(&stamped("preferences/lang", Some("main__u-alice")))
            .await
            .unwrap();
        db.append_memory_event(&stamped("preferences/lang", Some("main__u-bob")))
            .await
            .unwrap();

        let seen = |events: Vec<MemoryEventEnvelope>| {
            events.into_iter().map(|e| e.partition).collect::<Vec<_>>()
        };
        assert_eq!(
            seen(
                db.get_memory_events_for_fact(
                    "preferences/lang",
                    &read_set("u-alice"),
                    UnpartitionedRows::Refuse
                )
                .await
                .unwrap()
            ),
            vec![Some("main__u-alice".to_string())]
        );
        assert_eq!(
            seen(
                db.get_memory_events_for_fact(
                    "preferences/lang",
                    &read_set("u-bob"),
                    UnpartitionedRows::Refuse
                )
                .await
                .unwrap()
            ),
            vec![Some("main__u-bob".to_string())]
        );
        assert!(db
            .get_memory_events_for_fact(
                "preferences/lang",
                &read_set("u-carol"),
                UnpartitionedRows::Refuse
            )
            .await
            .unwrap()
            .is_empty());
        assert!(db
            .get_memory_events_for_fact(
                "never-written",
                &read_set("u-carol"),
                UnpartitionedRows::Refuse
            )
            .await
            .unwrap()
            .is_empty());
    }

    /// Rows with no partition are the legacy owner's (`owner_or_legacy`):
    /// admitted only when the caller's decision says so. The org tier is in
    /// every scoped read set, and an empty read set reads nothing stamped.
    #[tokio::test]
    async fn unattributed_rows_follow_the_callers_admission() {
        let db = make_test_db();
        db.append_memory_event(&stamped("legacy", None))
            .await
            .unwrap();
        db.append_memory_event(&stamped("org", Some("main")))
            .await
            .unwrap();
        let alice = read_set("u-alice");

        assert!(db
            .get_memory_events_for_fact("legacy", &alice, UnpartitionedRows::Refuse)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            db.get_memory_events_for_fact("legacy", &alice, UnpartitionedRows::Admit)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.get_memory_events_for_fact("org", &alice, UnpartitionedRows::Refuse)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(db
            .get_memory_events_for_fact("org", &[], UnpartitionedRows::Admit)
            .await
            .unwrap()
            .is_empty());
    }

    /// 判据 §9: enumerate the readers. The unscoped reader answers for every
    /// partition at once, so only the write side (`MemoryCommandHandler`'s
    /// folds and reconciler) may call it; the per-caller reader has one face
    /// today (the traveler, behind `memory_timeline`). A new face shows up
    /// here instead of shipping unreviewed.
    #[test]
    fn each_memory_event_reader_has_only_the_consumers_it_was_built_for() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let unscoped = crate::utils::source_scan::files_whose_production_code_reads(
            &root,
            "get_memory_events_for_fact_unscoped",
        );
        assert_eq!(
            unscoped.into_iter().collect::<Vec<_>>(),
            vec!["src/memory/events/handler.rs".to_string()],
            "only the write side may read events across partitions"
        );
        let scoped = crate::utils::source_scan::files_whose_production_code_reads(
            &root,
            "get_memory_events_for_fact",
        );
        assert_eq!(
            scoped.into_iter().collect::<Vec<_>>(),
            vec!["src/memory/events/traveler.rs".to_string()],
            "a new per-caller face must derive its partitions the way memory_timeline does \
             (session_read_ids + unattributed_memory_events_for)"
        );
    }
}
