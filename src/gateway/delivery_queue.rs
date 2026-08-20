//! Durable outbound delivery queue — persistent, retrying channel delivery.
//!
//! # Why
//!
//! [`ChannelRegistry::send`](super::channel_registry::ChannelRegistry::send)
//! retries only [`ChannelError::RateLimited`], in-memory, for a bounded window
//! (`SendRetryPolicy`). Every other failure that means *the message was never
//! delivered* — most importantly [`ChannelError::NotConnected`] while a channel
//! is mid-reconnect — drops the outbound message, and **nothing survives a
//! daemon restart**. For an assistant whose core promise is "AI comes to you"
//! (architectural redline R5: proactive multi-channel push), a lost Daemon
//! notification or agent reply is a silent correctness failure. The existing
//! `SendRetryPolicy` doc already admits that msteams / feishu / signal "simply
//! dropped it, losing the reply".
//!
//! This module adds an **opt-in** SQLite-backed queue: when a send fails with a
//! *definitely-not-delivered* error and a store is attached, the message is
//! persisted; a background drain task replays due records through the registry
//! with exponential backoff until delivery succeeds, the message exhausts its
//! attempt budget, or it is provably undeliverable.
//!
//! # Mapped from
//!
//! openclaw's `src/infra/delivery-queue-sqlite.ts` + `src/infra/outbound/`
//! (SQLite-backed, survives restarts, drains on backoff). Aleph improves on it
//! with type-safe, **duplicate-safe** error classification: only pre-delivery
//! failures ([`NotConnected`](ChannelError::NotConnected) /
//! [`RateLimited`](ChannelError::RateLimited)) are retried. Ambiguous errors
//! (`SendFailed` — the message may already be on the wire) are never retried,
//! preserving the at-most-once guarantee the registry deliberately maintains.
//!
//! # The crash window
//!
//! That at-most-once guarantee used to hold only for failures the transport
//! *reported*. A daemon that exits between a successful `send` and the
//! `mark_delivered` that settles the row left a still-pending, already-due
//! record behind — replayed verbatim on the next boot, i.e. a duplicate. The
//! outcome of an interrupted attempt is exactly as ambiguous as
//! [`SendFailed`](ChannelError::SendFailed), so it gets the same verdict:
//! [`DeliveryStore::mark_inflight`] stamps the row immediately before the send
//! crosses the transport boundary, and [`DeliveryStore::reconcile_inflight`]
//! (run once per process by [`spawn_drain`]) retires every surviving stamp into
//! the dead-letter table as [`DeadLetterReason::UnknownOutcome`] instead of
//! re-sending it. This mirrors openclaw's
//! `markDeliveryPlatformSendAttemptStarted` / `needsUnknownSendReconciliation`
//! pair, minus its multi-process lease machinery (Aleph enforces a
//! single-drainer invariant instead — see [`DeliveryStore::claim_due`]).
//!
//! # Layering (no reference cycle)
//!
//! [`DeliveryStore`] owns only the `SQLite` handle — it holds *no* reference to
//! the registry. [`ChannelRegistry`](super::channel_registry::ChannelRegistry)
//! holds `Option<Arc<DeliveryStore>>` and enqueues on transient failure. The
//! drain task ([`drain_loop`] / [`spawn_drain`]) is a free function holding
//! `Arc<ChannelRegistry>` + `Arc<DeliveryStore>`. The store never points back,
//! so there is no `Arc` cycle.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::channel::{ChannelError, ChannelId, OutboundMessage};
use super::channel_registry::ChannelRegistry;

/// Tuning for the durable delivery queue and its drain task.
#[derive(Debug, Clone)]
pub struct DeliveryQueueConfig {
    /// Maximum delivery attempts before a record is permanently dropped.
    pub max_attempts: u32,
    /// Delay before the first retry (and the floor of the backoff curve).
    pub initial_backoff: Duration,
    /// Upper bound on a single retry delay.
    pub max_backoff: Duration,
    /// Multiplier applied per consecutive failed attempt.
    pub backoff_factor: f64,
    /// How often the drain task wakes to look for due records.
    pub tick: Duration,
    /// Maximum records claimed per drain tick.
    pub batch: usize,
    /// Hard cap on stored records (CWE-400 defense). The oldest rows are
    /// evicted first when a new enqueue would exceed the cap.
    pub max_queue_len: i64,
    /// Hard cap on a single serialized payload (CWE-400 defense, *by bytes*).
    ///
    /// `max_queue_len` bounds the row **count**, which is the wrong dimension
    /// for this table: [`OutboundMessage`] carries `Vec<Attachment>`, and an
    /// [`Attachment`](super::channel::Attachment) may hold inline `data:
    /// Vec<u8>` — serialized to JSON that is several bytes of text per byte of
    /// image. A handful of media pushes to a wedged channel could therefore
    /// grow `delivery.db` by hundreds of megabytes while the row count stayed
    /// in single digits. An over-cap payload is dead-lettered on the spot
    /// ([`DeadLetterReason::PayloadTooLarge`]) rather than silently dropped, so
    /// the operator still sees *what* was too big.
    pub max_payload_bytes: usize,
    /// Most queued records one live [`send`](ChannelRegistry::send) will flush
    /// ahead of itself to keep a conversation in order (see
    /// [`flush_conversation`]). `0` disables the inline flush entirely.
    ///
    /// This bounds how long a user-facing send can block behind its own
    /// backlog: the flush stops at the first failure anyway, so the cap only
    /// bites when the channel is healthy and the backlog is deep — the case
    /// where the background drain is about to deliver them regardless. Past the
    /// cap the send proceeds live and may overtake what is still queued, which
    /// is logged rather than silently accepted.
    pub inline_flush_max: usize,
}

impl Default for DeliveryQueueConfig {
    fn default() -> Self {
        Self {
            max_attempts: 10,
            initial_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(300),
            backoff_factor: 2.0,
            tick: Duration::from_secs(5),
            batch: 32,
            max_queue_len: 10_000,
            max_payload_bytes: 1_048_576,
            inline_flush_max: 8,
        }
    }
}

/// TOML-facing tuning for the durable delivery queue (`[gateway.delivery_queue]`).
///
/// Mirrors [`DeliveryQueueConfig`] but uses plain seconds / scalars so it
/// round-trips cleanly through TOML (the runtime struct stores [`Duration`]s and
/// is built from this via [`to_runtime`](Self::to_runtime)). Field names and
/// defaults match the runtime struct one-for-one, so an empty
/// `[gateway.delivery_queue]` table (or none at all) is byte-identical to the
/// historic hardcoded [`DeliveryQueueConfig::default`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeliveryQueueTomlConfig {
    /// Maximum delivery attempts before a record is permanently dropped.
    pub max_attempts: u32,
    /// Delay (seconds) before the first retry and floor of the backoff curve.
    pub initial_backoff_secs: u64,
    /// Upper bound (seconds) on a single retry delay.
    pub max_backoff_secs: u64,
    /// Multiplier applied per consecutive failed attempt.
    pub backoff_factor: f64,
    /// How often (seconds) the drain task wakes to look for due records.
    pub tick_secs: u64,
    /// Maximum records claimed per drain tick.
    pub batch: usize,
    /// Hard cap on stored records (CWE-400 defense).
    pub max_queue_len: i64,
    /// Hard cap on one serialized payload, in bytes (CWE-400 defense).
    pub max_payload_bytes: usize,
    /// Most queued records one live send flushes ahead of itself (`0` = off).
    pub inline_flush_max: usize,
}

impl Default for DeliveryQueueTomlConfig {
    fn default() -> Self {
        let d = DeliveryQueueConfig::default();
        Self {
            max_attempts: d.max_attempts,
            initial_backoff_secs: d.initial_backoff.as_secs(),
            max_backoff_secs: d.max_backoff.as_secs(),
            backoff_factor: d.backoff_factor,
            tick_secs: d.tick.as_secs(),
            batch: d.batch,
            max_queue_len: d.max_queue_len,
            max_payload_bytes: d.max_payload_bytes,
            inline_flush_max: d.inline_flush_max,
        }
    }
}

impl DeliveryQueueTomlConfig {
    /// Build the runtime [`DeliveryQueueConfig`], clamping every field to a sane
    /// floor so a hostile or fat-fingered TOML cannot wedge the drain task.
    ///
    /// In particular `tick_secs = 0` would turn [`tokio::time::interval`] into a
    /// busy-loop, and `initial_backoff_secs = 0` would reschedule a still-down
    /// channel as immediately-due — both are floored to 1s. `max_backoff` is
    /// raised to at least `initial_backoff`, `backoff_factor` to ≥ 1.0 (so the
    /// curve never *shrinks*), and the count fields to ≥ 1.
    #[must_use]
    pub fn to_runtime(&self) -> DeliveryQueueConfig {
        let initial = self.initial_backoff_secs.max(1);
        DeliveryQueueConfig {
            max_attempts: self.max_attempts.max(1),
            initial_backoff: Duration::from_secs(initial),
            max_backoff: Duration::from_secs(self.max_backoff_secs.max(initial)),
            backoff_factor: if self.backoff_factor >= 1.0 {
                self.backoff_factor
            } else {
                1.0
            },
            tick: Duration::from_secs(self.tick_secs.max(1)),
            batch: self.batch.max(1),
            max_queue_len: self.max_queue_len.max(1),
            // A zero/absurdly-small byte cap would dead-letter *every* outbound
            // message including plain text, turning the safety valve into an
            // outage. Floor it at a value that comfortably holds any text-only
            // push plus its metadata.
            max_payload_bytes: self.max_payload_bytes.max(4096),
            // Deliberately *not* floored at 1: `0` is the meaningful "never
            // flush inline" setting, for an operator who would rather keep
            // `send` latency independent of queue depth than have ordering.
            inline_flush_max: self.inline_flush_max,
        }
    }
}

/// A single persisted outbound delivery, rehydrated for a drain attempt.
#[derive(Debug, Clone)]
pub struct DeliveryRecord {
    /// `SQLite` row id (primary key).
    pub id: i64,
    /// Target channel id (e.g. `telegram`, `signal`).
    pub channel_id: String,
    /// The message to (re)deliver.
    pub message: OutboundMessage,
    /// Number of attempts already made (0 on first enqueue).
    pub attempts: u32,
}

/// Result of admitting an outbound message to the durable queue.
///
/// Distinguishes "persisted, will be retried" from "refused and dead-lettered"
/// so the caller's log line matches what actually happened — the previous bare
/// `i64` could only say "queued for durable retry".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// Persisted for retry; carries the new row id.
    Queued(i64),
    /// Over [`DeliveryQueueConfig::max_payload_bytes`]; dead-lettered instead.
    TooLarge {
        /// Serialized size that tripped the cap.
        bytes: usize,
    },
}

impl EnqueueOutcome {
    /// The row id, when the message was actually admitted.
    #[must_use]
    pub const fn queued_id(self) -> Option<i64> {
        match self {
            Self::Queued(id) => Some(id),
            Self::TooLarge { .. } => None,
        }
    }
}

/// What a redrive actually did. Reporting the two *skip* reasons separately is
/// the point: "moved 3 of 12" is indistinguishable from a bug unless the
/// remaining 9 are attributed to a rule (not duplicate-safe) or to a transient
/// condition the operator can clear (live queue full — retry after it drains).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedriveOutcome {
    /// Records moved back into the live queue, immediately due.
    pub moved: u64,
    /// Replay-safe records left behind because the live queue is at
    /// `max_queue_len`. A later redrive picks them up.
    pub skipped_capacity: u64,
    /// Records left behind because replaying them could double-send
    /// ([`DeadLetterReason::replay_safe`] is false). Never moved implicitly.
    pub skipped_unsafe: u64,
}

/// Field bundle for [`DeliveryStore::insert_dead_letter`] — a struct rather
/// than eight positional parameters so a future column cannot be silently
/// swapped with its neighbour.
struct DeadLetterInsert<'a> {
    channel_id: &'a str,
    /// Carried so a redrive can restore the row's place in its conversation's
    /// ordering without re-parsing the payload.
    conversation_id: &'a str,
    payload: &'a str,
    attempts: u32,
    last_error: &'a str,
    reason: DeadLetterReason,
    created_at: i64,
    died_at: i64,
}

/// Runtime observability snapshot of the durable queue, surfaced to operators
/// and the LLM (redline R8: Aleph's own operations are inspectable) so a
/// silently-growing outbound backlog — a lost "AI comes to you" push (R5) — is
/// no longer invisible. Built in a single locked pass by [`DeliveryStore::stats`]
/// from columns the queue already maintains; nothing new is persisted for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryQueueStats {
    /// Pending records awaiting (re)delivery — the live queue depth.
    pub pending: i64,
    /// Subset of `pending` whose `next_attempt_at` is already due.
    pub due_now: i64,
    /// Age in seconds of the oldest pending record (by `created_at`), or `None`
    /// when the queue is empty. A large value flags a wedged channel.
    pub oldest_age_secs: Option<i64>,
    /// Pending count per channel id, descending — pinpoints which transport is
    /// backed up without dumping every record.
    pub per_channel: Vec<(String, i64)>,
    /// Records retired to the dead-letter table rather than silently deleted
    /// (forensic trail; parity-plus over openclaw).
    pub dead_lettered: i64,
    /// Subset of `dead_lettered` a redrive would actually replay
    /// ([`DeadLetterReason::replay_safe`]). Reported separately because the
    /// gap between the two numbers *is* the answer to "why did redrive move
    /// fewer than I can see?" — without it, an operator staring at 12 dead
    /// letters and a redrive that moved 3 has no way to tell a bug from the
    /// duplicate-safety rule doing its job.
    pub dead_lettered_replayable: i64,
}

/// Why a delivery stopped being retried.
///
/// The distinction that matters is **not** severity but *replay safety*: it is
/// the single input to [`DeliveryStore::redrive_dead_letters`], which used to
/// rest on the blanket claim "every dead letter is duplicate-safe by
/// construction". That claim held only while the sole producer was an exhausted
/// [`should_enqueue`] retry budget. Once terminal failures and interrupted
/// attempts also land here — which is the whole point of a forensic trail — the
/// invariant has to be carried per record instead of asserted per table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadLetterReason {
    /// Exhausted the retry budget on duplicate-safe transient errors.
    /// Definitely never delivered ⇒ replay-safe.
    Exhausted,
    /// A drain attempt returned an error that may have landed anyway
    /// (`SendFailed` / `Internal`). Replaying risks a duplicate.
    Ambiguous,
    /// A drain attempt returned a definitely-not-delivered but non-transient
    /// error (`MessageTooLarge` / `ConfigError` / …). Replay-safe, though a
    /// retry only succeeds once the operator fixes the cause.
    Permanent,
    /// The daemon exited between "attempt started" and "outcome recorded".
    /// Exactly as ambiguous as [`Ambiguous`](Self::Ambiguous) — see the module
    /// docs' *crash window* section.
    UnknownOutcome,
    /// The serialized payload exceeded [`DeliveryQueueConfig::max_payload_bytes`]
    /// and was never admitted to the live queue. Never delivered, but replaying
    /// it verbatim would hit the same cap, so it is not offered for redrive.
    PayloadTooLarge,
}

impl DeadLetterReason {
    /// Every variant, in declaration order. Single source for the callers that
    /// need to enumerate reasons (the replay-safe projection in
    /// [`DeliveryStore::stats`] / [`DeliveryStore::redrive_dead_letters`]); a
    /// `#[cfg(test)]` exhaustive match keeps it honest when a variant is added.
    pub const ALL: &'static [Self] = &[
        Self::Exhausted,
        Self::Ambiguous,
        Self::Permanent,
        Self::UnknownOutcome,
        Self::PayloadTooLarge,
    ];

    /// The database tokens a redrive is willing to move.
    #[must_use]
    pub fn replay_safe_tokens() -> Vec<&'static str> {
        Self::ALL
            .iter()
            .filter(|r| r.replay_safe())
            .map(|r| r.as_str())
            .collect()
    }

    /// Whether redriving this record can *not* produce a duplicate delivery.
    #[must_use]
    pub const fn replay_safe(self) -> bool {
        matches!(self, Self::Exhausted | Self::Permanent)
    }

    /// Stable wire/database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exhausted => "exhausted",
            Self::Ambiguous => "ambiguous",
            Self::Permanent => "permanent",
            Self::UnknownOutcome => "unknown_outcome",
            Self::PayloadTooLarge => "payload_too_large",
        }
    }

    /// Parse a database token. An unknown token (schema drift, downgrade) is
    /// read as [`Ambiguous`](Self::Ambiguous) — the conservative side, since
    /// that is the one redrive refuses by default.
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "exhausted" => Self::Exhausted,
            "permanent" => Self::Permanent,
            "unknown_outcome" => Self::UnknownOutcome,
            "payload_too_large" => Self::PayloadTooLarge,
            _ => Self::Ambiguous,
        }
    }
}

/// A delivery that will not be retried again, retained for forensics instead of
/// being hard-deleted. Read back via [`DeliveryStore::recent_dead_letters`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetter {
    /// Target channel id the message could never be delivered to.
    pub channel_id: String,
    /// The undelivered message.
    pub message: OutboundMessage,
    /// Attempts made before giving up.
    pub attempts: u32,
    /// Last error observed before the record was retired.
    pub last_error: String,
    /// Why retrying stopped — and therefore whether a redrive is safe.
    pub reason: DeadLetterReason,
    /// Unix epoch seconds when the record was first enqueued.
    pub created_at: i64,
    /// Unix epoch seconds when the record was dead-lettered.
    pub died_at: i64,
}

/// Returns `true` when an outbound failure means the message was *definitely
/// not delivered*, so a retry cannot produce a duplicate.
///
/// Only [`NotConnected`](ChannelError::NotConnected) (channel down /
/// reconnecting) and [`RateLimited`](ChannelError::RateLimited) (a pre-delivery
/// `429`) qualify. [`SendFailed`](ChannelError::SendFailed) and
/// [`Internal`](ChannelError::Internal) are ambiguous — the message may already
/// be on the wire — and [`MessageTooLarge`](ChannelError::MessageTooLarge) /
/// [`ConfigError`](ChannelError::ConfigError) are permanent. None of those are
/// retried.
#[must_use]
pub const fn should_enqueue(err: &ChannelError) -> bool {
    matches!(
        err,
        ChannelError::NotConnected(_) | ChannelError::RateLimited { .. }
    )
}

/// Classify a *terminal* drain failure — one [`should_enqueue`] refuses to
/// retry — into the dead-letter reason that records whether replaying it is
/// safe.
///
/// The split is the same one `should_enqueue` makes, read for a different
/// question: `should_enqueue` asks "may I retry this automatically?", this asks
/// "may a human ask me to retry it later?". `SendFailed` / `Internal` are
/// ambiguous (the message may already be on the wire); everything else in this
/// arm is definitely-not-delivered but not transient.
#[must_use]
pub const fn terminal_reason(err: &ChannelError) -> DeadLetterReason {
    match err {
        ChannelError::SendFailed(_) | ChannelError::Internal(_) => DeadLetterReason::Ambiguous,
        _ => DeadLetterReason::Permanent,
    }
}

/// Current wall-clock time as unix epoch seconds (saturating to 0 on a clock
/// before the epoch — defensive, never panics).
#[must_use]
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Deterministic exponential backoff: `min(initial * factor^attempt, max)`.
///
/// Jitter is intentionally omitted — unlike reconnect storms, per-record DB
/// scheduling does not benefit from decorrelation, and determinism keeps the
/// drain test exact.
fn backoff_delay(cfg: &DeliveryQueueConfig, attempt: u32) -> Duration {
    let factor = cfg.backoff_factor.powi(attempt as i32);
    let mut secs = cfg.initial_backoff.as_secs_f64() * factor;
    let max = cfg.max_backoff.as_secs_f64();
    if secs > max {
        secs = max;
    }
    Duration::from_secs_f64(secs)
}

/// SQLite-backed store for pending outbound deliveries.
///
/// Pure persistence: it knows nothing about channels or the registry, which is
/// what keeps the ownership graph acyclic (see module docs).
pub struct DeliveryStore {
    conn: Mutex<Connection>,
    config: DeliveryQueueConfig,
    /// Runtime half of the single-drainer invariant that keeps [`claim_due`]'s
    /// non-atomic SELECT correct (see its `# Concurrency` docs). Flipped `true`
    /// by the first [`spawn_drain`] via [`Self::try_claim_drainer`]; a second
    /// spawn on the same store is refused loudly instead of racing the first
    /// drainer into duplicate deliveries.
    drain_spawned: AtomicBool,
    /// Serializes every claim→deliver→settle sequence against this store.
    ///
    /// [`claim_due`](Self::claim_due) / [`claim_conversation`](Self::claim_conversation)
    /// are plain SELECTs with no row lease, so two overlapping passes would
    /// claim the same row twice and deliver it twice. There are now two
    /// producers of such a pass — the background [`drain_loop`] and the
    /// opportunistic [`flush_conversation`] that runs ahead of a live send —
    /// which is exactly the "a second drainer is introduced" case
    /// [`claim_due`] warns about. A process-local async mutex is the whole fix
    /// rather than a `claimed_until` column, because the single-drainer
    /// invariant this store relies on is *already* process-local (enforced by
    /// [`try_claim_drainer`](Self::try_claim_drainer) over an in-memory flag):
    /// a second process was never a supported deployment, so a durable lease
    /// would buy nothing and cost a migration.
    drain_gate: tokio::sync::Mutex<()>,
}

impl DeliveryStore {
    /// Open (creating if absent) the delivery database at `path`.
    pub fn open(path: &Path, config: DeliveryQueueConfig) -> rusqlite::Result<Self> {
        let conn = crate::utils::sqlite_open::open_sqlite_safe(path)?;
        Self::init(conn, config)
    }

    /// Open an ephemeral in-memory store (tests).
    pub fn open_in_memory(config: DeliveryQueueConfig) -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn, config)
    }

    fn init(conn: Connection, config: DeliveryQueueConfig) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS outbound_deliveries (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id      TEXT    NOT NULL,
                payload         TEXT    NOT NULL,
                attempts        INTEGER NOT NULL DEFAULT 0,
                next_attempt_at INTEGER NOT NULL,
                created_at      INTEGER NOT NULL,
                last_error      TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_outbound_due
                ON outbound_deliveries(next_attempt_at);
            CREATE TABLE IF NOT EXISTS dead_letters (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id      TEXT    NOT NULL,
                payload         TEXT    NOT NULL,
                attempts        INTEGER NOT NULL,
                last_error      TEXT,
                created_at      INTEGER NOT NULL,
                died_at         INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_dead_letters_died
                ON dead_letters(died_at);",
        )?;
        // Additive migrations for stores created before these columns existed.
        // `ADD COLUMN` has no `IF NOT EXISTS` in SQLite, and a duplicate-column
        // error is the expected outcome on an already-migrated database — the
        // only failure worth propagating would be a genuinely unusable file,
        // which the `CREATE TABLE` batch above has already proven otherwise.
        for stmt in [
            "ALTER TABLE outbound_deliveries ADD COLUMN inflight_since INTEGER",
            "ALTER TABLE outbound_deliveries ADD COLUMN conversation_id TEXT",
            "ALTER TABLE dead_letters ADD COLUMN reason TEXT NOT NULL DEFAULT 'exhausted'",
            "ALTER TABLE dead_letters ADD COLUMN conversation_id TEXT",
        ] {
            let _ = conn.execute(stmt, []);
        }
        // Created after the migration above, not in the batch: `conversation_id`
        // does not exist yet on a store opened before that column was added.
        // Covers both per-conversation accesses — `defer_conversation`'s update
        // and `claim_conversation`, the latter of which now runs on *every*
        // outbound send that has a store attached.
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_outbound_conversation
                ON outbound_deliveries(channel_id, conversation_id, id)",
            [],
        );
        // `conversation_id` is a projection of the payload, so pre-existing rows
        // can be backfilled rather than left permanently outside the
        // per-conversation ordering guarantee. Bounded by `max_queue_len`, runs
        // once, and skips anything that no longer deserializes (those rows are
        // dropped by `claim_due` anyway).
        Self::backfill_conversation_ids(&conn, "outbound_deliveries");
        Self::backfill_conversation_ids(&conn, "dead_letters");
        Ok(Self {
            conn: Mutex::new(conn),
            config,
            drain_spawned: AtomicBool::new(false),
            drain_gate: tokio::sync::Mutex::new(()),
        })
    }

    fn backfill_conversation_ids(conn: &Connection, table: &str) {
        // SQL identifier allow-list: even though callers currently pass hard-
        // coded literals, this function's API shape lets a future caller
        // inject arbitrary SQL via `table` because identifiers cannot be
        // bound as parameters. Reject anything that isn't [A-Za-z0-9_] up
        // front so this stays safe under any future refactor.
        if !table
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
            || table.is_empty()
        {
            tracing::warn!(table = %table, "backfill_conversation_ids: rejecting non-identifier table name");
            return;
        }
        let sql = format!("SELECT id, payload FROM {table} WHERE conversation_id IS NULL");
        let Ok(mut stmt) = conn.prepare(&sql) else {
            return;
        };
        let Ok(mapped) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        else {
            return;
        };
        let rows: Vec<(i64, String)> = mapped.filter_map(std::result::Result::ok).collect();
        drop(stmt);
        for (id, payload) in rows {
            if let Ok(m) = serde_json::from_str::<OutboundMessage>(&payload) {
                let _ = conn.execute(
                    &format!("UPDATE {table} SET conversation_id = ?1 WHERE id = ?2"),
                    params![m.conversation_id.as_str(), id],
                );
            }
        }
    }

    /// Access the queue tuning this store was built with.
    pub const fn config(&self) -> &DeliveryQueueConfig {
        &self.config
    }

    /// Attempt to become the sole drainer for this store. Returns `true` for the
    /// first caller and `false` for every subsequent one — the runtime half of
    /// the single-drainer invariant [`claim_due`](Self::claim_due) relies on.
    /// [`spawn_drain`] calls this so a second drain task can never race the
    /// first into duplicate claims.
    fn try_claim_drainer(&self) -> bool {
        !self.drain_spawned.swap(true, Ordering::SeqCst)
    }

    /// Lock the connection, recovering from poisoning (P7: lock safety).
    fn guard(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Persist an outbound message for later retry. Enforces the bounded queue
    /// cap by evicting the oldest rows first.
    ///
    /// A payload over [`DeliveryQueueConfig::max_payload_bytes`] is **not**
    /// admitted: it goes straight to the dead-letter table as
    /// [`DeadLetterReason::PayloadTooLarge`] so the operator can still see what
    /// was dropped and why, and the caller gets
    /// [`EnqueueOutcome::TooLarge`].
    pub fn enqueue(
        &self,
        channel_id: &str,
        message: &OutboundMessage,
        last_error: &str,
        next_attempt_at: i64,
    ) -> rusqlite::Result<EnqueueOutcome> {
        let payload = serde_json::to_string(message)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let now = now_secs();
        let conn = self.guard();

        if payload.len() > self.config.max_payload_bytes {
            let bytes = payload.len();
            Self::insert_dead_letter(
                &conn,
                self.config.max_queue_len,
                DeadLetterInsert {
                    channel_id,
                    conversation_id: message.conversation_id.as_str(),
                    payload: &payload,
                    attempts: 0,
                    last_error: &format!(
                        "payload {bytes} B exceeds max_payload_bytes {} (never enqueued); last transport error: {last_error}",
                        self.config.max_payload_bytes
                    ),
                    reason: DeadLetterReason::PayloadTooLarge,
                    created_at: now,
                    died_at: now,
                },
            )?;
            return Ok(EnqueueOutcome::TooLarge { bytes });
        }

        let len: i64 =
            conn.query_row("SELECT COUNT(*) FROM outbound_deliveries", [], |r| r.get(0))?;
        if len >= self.config.max_queue_len {
            let overflow = len - self.config.max_queue_len + 1;
            conn.execute(
                "DELETE FROM outbound_deliveries WHERE id IN (
                    SELECT id FROM outbound_deliveries ORDER BY created_at ASC LIMIT ?1)",
                params![overflow],
            )?;
        }

        conn.execute(
            "INSERT INTO outbound_deliveries
                (channel_id, conversation_id, payload, attempts, next_attempt_at, created_at, last_error)
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6)",
            params![
                channel_id,
                message.conversation_id.as_str(),
                payload,
                next_attempt_at,
                now,
                last_error
            ],
        )?;
        Ok(EnqueueOutcome::Queued(conn.last_insert_rowid()))
    }

    /// Push every *later* queued message for the same conversation out to
    /// `not_before`, so it cannot overtake a head that just failed.
    ///
    /// Grouping inside one drain tick is not enough on its own: a failed head is
    /// rescheduled with backoff into the future while its followers keep their
    /// original `next_attempt_at`, so the very next tick claims a follower, does
    /// not see the head at all, and delivers it first — permanently reordering
    /// the conversation. Ordering by `id` is ordering by enqueue time, which is
    /// the order the caller produced them in.
    ///
    /// Only ever moves a row forward (`next_attempt_at < not_before` guard), so
    /// it can never make a deferred record due sooner than its own backoff.
    pub fn defer_conversation(
        &self,
        channel_id: &str,
        conversation_id: &str,
        after_id: i64,
        not_before: i64,
    ) -> rusqlite::Result<u64> {
        let n = self.guard().execute(
            "UPDATE outbound_deliveries SET next_attempt_at = ?4
             WHERE channel_id = ?1 AND conversation_id = ?2
               AND id > ?3 AND next_attempt_at < ?4",
            params![channel_id, conversation_id, after_id, not_before],
        )?;
        Ok(n as u64)
    }

    /// Test-only terse wrapper: enqueue and unwrap the row id, panicking if the
    /// payload was rejected. Keeps the many `enqueue → record_dead_letter`
    /// fixtures readable without weakening the production return type.
    #[cfg(test)]
    fn enqueue_id(
        &self,
        channel_id: &str,
        message: &OutboundMessage,
        last_error: &str,
        next_attempt_at: i64,
    ) -> rusqlite::Result<i64> {
        match self.enqueue(channel_id, message, last_error, next_attempt_at)? {
            EnqueueOutcome::Queued(id) => Ok(id),
            EnqueueOutcome::TooLarge { bytes } => {
                panic!("fixture payload unexpectedly rejected at {bytes} B")
            }
        }
    }

    /// Insert one dead letter, enforcing the table's own bound (oldest-died
    /// evicted first). Takes a plain `&Connection` so it serves both the
    /// transactional move in [`record_dead_letter`] and the direct insert in
    /// [`enqueue`] — a `Transaction` derefs to `Connection`, so there is one
    /// implementation of "what a dead letter row looks like", not two.
    fn insert_dead_letter(
        conn: &Connection,
        max_queue_len: i64,
        row: DeadLetterInsert<'_>,
    ) -> rusqlite::Result<()> {
        let len: i64 = conn.query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))?;
        if len >= max_queue_len {
            let overflow = len - max_queue_len + 1;
            conn.execute(
                "DELETE FROM dead_letters WHERE id IN (
                    SELECT id FROM dead_letters ORDER BY died_at ASC LIMIT ?1)",
                params![overflow],
            )?;
        }
        conn.execute(
            "INSERT INTO dead_letters
                (channel_id, conversation_id, payload, attempts, last_error, reason, created_at, died_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.channel_id,
                row.conversation_id,
                row.payload,
                i64::from(row.attempts),
                row.last_error,
                row.reason.as_str(),
                row.created_at,
                row.died_at
            ],
        )?;
        Ok(())
    }

    /// Claim up to `limit` records whose `next_attempt_at <= now`, oldest first.
    ///
    /// Rows whose payload no longer deserializes (schema drift, corruption) are
    /// dropped in place so a poison record can never wedge the queue forever.
    ///
    /// # Concurrency
    ///
    /// This claim is a plain SELECT — it does **not** lease or mark rows, so a
    /// row stays visible to any concurrent `claim_due` until [`drain_once`]
    /// settles it (delete / reschedule / dead-letter). Correctness therefore
    /// rests on a **single-drainer invariant**: exactly one [`drain_loop`] runs
    /// per store (one [`spawn_drain`] at boot, guarded against a second spawn by
    /// [`try_claim_drainer`](Self::try_claim_drainer)), and it awaits each
    /// `drain_once` to completion before the next tick. Two concurrent drainers
    /// would double-claim and double-deliver; if a second drainer is ever
    /// introduced, give this a real row lease (e.g. a `claimed_until` column
    /// updated in the same transaction as the SELECT) *before* doing so.
    pub fn claim_due(&self, now: i64, limit: usize) -> rusqlite::Result<Vec<DeliveryRecord>> {
        let conn = self.guard();

        // Collect raw rows fully, then release the statement before any DELETE
        // so there is no overlapping borrow of the connection.
        let raw: Vec<(i64, String, String, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT id, channel_id, payload, attempts
                 FROM outbound_deliveries
                 WHERE next_attempt_at <= ?1
                 ORDER BY next_attempt_at ASC
                 LIMIT ?2",
            )?;
            let mapped = stmt.query_map(params![now, limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };

        Ok(Self::hydrate(&conn, raw))
    }

    /// Claim this conversation's queued records in enqueue order, **ignoring
    /// `next_attempt_at`** — the backlog a live send must not overtake.
    ///
    /// Deliberately blind to the backoff schedule: the head of a wedged
    /// conversation is precisely the record sitting in the future, and it is the
    /// one a live send would jump ahead of. Pulling it forward is no more
    /// aggressive than the live send that is about to hit the same transport
    /// anyway. What the caller must *not* do is settle a transient failure here
    /// — see [`AttemptMode::Inline`].
    ///
    /// Ordered by `id` (enqueue order), which is the order the caller produced
    /// them in. Same poison-row handling as [`claim_due`](Self::claim_due).
    pub fn claim_conversation(
        &self,
        channel_id: &str,
        conversation_id: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<DeliveryRecord>> {
        let conn = self.guard();
        let raw: Vec<(i64, String, String, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT id, channel_id, payload, attempts
                 FROM outbound_deliveries
                 WHERE channel_id = ?1 AND conversation_id = ?2
                 ORDER BY id ASC
                 LIMIT ?3",
            )?;
            let mapped =
                stmt.query_map(params![channel_id, conversation_id, limit as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(Self::hydrate(&conn, raw))
    }

    /// Turn claimed raw rows into records, dropping any whose payload no longer
    /// deserializes so a poison row can never wedge the queue. Shared by both
    /// claim paths so "what a claimed record is" has one definition.
    fn hydrate(conn: &Connection, raw: Vec<(i64, String, String, i64)>) -> Vec<DeliveryRecord> {
        let mut out = Vec::with_capacity(raw.len());
        for (id, channel_id, payload, attempts) in raw {
            match serde_json::from_str::<OutboundMessage>(&payload) {
                Ok(message) => out.push(DeliveryRecord {
                    id,
                    channel_id,
                    message,
                    attempts: attempts.max(0) as u32,
                }),
                Err(e) => {
                    warn!(id, error = %e, "dropping undeserializable delivery record");
                    let _ =
                        conn.execute("DELETE FROM outbound_deliveries WHERE id = ?1", params![id]);
                }
            }
        }
        out
    }

    /// Delete a record after a successful delivery.
    pub fn mark_delivered(&self, id: i64) -> rusqlite::Result<()> {
        self.guard()
            .execute("DELETE FROM outbound_deliveries WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Bump attempt count and push the next attempt into the future.
    ///
    /// Also clears the in-flight stamp: the attempt ended with a *reported*
    /// transient error, so its outcome is known (never delivered) and it must
    /// not be mistaken for an interrupted one by
    /// [`reconcile_inflight`](Self::reconcile_inflight).
    pub fn reschedule(
        &self,
        id: i64,
        attempts: u32,
        next_attempt_at: i64,
        last_error: &str,
    ) -> rusqlite::Result<()> {
        self.guard().execute(
            "UPDATE outbound_deliveries
             SET attempts = ?1, next_attempt_at = ?2, last_error = ?3, inflight_since = NULL
             WHERE id = ?4",
            params![i64::from(attempts), next_attempt_at, last_error, id],
        )?;
        Ok(())
    }

    /// Permanently drop a record (exhausted budget or non-retryable error).
    pub fn drop_record(&self, id: i64, reason: &str) -> rusqlite::Result<()> {
        debug!(id, reason, "dropping outbound delivery record");
        self.guard()
            .execute("DELETE FROM outbound_deliveries WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Total stored records (diagnostics / tests).
    pub fn len(&self) -> rusqlite::Result<i64> {
        self.guard()
            .query_row("SELECT COUNT(*) FROM outbound_deliveries", [], |r| r.get(0))
    }

    /// `true` when the store holds no pending deliveries.
    pub fn is_empty(&self) -> bool {
        self.len().map_or(true, |n| n == 0)
    }

    /// Stamp a record as *attempt in progress*, immediately before the send
    /// crosses the transport boundary.
    ///
    /// This is the write half of the crash-window fix (see the module docs):
    /// the stamp is what lets [`reconcile_inflight`](Self::reconcile_inflight)
    /// tell "never attempted, safe to replay" from "attempted, outcome unknown"
    /// after an abrupt exit. Best-effort by design — a failure here is logged by
    /// the caller and the send proceeds, because refusing to deliver is a worse
    /// outcome than risking one duplicate.
    pub fn mark_inflight(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        self.guard().execute(
            "UPDATE outbound_deliveries SET inflight_since = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// Clear an in-flight stamp without touching the record's retry schedule.
    ///
    /// The counterpart to [`mark_inflight`](Self::mark_inflight) for an attempt
    /// that ended in a *reported* transient failure yet must leave the record
    /// exactly as it was — the opportunistic inline flush
    /// ([`AttemptMode::Inline`]). [`reschedule`](Self::reschedule) also clears
    /// the stamp but owns `attempts` and `next_attempt_at` with it, which is
    /// precisely what an opportunistic probe must not spend.
    pub fn clear_inflight(&self, id: i64) -> rusqlite::Result<()> {
        self.guard().execute(
            "UPDATE outbound_deliveries SET inflight_since = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Retire every record still stamped in-flight into the dead-letter table
    /// as [`DeadLetterReason::UnknownOutcome`], returning how many were moved.
    ///
    /// Called exactly once per process by [`spawn_drain`], **before** the drain
    /// loop starts. Any stamp that survived into a fresh process belongs to an
    /// attempt whose outcome nobody recorded — the daemon exited mid-send. That
    /// is the same ambiguity [`should_enqueue`] already refuses to retry for
    /// [`SendFailed`](ChannelError::SendFailed), so it gets the same verdict:
    /// preserved for inspection, never silently replayed. An operator who knows
    /// the message did not land can still redrive it explicitly.
    pub fn reconcile_inflight(&self, now: i64) -> rusqlite::Result<u64> {
        let mut conn = self.guard();
        let tx = conn.transaction()?;
        let rows: Vec<(
            i64,
            String,
            Option<String>,
            String,
            u32,
            Option<String>,
            i64,
            i64,
        )> = {
            let mut stmt = tx.prepare(
                "SELECT id, channel_id, conversation_id, payload, attempts, last_error,
                        created_at, inflight_since
                 FROM outbound_deliveries WHERE inflight_since IS NOT NULL",
            )?;
            let mapped = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?.max(0) as u32,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut moved = 0u64;
        for (id, channel_id, conversation_id, payload, attempts, last_error, created_at, since) in
            &rows
        {
            Self::insert_dead_letter(
                &tx,
                self.config.max_queue_len,
                DeadLetterInsert {
                    channel_id,
                    conversation_id: conversation_id.as_deref().unwrap_or_default(),
                    payload,
                    // The interrupted attempt counts: it reached the transport.
                    attempts: attempts.saturating_add(1),
                    last_error: &format!(
                        "daemon exited mid-send (attempt started at {since}); outcome unknown, not replayed automatically. Prior error: {}",
                        last_error.as_deref().unwrap_or("none")
                    ),
                    reason: DeadLetterReason::UnknownOutcome,
                    created_at: *created_at,
                    died_at: now,
                },
            )?;
            tx.execute("DELETE FROM outbound_deliveries WHERE id = ?1", params![id])?;
            moved += 1;
        }
        tx.commit()?;
        Ok(moved)
    }

    /// Move a record that will not be retried again into the `dead_letters`
    /// table instead of hard-deleting it, so an undelivered proactive push
    /// leaves a forensic trail (channel, attempts, last error, reason, age).
    /// The move is transactional — the row is never in both tables nor lost
    /// between them — and the dead-letter table is bounded by the same
    /// `max_queue_len` (CWE-400), evicting the oldest dead letters first. A
    /// missing source row (already drained) is a no-op.
    ///
    /// `reason` decides whether a later redrive will touch this record; see
    /// [`DeadLetterReason`].
    pub fn record_dead_letter(
        &self,
        id: i64,
        attempts: u32,
        last_error: &str,
        reason: DeadLetterReason,
    ) -> rusqlite::Result<()> {
        let died_at = now_secs();
        let mut conn = self.guard();
        let tx = conn.transaction()?;

        // Snapshot the source row (preserving the original created_at so the
        // dead letter reports true age, not move time) before deleting it.
        let row: Option<(String, Option<String>, String, i64)> = tx
            .query_row(
                "SELECT channel_id, conversation_id, payload, created_at
                 FROM outbound_deliveries WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;

        if let Some((channel_id, conversation_id, payload, created_at)) = row {
            Self::insert_dead_letter(
                &tx,
                self.config.max_queue_len,
                DeadLetterInsert {
                    channel_id: &channel_id,
                    conversation_id: conversation_id.as_deref().unwrap_or_default(),
                    payload: &payload,
                    attempts,
                    last_error,
                    reason,
                    created_at,
                    died_at,
                },
            )?;
            tx.execute("DELETE FROM outbound_deliveries WHERE id = ?1", params![id])?;
        }
        tx.commit()
    }

    /// One-pass observability snapshot of the live queue (and the dead-letter
    /// count). Reads only columns the queue already maintains — no extra state.
    /// `now` is injected so the oldest-age computation is testable.
    pub fn stats(&self, now: i64) -> rusqlite::Result<DeliveryQueueStats> {
        let conn = self.guard();
        let pending: i64 =
            conn.query_row("SELECT COUNT(*) FROM outbound_deliveries", [], |r| r.get(0))?;
        let due_now: i64 = conn.query_row(
            "SELECT COUNT(*) FROM outbound_deliveries WHERE next_attempt_at <= ?1",
            params![now],
            |r| r.get(0),
        )?;
        // MIN over an empty table yields a single NULL row → Option::None.
        let oldest_created: Option<i64> =
            conn.query_row("SELECT MIN(created_at) FROM outbound_deliveries", [], |r| {
                r.get(0)
            })?;
        let oldest_age_secs = oldest_created.map(|c| (now - c).max(0));

        let per_channel: Vec<(String, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT channel_id, COUNT(*) AS n FROM outbound_deliveries
                 GROUP BY channel_id ORDER BY n DESC, channel_id ASC",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let dead_lettered: i64 =
            conn.query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))?;
        // Counted with the same predicate redrive uses, expressed once in Rust
        // (`DeadLetterReason::replay_safe`) and projected into SQL here, so the
        // reported figure cannot drift from what a redrive would actually move.
        let replay_safe_tokens = DeadLetterReason::replay_safe_tokens();
        let dead_lettered_replayable: i64 = if replay_safe_tokens.is_empty() {
            0
        } else {
            let placeholders = vec!["?"; replay_safe_tokens.len()].join(",");
            conn.query_row(
                &format!("SELECT COUNT(*) FROM dead_letters WHERE reason IN ({placeholders})"),
                rusqlite::params_from_iter(replay_safe_tokens.iter()),
                |r| r.get(0),
            )?
        };

        Ok(DeliveryQueueStats {
            pending,
            due_now,
            oldest_age_secs,
            per_channel,
            dead_lettered,
            dead_lettered_replayable,
        })
    }

    /// Most-recently dead-lettered deliveries, newest first, for troubleshooting
    /// (parity-plus over openclaw, which retains failed rows but exposes no
    /// inspection path). Rows whose payload no longer deserializes are skipped.
    pub fn recent_dead_letters(&self, limit: usize) -> rusqlite::Result<Vec<DeadLetter>> {
        let conn = self.guard();
        let raw: Vec<(
            String,
            String,
            i64,
            Option<String>,
            Option<String>,
            i64,
            i64,
        )> = {
            let mut stmt = conn.prepare(
                "SELECT channel_id, payload, attempts, last_error, reason, created_at, died_at
                 FROM dead_letters ORDER BY died_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::with_capacity(raw.len());
        for (channel_id, payload, attempts, last_error, reason, created_at, died_at) in raw {
            if let Ok(message) = serde_json::from_str::<OutboundMessage>(&payload) {
                out.push(DeadLetter {
                    channel_id,
                    message,
                    attempts: attempts.max(0) as u32,
                    last_error: last_error.unwrap_or_default(),
                    // Rows written before the column existed carry the DEFAULT
                    // ('exhausted'), which is exactly what they were.
                    reason: reason.as_deref().map_or(
                        DeadLetterReason::Exhausted,
                        DeadLetterReason::from_str_lossy,
                    ),
                    created_at,
                    died_at,
                });
            }
        }
        Ok(out)
    }

    /// Move dead-lettered deliveries back into the live queue for another
    /// delivery pass — the recovery half of the forensic trail (`record_dead_letter`
    /// preserves *what* was lost; this replays it once the transport is healthy
    /// again, R5). Optionally restricted to a single `channel`; `None` redrives
    /// every dead letter. Returns the number of records moved.
    ///
    /// **Safety is per record, not per table.** Only reasons whose
    /// [`replay_safe`](DeadLetterReason::replay_safe) is true are moved —
    /// [`Exhausted`](DeadLetterReason::Exhausted) (a duplicate-safe transient
    /// error that ran out of attempts) and
    /// [`Permanent`](DeadLetterReason::Permanent). Records whose outcome is
    /// *unknown* ([`Ambiguous`](DeadLetterReason::Ambiguous),
    /// [`UnknownOutcome`](DeadLetterReason::UnknownOutcome)) are left in place
    /// and reported as `skipped_unsafe`: replaying them could double-send. This
    /// replaces the old blanket "every dead letter is duplicate-safe by
    /// construction" claim, which stopped being true the moment terminal
    /// failures and interrupted attempts also started landing in this table.
    ///
    /// Each moved record is reset to a fresh budget (`attempts = 0`) and made
    /// immediately due (`next_attempt_at = created_at = now`), preserving its
    /// prior `last_error` as context. The whole batch moves in one transaction.
    ///
    /// **The live queue's bound is respected by moving fewer records, never by
    /// evicting live ones.** The previous implementation moved everything and
    /// then trimmed `outbound_deliveries` by oldest `created_at` — and since
    /// redriven rows carry `created_at = now`, the rows evicted were the
    /// genuinely-older *pending* deliveries: a recovery action that destroyed
    /// in-flight work to make room for already-failed work. Anything that does
    /// not fit is left in `dead_letters` and reported as `skipped_capacity`,
    /// so a second redrive after the queue drains picks it up.
    pub fn redrive_dead_letters(
        &self,
        now: i64,
        channel: Option<&str>,
    ) -> rusqlite::Result<RedriveOutcome> {
        type RedriveRow = (i64, String, Option<String>, String, Option<String>);
        fn map_row(r: &rusqlite::Row) -> rusqlite::Result<RedriveRow> {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        }

        let safe = DeadLetterReason::replay_safe_tokens();
        let mut conn = self.guard();
        let tx = conn.transaction()?;

        // How many rows the live queue can still take. Computed before the move
        // so the batch is bounded by real capacity rather than trimmed after.
        let live_len: i64 =
            tx.query_row("SELECT COUNT(*) FROM outbound_deliveries", [], |r| r.get(0))?;
        let capacity = (self.config.max_queue_len - live_len).max(0);

        // Snapshot the dead letters to move (optionally filtered by channel)
        // before mutating either table, so the statement is released first.
        let placeholders = vec!["?"; safe.len()].join(",");
        let (rows, unsafe_total): (Vec<RedriveRow>, i64) = if safe.is_empty() {
            (Vec::new(), 0)
        } else {
            match channel {
                Some(ch) => {
                    let mut stmt = tx.prepare(&format!(
                        "SELECT id, channel_id, conversation_id, payload, last_error
                         FROM dead_letters
                         WHERE channel_id = ?1 AND reason IN ({placeholders})
                         ORDER BY died_at ASC"
                    ))?;
                    let args: Vec<&dyn rusqlite::ToSql> =
                        std::iter::once(&ch as &dyn rusqlite::ToSql)
                            .chain(safe.iter().map(|t| t as &dyn rusqlite::ToSql))
                            .collect();
                    let mapped = stmt.query_map(args.as_slice(), map_row)?;
                    let rows = mapped.collect::<rusqlite::Result<Vec<_>>>()?;
                    let unsafe_total: i64 = tx.query_row(
                        &format!(
                            "SELECT COUNT(*) FROM dead_letters
                             WHERE channel_id = ?1 AND reason NOT IN ({placeholders})"
                        ),
                        args.as_slice(),
                        |r| r.get(0),
                    )?;
                    (rows, unsafe_total)
                }
                None => {
                    let mut stmt = tx.prepare(&format!(
                        "SELECT id, channel_id, conversation_id, payload, last_error
                         FROM dead_letters
                         WHERE reason IN ({placeholders}) ORDER BY died_at ASC"
                    ))?;
                    let mapped =
                        stmt.query_map(rusqlite::params_from_iter(safe.iter()), map_row)?;
                    let rows = mapped.collect::<rusqlite::Result<Vec<_>>>()?;
                    let unsafe_total: i64 = tx.query_row(
                        &format!(
                            "SELECT COUNT(*) FROM dead_letters WHERE reason NOT IN ({placeholders})"
                        ),
                        rusqlite::params_from_iter(safe.iter()),
                        |r| r.get(0),
                    )?;
                    (rows, unsafe_total)
                }
            }
        };

        let admitted = rows.len().min(capacity.max(0) as usize);
        let skipped_capacity = (rows.len() - admitted) as u64;
        let mut moved = 0u64;
        for (id, channel_id, conversation_id, payload, last_error) in rows.iter().take(admitted) {
            tx.execute(
                "INSERT INTO outbound_deliveries
                    (channel_id, conversation_id, payload, attempts, next_attempt_at,
                     created_at, last_error)
                 VALUES (?1, ?2, ?3, 0, ?4, ?4, ?5)",
                params![channel_id, conversation_id, payload, now, last_error],
            )?;
            tx.execute("DELETE FROM dead_letters WHERE id = ?1", params![id])?;
            moved += 1;
        }

        tx.commit()?;
        Ok(RedriveOutcome {
            moved,
            skipped_capacity,
            skipped_unsafe: unsafe_total.max(0) as u64,
        })
    }
}

/// Drive one pass over all currently-due records: claim, attempt delivery via
/// the enqueue-free [`send_attempt`](ChannelRegistry::send_attempt), then settle
/// each record (delivered → delete, transient → reschedule with backoff,
/// exhausted → dead-letter, ambiguous/permanent → drop). Factored out of
/// [`drain_loop`] to keep the loop body readable.
async fn drain_once(registry: &ChannelRegistry, store: &DeliveryStore) {
    // Held for the whole pass: claim → deliver → settle must not interleave
    // with the inline flush, which claims from the same unleased table.
    let _gate = store.drain_gate.lock().await;
    let cfg = store.config();
    let now = now_secs();
    let due = match store.claim_due(now, cfg.batch) {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "delivery queue: claim_due failed");
            return;
        }
    };

    // Group the batch by destination conversation, preserving claim order
    // (`claim_due` returns oldest-due first) both between and within groups.
    //
    // Without this the tick treats every record independently: two replies
    // queued for the same chat are attempted back to back, and if the first
    // fails while the second succeeds, the user reads them in the wrong order —
    // and keeps reading them in the wrong order, since the failed one is
    // rescheduled *behind* everything that came after it. Draining a
    // conversation strictly in order, and stopping that conversation at its
    // first non-success, makes queued delivery order-preserving per
    // conversation. Different conversations stay independent, so one wedged
    // chat cannot stall the rest of the batch.
    let mut order: Vec<(String, String)> = Vec::new();
    let mut groups: std::collections::HashMap<(String, String), Vec<DeliveryRecord>> =
        std::collections::HashMap::new();
    for rec in due {
        let key = (
            rec.channel_id.clone(),
            rec.message.conversation_id.as_str().to_string(),
        );
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(rec);
    }

    for key in order {
        let Some(records) = groups.remove(&key) else {
            continue;
        };
        for rec in records {
            if !deliver_one(registry, store, cfg, now, &rec, AttemptMode::Drain).await {
                // Head-of-line: everything still queued for this conversation
                // stays queued and keeps its relative order.
                break;
            }
        }
    }
}

/// Who is attempting a queued record, and therefore what a *failure* is allowed
/// to change about it.
///
/// The success path is identical for both. The difference is ownership of the
/// retry schedule: the drain loop owns `attempts` / `next_attempt_at` and the
/// `max_attempts` budget behind them; the inline flush is an opportunistic
/// probe riding along with a live send and must leave that budget alone. If it
/// did not, a chatty conversation talking to a wedged channel would burn its
/// own backlog's ten attempts in ten consecutive user messages and dead-letter
/// it far sooner than the configured backoff curve intends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptMode {
    /// Background drain tick — settles every outcome.
    Drain,
    /// Inline flush ahead of a live send — settles success and *terminal*
    /// failures only (see [`deliver_one`]).
    Inline,
}

/// Attempt one queued record and settle it. Returns `true` when the
/// conversation may proceed to its next queued record (i.e. this one was
/// delivered), `false` when the conversation must stop for this tick.
async fn deliver_one(
    registry: &ChannelRegistry,
    store: &DeliveryStore,
    cfg: &DeliveryQueueConfig,
    now: i64,
    rec: &DeliveryRecord,
    mode: AttemptMode,
) -> bool {
    let channel = ChannelId(rec.channel_id.clone());

    // Stamp *before* crossing the transport boundary. If the daemon dies
    // between here and the settle below, the stamp is the only evidence that
    // this record's outcome is unknown rather than "never tried" — see
    // `DeliveryStore::reconcile_inflight`. Best-effort: a stamp failure is not
    // a reason to withhold delivery.
    if let Err(e) = store.mark_inflight(rec.id, now) {
        warn!(id = rec.id, error = %e, "delivery queue: could not stamp attempt as in-flight");
    }

    // Call the enqueue-free send path: the public `send` would re-persist the
    // record on transient failure, doubling rows on every drain tick.
    match registry.send_attempt(&channel, &rec.message).await {
        Ok(_) => {
            if let Err(e) = store.mark_delivered(rec.id) {
                warn!(id = rec.id, error = %e, "delivery queue: mark_delivered failed");
            } else {
                info!(
                    id = rec.id,
                    channel = %rec.channel_id,
                    attempts = rec.attempts + 1,
                    "delivery queue: delivered queued outbound message"
                );
            }
            true
        }
        // Transient, and the record's schedule is not this attempt's to spend:
        // put it back exactly as it was found. The live send this flush runs
        // ahead of is about to hit the same failure and be enqueued *behind*
        // this record, so the conversation's order survives either way.
        Err(e) if should_enqueue(&e) && mode == AttemptMode::Inline => {
            if let Err(clear_err) = store.clear_inflight(rec.id) {
                warn!(id = rec.id, error = %clear_err, "delivery queue: could not clear the inline probe's in-flight stamp");
            }
            debug!(
                id = rec.id,
                channel = %rec.channel_id,
                error = ?e,
                "delivery queue: inline flush stopped at a still-failing record; schedule untouched"
            );
            false
        }
        Err(e) if should_enqueue(&e) => {
            let attempts = rec.attempts + 1;
            if attempts >= cfg.max_attempts {
                retire(
                    store,
                    rec,
                    attempts,
                    &format!("{e:?}"),
                    DeadLetterReason::Exhausted,
                );
            } else {
                // Floor at 1s: a sub-second backoff truncates to 0 through
                // `as_secs()`, which would reschedule the record as
                // immediately-due and hot-retry a still-down channel.
                let next = now + backoff_delay(cfg, attempts).as_secs().max(1) as i64;
                let _ = store.reschedule(rec.id, attempts, next, &format!("{e:?}"));
                // Carry the head's backoff to the rest of its conversation.
                // Breaking out of this tick is not enough on its own: the head
                // just moved into the future while its followers stayed due, so
                // the next tick would claim a follower, never see the head, and
                // deliver it first — reordering the conversation permanently.
                if let Err(defer_err) = store.defer_conversation(
                    &rec.channel_id,
                    rec.message.conversation_id.as_str(),
                    rec.id,
                    next,
                ) {
                    warn!(
                        id = rec.id,
                        error = %defer_err,
                        "delivery queue: could not defer the rest of this conversation; \
                         later messages may overtake the one that failed"
                    );
                }
            }
            false
        }
        // Ambiguous (may already be on the wire) or permanent — never retried.
        // Previously hard-deleted, which left the operator with *less* evidence
        // for a terminal failure than for a transient one: the dead-letter trail
        // existed precisely so an undelivered proactive push is never silent
        // (R5), and this was the one path that stayed silent.
        //
        // Settled in **both** modes, unlike the transient arm above: leaving an
        // ambiguous failure for the drain loop to retry is how at-most-once
        // dies. An inline probe that may already have put the message on the
        // wire must retire it here, not hand it back for a second attempt.
        Err(e) => {
            retire(
                store,
                rec,
                rec.attempts + 1,
                &format!("non-retryable: {e:?}"),
                terminal_reason(&e),
            );
            false
        }
    }
}

/// Deliver this conversation's queued backlog, in enqueue order, immediately
/// before a live send to the same conversation goes out.
///
/// # Why this exists
///
/// Per-conversation ordering used to hold only *within* the queue. A message
/// that never failed took the live path and went out at once, while messages
/// that had failed sat in the queue serving out a backoff of up to
/// `max_backoff` — so a reply the user was waiting for could land minutes after
/// the replies that came later. Fixing that by admitting live sends into the
/// queue would have changed what every caller of
/// [`send`](ChannelRegistry::send) observes (a queued message has no
/// [`SendResult`](super::channel::SendResult) to return) and would have turned
/// a reordering bug into a head-of-line availability bug: one wedged record
/// would hold up every later message for its conversation until the record
/// dead-lettered. Flushing the backlog *ahead* of the live send instead keeps
/// the return contract byte-for-byte and stops at the first failure, so a
/// wedged conversation costs one extra attempt, not a stall.
///
/// # What it will not do
///
/// - It never runs while the background drainer holds the gate
///   ([`try_lock`](tokio::sync::Mutex::try_lock)): the drainer is already
///   delivering this conversation in order, and blocking a user-facing send
///   behind a whole drain tick is worse than the one-tick reorder window that
///   skipping leaves.
/// - It never spends the backlog's retry budget — see [`AttemptMode::Inline`].
/// - It stops at [`DeliveryQueueConfig::inline_flush_max`] records and says so,
///   rather than letting one send block on an unbounded backlog.
pub(super) async fn flush_conversation(
    registry: &ChannelRegistry,
    store: &DeliveryStore,
    channel_id: &str,
    conversation_id: &str,
) {
    let cfg = store.config();
    let limit = cfg.inline_flush_max;
    if limit == 0 {
        return;
    }
    let Ok(_gate) = store.drain_gate.try_lock() else {
        debug!(
            channel = %channel_id,
            "delivery queue: drain in progress; skipping the inline flush for this send"
        );
        return;
    };

    // One over the cap: the extra record is never attempted, it only answers
    // "is there more behind what we are about to flush?" without a second query.
    let mut records = match store.claim_conversation(channel_id, conversation_id, limit + 1) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, channel = %channel_id, "delivery queue: claim_conversation failed");
            return;
        }
    };
    if records.is_empty() {
        return;
    }
    let overflowed = records.len() > limit;
    records.truncate(limit);

    let now = now_secs();
    let mut delivered = 0usize;
    for rec in &records {
        if !deliver_one(registry, store, cfg, now, rec, AttemptMode::Inline).await {
            return;
        }
        delivered += 1;
    }
    if overflowed {
        warn!(
            channel = %channel_id,
            flushed = delivered,
            limit,
            "delivery queue: backlog deeper than inline_flush_max; this send may overtake what is still queued"
        );
    } else {
        debug!(
            channel = %channel_id,
            flushed = delivered,
            "delivery queue: flushed this conversation's backlog ahead of a live send"
        );
    }
}

/// Take custody of producer-owned local attachments before a message is
/// persisted for retry, returning a rewritten copy — or `None` when the message
/// is already durable as-is (the common case, which then costs no deep copy).
///
/// # Why
///
/// A durable row must not outlive its media. Every local
/// [`Attachment::path`](super::channel::Attachment::path) Aleph produces points
/// **inside the OS temp directory** — `media::cache` refuses to attach a local
/// path from anywhere else (arbitrary-file exfiltration guard), and TTS writes
/// its output there too. So a queued row referencing a path is referencing a
/// file the OS is entitled to delete, and "survives a daemon restart" is
/// exactly the promise this queue exists to make. The byte cap does not cover
/// this: a path attachment serializes to a couple of hundred bytes and sails
/// through, then replays as a message whose media is simply gone.
///
/// Custody is taken by inlining the bytes into
/// [`Attachment::data`](super::channel::Attachment::data) — the branch every
/// adapter already prefers over `path` — rather than by copying into a separate
/// spool directory. The row stays self-contained, so eviction, dead-lettering,
/// redrive and the byte cap keep working unchanged and there is no second
/// lifecycle to garbage-collect.
///
/// # Never worse than before
///
/// Both refusals leave the attachment exactly as the caller wrote it, so the
/// queued row is no less deliverable than it is today:
///
/// - the file cannot be read (already gone, permissions) — nothing to inline;
/// - inlining would push the payload over `max_payload_bytes` — a large video
///   is better queued as a path that *might* still be there than dead-lettered
///   on the spot.
///
/// Both are logged, because "this row's media will not survive a restart" is
/// exactly the kind of thing that is otherwise only discovered by a user who
/// received a caption with no picture.
pub async fn take_media_custody(
    message: &OutboundMessage,
    max_payload_bytes: usize,
) -> Option<OutboundMessage> {
    let needs_custody = message
        .attachments
        .iter()
        .any(|a| a.data.is_none() && a.path.is_some());
    if !needs_custody {
        return None;
    }

    let mut rewritten = message.clone();
    let mut inlined_any = false;
    for att in &mut rewritten.attachments {
        let (None, Some(path)) = (att.data.as_ref(), att.path.as_ref()) else {
            continue;
        };
        match tokio::fs::read(path).await {
            Ok(bytes) => {
                // `InputFile::memory` carries no name of its own, so the name the
                // path used to supply has to move with the bytes or the upload
                // silently becomes "attachment".
                if att.filename.is_none() {
                    att.filename = std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned());
                }
                att.size = Some(bytes.len() as u64);
                att.data = Some(bytes);
                att.path = None;
                inlined_any = true;
            }
            Err(e) => warn!(
                path = %path,
                error = %e,
                "delivery queue: could not read a local attachment while queueing it; \
                 the queued row still references the original path and its media may \
                 not survive a restart"
            ),
        }
    }
    if !inlined_any {
        return None;
    }

    // Measured on the same serialization the store will cap, so the two can
    // never disagree about what fits.
    let fits = serde_json::to_string(&rewritten).is_ok_and(|s| s.len() <= max_payload_bytes);
    if !fits {
        warn!(
            max_payload_bytes,
            "delivery queue: attachment bytes exceed the payload cap; queueing the \
             original local path instead of dead-lettering, but its media may not \
             survive a restart"
        );
        return None;
    }
    Some(rewritten)
}

/// Move a record out of the live queue into the dead-letter trail, falling back
/// to a plain drop if the move itself fails so a poison record can never wedge
/// the queue — and reporting the outcome accurately (a fallen-back drop is *not*
/// a dead letter anyone can inspect later).
fn retire(
    store: &DeliveryStore,
    rec: &DeliveryRecord,
    attempts: u32,
    last_error: &str,
    reason: DeadLetterReason,
) {
    match store.record_dead_letter(rec.id, attempts, last_error, reason) {
        Ok(()) => warn!(
            id = rec.id,
            channel = %rec.channel_id,
            attempts,
            reason = reason.as_str(),
            replay_safe = reason.replay_safe(),
            "delivery queue: retired undelivered outbound message (dead-lettered)"
        ),
        Err(dl_err) => {
            warn!(id = rec.id, error = %dl_err, "delivery queue: dead-letter failed; dropping");
            let _ = store.drop_record(rec.id, "dead-letter failed");
        }
    }
}

/// Background drain loop: wakes every `tick` and replays all due records.
pub async fn drain_loop(registry: Arc<ChannelRegistry>, store: Arc<DeliveryStore>) {
    let tick = store.config().tick;
    let mut ticker = tokio::time::interval(tick);
    info!(
        tick_secs = tick.as_secs(),
        "delivery queue drain task started"
    );
    loop {
        ticker.tick().await;
        drain_once(&registry, &store).await;
    }
}

/// Spawn [`drain_loop`] on the current Tokio runtime.
///
/// Enforces the single-drainer invariant [`DeliveryStore::claim_due`] relies on:
/// the first call for a given store wins the drainer slot; any later call is a
/// loud no-op rather than a second drainer racing the first into duplicate
/// claims (`claim_due` is a plain SELECT with no row lease).
/// Also performs the once-per-process in-flight reconciliation
/// ([`DeliveryStore::reconcile_inflight`]) **before** the loop starts, so a
/// record whose send was interrupted by an abrupt exit is retired for
/// inspection instead of being replayed into a duplicate. Winning the drainer
/// slot is the right place for it: it is exactly the "this process now owns
/// this store" moment, and it can never race the loop it precedes.
pub fn spawn_drain(registry: Arc<ChannelRegistry>, store: Arc<DeliveryStore>) {
    if !store.try_claim_drainer() {
        warn!(
            "delivery queue: a drain task is already running for this store; \
             refusing to spawn a second drainer (claim_due is not lease-atomic)"
        );
        return;
    }
    match store.reconcile_inflight(now_secs()) {
        Ok(0) => {}
        Ok(n) => warn!(
            interrupted = n,
            "delivery queue: found sends interrupted by a previous exit; \
             retired as unknown-outcome rather than replayed (inspect via the \
             channel_outbox tool; redrive them explicitly if they never landed)"
        ),
        Err(e) => warn!(error = %e, "delivery queue: in-flight reconciliation failed"),
    }
    tokio::spawn(drain_loop(registry, store));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::OutboundMessage;

    fn cfg() -> DeliveryQueueConfig {
        DeliveryQueueConfig::default()
    }

    fn store() -> DeliveryStore {
        DeliveryStore::open_in_memory(cfg()).expect("open in-memory store")
    }

    fn msg(text: &str) -> OutboundMessage {
        OutboundMessage::text("conv-1", text)
    }

    /// The single-drainer invariant `claim_due` relies on: only the first caller
    /// wins the drainer slot, so `spawn_drain` can never start a second drainer
    /// that would double-claim the same rows.
    #[test]
    fn only_one_drainer_can_claim_the_store() {
        let s = store();
        assert!(s.try_claim_drainer(), "first caller wins the drainer slot");
        assert!(!s.try_claim_drainer(), "second caller is refused");
        assert!(!s.try_claim_drainer(), "and stays refused");
    }

    #[test]
    fn enqueue_then_claim_roundtrips_the_message() {
        let s = store();
        let now = now_secs();
        let id = s
            .enqueue_id("telegram", &msg("hello"), "NotConnected", now)
            .unwrap();
        assert!(id > 0);

        let due = s.claim_due(now, 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, id);
        assert_eq!(due[0].channel_id, "telegram");
        assert_eq!(due[0].message.text, "hello");
        assert_eq!(due[0].attempts, 0);
    }

    #[test]
    fn future_records_are_not_claimed_yet() {
        let s = store();
        let now = now_secs();
        s.enqueue_id("signal", &msg("later"), "NotConnected", now + 3600)
            .unwrap();
        assert!(s.claim_due(now, 10).unwrap().is_empty());
        // ...but become due once the clock passes their schedule.
        assert_eq!(s.claim_due(now + 3600, 10).unwrap().len(), 1);
    }

    #[test]
    fn mark_delivered_removes_the_record() {
        let s = store();
        let now = now_secs();
        let id = s
            .enqueue_id("telegram", &msg("bye"), "NotConnected", now)
            .unwrap();
        s.mark_delivered(id).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn reschedule_bumps_attempts_and_defers() {
        let s = store();
        let now = now_secs();
        let id = s
            .enqueue_id("telegram", &msg("retry"), "NotConnected", now)
            .unwrap();
        s.reschedule(id, 1, now + 100, "still down").unwrap();

        assert!(
            s.claim_due(now, 10).unwrap().is_empty(),
            "deferred record not yet due"
        );
        let due = s.claim_due(now + 100, 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].attempts, 1);
    }

    #[test]
    fn should_enqueue_classifies_duplicate_safe_errors_only() {
        assert!(should_enqueue(&ChannelError::NotConnected("down".into())));
        assert!(should_enqueue(&ChannelError::RateLimited {
            retry_after_secs: 5
        }));
        // Ambiguous / permanent — never retried (at-most-once for SendFailed).
        assert!(!should_enqueue(&ChannelError::SendFailed(
            "ambiguous".into()
        )));
        assert!(!should_enqueue(&ChannelError::Internal("boom".into())));
        assert!(!should_enqueue(&ChannelError::ConfigError("bad".into())));
        assert!(!should_enqueue(&ChannelError::MessageTooLarge {
            size: 10,
            max_size: 5
        }));
    }

    #[test]
    fn backoff_is_monotonic_and_capped() {
        let c = cfg();
        let d0 = backoff_delay(&c, 0);
        let d1 = backoff_delay(&c, 1);
        let d2 = backoff_delay(&c, 2);
        assert!(d0 <= d1 && d1 <= d2, "backoff must not decrease");
        assert_eq!(backoff_delay(&c, 100), c.max_backoff, "saturates at max");
    }

    #[test]
    fn bounded_queue_evicts_oldest_first() {
        let mut c = cfg();
        c.max_queue_len = 3;
        let s = DeliveryStore::open_in_memory(c).unwrap();
        let now = now_secs();
        let first = s.enqueue_id("ch", &msg("1"), "NotConnected", now).unwrap();
        s.enqueue_id("ch", &msg("2"), "NotConnected", now).unwrap();
        s.enqueue_id("ch", &msg("3"), "NotConnected", now).unwrap();
        // Fourth enqueue must evict the oldest (id == first) to stay at the cap.
        s.enqueue_id("ch", &msg("4"), "NotConnected", now).unwrap();

        assert_eq!(s.len().unwrap(), 3);
        let surviving: Vec<i64> = s.claim_due(now, 10).unwrap().iter().map(|r| r.id).collect();
        assert!(
            !surviving.contains(&first),
            "oldest record should be evicted"
        );
    }

    #[test]
    fn toml_config_default_matches_runtime_default() {
        let from_toml = DeliveryQueueTomlConfig::default().to_runtime();
        let native = DeliveryQueueConfig::default();
        assert_eq!(from_toml.max_attempts, native.max_attempts);
        assert_eq!(from_toml.initial_backoff, native.initial_backoff);
        assert_eq!(from_toml.max_backoff, native.max_backoff);
        assert_eq!(from_toml.backoff_factor, native.backoff_factor);
        assert_eq!(from_toml.tick, native.tick);
        assert_eq!(from_toml.batch, native.batch);
        assert_eq!(from_toml.max_queue_len, native.max_queue_len);
    }

    #[test]
    fn toml_config_floors_pathological_values() {
        // Every zero/negative must clamp so the drain task can never busy-loop
        // or reschedule a still-down channel as immediately-due.
        let bad = DeliveryQueueTomlConfig {
            max_attempts: 0,
            initial_backoff_secs: 0,
            max_backoff_secs: 0,
            backoff_factor: 0.0,
            tick_secs: 0,
            batch: 0,
            max_queue_len: 0,
            max_payload_bytes: 0,
            inline_flush_max: 0,
        };
        let rt = bad.to_runtime();
        assert_eq!(rt.max_attempts, 1);
        // A zero byte cap would dead-letter every plain-text push.
        assert_eq!(rt.max_payload_bytes, 4096);
        assert_eq!(rt.initial_backoff, Duration::from_secs(1));
        // max_backoff is raised to at least initial_backoff.
        assert_eq!(rt.max_backoff, Duration::from_secs(1));
        assert!(rt.backoff_factor >= 1.0, "curve must never shrink");
        assert_eq!(rt.tick, Duration::from_secs(1));
        assert_eq!(rt.batch, 1);
        assert_eq!(rt.max_queue_len, 1);
        // Deliberately *not* floored: 0 is the meaningful "never flush inline"
        // setting, not a pathological value to clamp away.
        assert_eq!(rt.inline_flush_max, 0);
    }

    #[test]
    fn toml_config_preserves_valid_overrides() {
        let cfg = DeliveryQueueTomlConfig {
            max_attempts: 5,
            initial_backoff_secs: 10,
            max_backoff_secs: 600,
            backoff_factor: 3.0,
            tick_secs: 15,
            batch: 64,
            max_queue_len: 50_000,
            max_payload_bytes: 65_536,
            inline_flush_max: 3,
        };
        let rt = cfg.to_runtime();
        assert_eq!(rt.max_payload_bytes, 65_536);
        assert_eq!(rt.max_attempts, 5);
        assert_eq!(rt.initial_backoff, Duration::from_secs(10));
        assert_eq!(rt.max_backoff, Duration::from_secs(600));
        assert_eq!(rt.backoff_factor, 3.0);
        assert_eq!(rt.tick, Duration::from_secs(15));
        assert_eq!(rt.batch, 64);
        assert_eq!(rt.max_queue_len, 50_000);
        assert_eq!(rt.inline_flush_max, 3);
    }

    #[test]
    fn corrupt_payload_is_dropped_on_claim() {
        let s = store();
        let now = now_secs();
        // Insert a row whose payload is not a valid OutboundMessage.
        s.guard()
            .execute(
                "INSERT INTO outbound_deliveries
                    (channel_id, payload, attempts, next_attempt_at, created_at, last_error)
                 VALUES ('ch', 'not-json', 0, ?1, ?1, 'x')",
                params![now],
            )
            .unwrap();
        assert_eq!(s.len().unwrap(), 1);
        // claim_due must skip and delete the poison row.
        assert!(s.claim_due(now, 10).unwrap().is_empty());
        assert!(s.is_empty(), "corrupt record purged");
    }

    #[test]
    fn stats_reports_depth_age_and_per_channel() {
        let s = store();
        let now = now_secs();
        // Two telegram (one due, one future), one signal (due). Oldest is 100s old.
        s.enqueue_id("telegram", &msg("a"), "NotConnected", now)
            .unwrap();
        s.enqueue_id("telegram", &msg("b"), "NotConnected", now + 3600)
            .unwrap();
        s.enqueue_id("signal", &msg("c"), "NotConnected", now)
            .unwrap();

        let stats = s.stats(now + 100).unwrap();
        assert_eq!(stats.pending, 3);
        assert_eq!(stats.due_now, 2, "two records are already due");
        // created_at was stamped at enqueue (~now); age measured at now+100.
        assert!(
            stats
                .oldest_age_secs
                .is_some_and(|a| (95..=105).contains(&a)),
            "oldest age ~100s, got {:?}",
            stats.oldest_age_secs
        );
        // Busiest channel first.
        assert_eq!(stats.per_channel.first().unwrap().0, "telegram");
        assert_eq!(stats.per_channel.first().unwrap().1, 2);
        assert_eq!(stats.dead_lettered, 0);
    }

    #[test]
    fn stats_on_empty_queue_has_no_oldest() {
        let s = store();
        let stats = s.stats(now_secs()).unwrap();
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.due_now, 0);
        assert!(stats.oldest_age_secs.is_none());
        assert!(stats.per_channel.is_empty());
    }

    #[test]
    fn record_dead_letter_moves_row_preserving_age() {
        let s = store();
        let now = now_secs();
        let id = s
            .enqueue_id("telegram", &msg("lost"), "NotConnected", now)
            .unwrap();
        // Backdate created_at so we can prove the original age is preserved.
        s.guard()
            .execute(
                "UPDATE outbound_deliveries SET created_at = ?1 WHERE id = ?2",
                params![now - 500, id],
            )
            .unwrap();

        s.record_dead_letter(
            id,
            10,
            "NotConnected(\"down\")",
            DeadLetterReason::Exhausted,
        )
        .unwrap();

        // Gone from the live queue, present in the dead-letter table.
        assert!(s.is_empty(), "exhausted record left the live queue");
        let stats = s.stats(now).unwrap();
        assert_eq!(stats.dead_lettered, 1);

        let dl = s.recent_dead_letters(10).unwrap();
        assert_eq!(dl.len(), 1);
        assert_eq!(dl[0].channel_id, "telegram");
        assert_eq!(dl[0].message.text, "lost");
        assert_eq!(dl[0].attempts, 10);
        assert_eq!(
            dl[0].created_at,
            now - 500,
            "original enqueue time preserved"
        );
        assert!(dl[0].last_error.contains("NotConnected"));
    }

    #[test]
    fn record_dead_letter_missing_row_is_noop() {
        let s = store();
        // No such id — must not error, must not create a phantom dead letter.
        s.record_dead_letter(999, 5, "gone", DeadLetterReason::Exhausted)
            .unwrap();
        assert_eq!(s.stats(now_secs()).unwrap().dead_lettered, 0);
    }

    #[test]
    fn dead_letters_are_bounded() {
        let mut c = cfg();
        c.max_queue_len = 2;
        let s = DeliveryStore::open_in_memory(c).unwrap();
        let now = now_secs();
        // Dead-letter three records in order; the table must cap at 2. Eviction
        // is by died_at ASC, with the dead_letters autoincrement id breaking the
        // same-second tie — so the first one dead-lettered ("1") is evicted.
        for t in ["1", "2", "3"] {
            let id = s.enqueue_id("ch", &msg(t), "NotConnected", now).unwrap();
            s.record_dead_letter(id, 10, "exhausted", DeadLetterReason::Exhausted)
                .unwrap();
        }
        let stats = s.stats(now).unwrap();
        assert_eq!(
            stats.dead_lettered, 2,
            "dead-letter table capped at max_queue_len"
        );
        // The two newest survive; the first dead-lettered ("1") is evicted.
        let texts: Vec<String> = s
            .recent_dead_letters(10)
            .unwrap()
            .into_iter()
            .map(|d| d.message.text)
            .collect();
        assert!(
            !texts.contains(&"1".to_string()),
            "oldest dead letter evicted"
        );
    }

    #[test]
    fn redrive_moves_dead_letters_back_to_live_queue() {
        let s = store();
        let now = now_secs();
        // Dead-letter two records.
        for t in ["a", "b"] {
            let id = s
                .enqueue_id("telegram", &msg(t), "NotConnected", now)
                .unwrap();
            s.record_dead_letter(id, 10, "exhausted", DeadLetterReason::Exhausted)
                .unwrap();
        }
        assert_eq!(s.stats(now).unwrap().dead_lettered, 2);
        assert!(s.is_empty(), "no live records before redrive");

        // Redrive: both move back, immediately due, fresh budget.
        let outcome = s.redrive_dead_letters(now, None).unwrap();
        assert_eq!(outcome.moved, 2);
        assert_eq!(outcome.skipped_capacity, 0);
        assert_eq!(outcome.skipped_unsafe, 0);
        assert_eq!(
            s.stats(now).unwrap().dead_lettered,
            0,
            "dead letters cleared"
        );

        let due = s.claim_due(now, 10).unwrap();
        assert_eq!(due.len(), 2, "both are live and due");
        assert!(
            due.iter().all(|r| r.attempts == 0),
            "redriven records get a fresh retry budget"
        );
    }

    #[test]
    fn redrive_can_filter_by_channel() {
        let s = store();
        let now = now_secs();
        for (ch, t) in [("telegram", "x"), ("signal", "y")] {
            let id = s.enqueue_id(ch, &msg(t), "NotConnected", now).unwrap();
            s.record_dead_letter(id, 10, "exhausted", DeadLetterReason::Exhausted)
                .unwrap();
        }

        // Only telegram is redriven; signal stays dead-lettered.
        let moved = s.redrive_dead_letters(now, Some("telegram")).unwrap().moved;
        assert_eq!(moved, 1);
        assert_eq!(s.stats(now).unwrap().dead_lettered, 1);

        let due = s.claim_due(now, 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].channel_id, "telegram");

        let remaining = s.recent_dead_letters(10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].channel_id, "signal");
    }

    #[test]
    fn redrive_on_empty_is_zero() {
        let s = store();
        assert_eq!(
            s.redrive_dead_letters(now_secs(), None).unwrap(),
            RedriveOutcome::default()
        );
    }

    #[test]
    fn redrive_respects_live_queue_bound() {
        let mut c = cfg();
        c.max_queue_len = 2;
        let s = DeliveryStore::open_in_memory(c).unwrap();
        let now = now_secs();
        // Dead-letter three records (the dead_letters table itself is capped at
        // 2, so only the two newest survive to be redriven).
        for t in ["1", "2", "3"] {
            let id = s.enqueue_id("ch", &msg(t), "NotConnected", now).unwrap();
            s.record_dead_letter(id, 10, "exhausted", DeadLetterReason::Exhausted)
                .unwrap();
        }
        assert_eq!(s.stats(now).unwrap().dead_lettered, 2);

        let outcome = s.redrive_dead_letters(now, None).unwrap();
        assert_eq!(outcome.moved, 2);
        // Live queue honors its own cap.
        assert!(s.len().unwrap() <= 2, "redrive cannot blow the live bound");
    }

    #[test]
    fn redrive_never_evicts_live_pending_work_to_make_room() {
        // The regression this replaces: redrive used to move everything and
        // then trim `outbound_deliveries` by oldest created_at. Redriven rows
        // carry created_at = now, so the rows trimmed were the genuinely-older
        // PENDING deliveries — a recovery action destroying in-flight work.
        let mut c = cfg();
        c.max_queue_len = 2;
        let s = DeliveryStore::open_in_memory(c).unwrap();
        let now = now_secs();

        // Retire one record first (so filling the live queue afterwards cannot
        // evict it through `enqueue`'s own oldest-first bound)...
        let dead = s
            .enqueue_id("ch", &msg("dead-1"), "NotConnected", now)
            .unwrap();
        s.record_dead_letter(dead, 10, "exhausted", DeadLetterReason::Exhausted)
            .unwrap();
        // ...then fill the live queue to capacity with pending work.
        s.enqueue_id("ch", &msg("live-1"), "NotConnected", now + 60)
            .unwrap();
        s.enqueue_id("ch", &msg("live-2"), "NotConnected", now + 60)
            .unwrap();
        assert_eq!(s.len().unwrap(), 2, "live queue is at max_queue_len");

        let outcome = s.redrive_dead_letters(now, None).unwrap();
        assert_eq!(outcome.moved, 0, "no capacity, so nothing moves");
        assert_eq!(outcome.skipped_capacity, 1);
        assert_eq!(
            s.stats(now).unwrap().dead_lettered,
            1,
            "the record it could not admit stays inspectable, not lost"
        );

        let live: Vec<String> = s
            .claim_due(now + 120, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.message.text)
            .collect();
        assert!(
            live.contains(&"live-1".to_string()) && live.contains(&"live-2".to_string()),
            "pending deliveries survive a redrive; got {live:?}"
        );
    }

    #[test]
    fn redrive_refuses_records_whose_outcome_is_unknown() {
        let s = store();
        let now = now_secs();
        let safe = s
            .enqueue_id("ch", &msg("never-sent"), "NotConnected", now)
            .unwrap();
        s.record_dead_letter(safe, 10, "exhausted", DeadLetterReason::Exhausted)
            .unwrap();
        let risky = s.enqueue_id("ch", &msg("maybe-sent"), "x", now).unwrap();
        s.record_dead_letter(risky, 1, "SendFailed", DeadLetterReason::Ambiguous)
            .unwrap();

        let outcome = s.redrive_dead_letters(now, None).unwrap();
        assert_eq!(outcome.moved, 1, "only the provably-undelivered one");
        assert_eq!(outcome.skipped_unsafe, 1);

        // The ambiguous one is still there to be inspected, never auto-replayed.
        let left = s.recent_dead_letters(10).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].message.text, "maybe-sent");
        assert!(!left[0].reason.replay_safe());
    }

    #[test]
    fn stats_separates_replayable_dead_letters_from_the_rest() {
        let s = store();
        let now = now_secs();
        for (text, reason) in [
            ("a", DeadLetterReason::Exhausted),
            ("b", DeadLetterReason::Permanent),
            ("c", DeadLetterReason::Ambiguous),
            ("d", DeadLetterReason::UnknownOutcome),
        ] {
            let id = s.enqueue_id("ch", &msg(text), "e", now).unwrap();
            s.record_dead_letter(id, 1, "e", reason).unwrap();
        }
        let stats = s.stats(now).unwrap();
        assert_eq!(stats.dead_lettered, 4);
        assert_eq!(
            stats.dead_lettered_replayable, 2,
            "exhausted + permanent are replay-safe; ambiguous + unknown are not"
        );
    }

    #[test]
    fn interrupted_sends_are_retired_not_replayed() {
        // The crash window: a record stamped in-flight whose outcome nobody
        // recorded must never be re-sent automatically, because it may already
        // have been delivered.
        let s = store();
        let now = now_secs();
        let id = s
            .enqueue_id("telegram", &msg("maybe-delivered"), "NotConnected", now)
            .unwrap();
        s.mark_inflight(id, now).unwrap();

        let moved = s.reconcile_inflight(now + 5).unwrap();
        assert_eq!(moved, 1);
        assert!(
            s.claim_due(now + 5, 10).unwrap().is_empty(),
            "an interrupted send is not left claimable"
        );

        let dl = s.recent_dead_letters(10).unwrap();
        assert_eq!(dl.len(), 1);
        assert_eq!(dl[0].reason, DeadLetterReason::UnknownOutcome);
        assert!(!dl[0].reason.replay_safe());
        assert_eq!(
            dl[0].created_at, now,
            "true age preserved, not reconciliation time"
        );
    }

    #[test]
    fn a_settled_attempt_is_not_mistaken_for_an_interrupted_one() {
        // `reschedule` clears the stamp, so a reported transient failure stays
        // in the live queue instead of being retired as unknown-outcome.
        let s = store();
        let now = now_secs();
        let id = s.enqueue_id("ch", &msg("retry me"), "x", now).unwrap();
        s.mark_inflight(id, now).unwrap();
        s.reschedule(id, 1, now + 10, "NotConnected").unwrap();

        assert_eq!(s.reconcile_inflight(now + 20).unwrap(), 0);
        assert_eq!(s.claim_due(now + 20, 10).unwrap().len(), 1);
    }

    #[test]
    fn oversized_payloads_are_dead_lettered_instead_of_queued() {
        // Row count is the wrong dimension for a table whose rows can carry
        // inline attachment bytes.
        let mut c = cfg();
        c.max_payload_bytes = 512;
        let s = DeliveryStore::open_in_memory(c).unwrap();
        let now = now_secs();

        let big = OutboundMessage::text("conv-1", "x".repeat(2000));
        let outcome = s.enqueue("ch", &big, "NotConnected", now).unwrap();
        assert!(matches!(outcome, EnqueueOutcome::TooLarge { .. }));
        assert!(s.is_empty(), "never admitted to the live queue");

        let dl = s.recent_dead_letters(10).unwrap();
        assert_eq!(dl.len(), 1, "but still visible to the operator");
        assert_eq!(dl[0].reason, DeadLetterReason::PayloadTooLarge);
        assert!(
            !dl[0].reason.replay_safe(),
            "redriving it would re-hit the cap"
        );

        // A normal message still goes through.
        assert!(matches!(
            s.enqueue("ch", &msg("small"), "NotConnected", now).unwrap(),
            EnqueueOutcome::Queued(_)
        ));
    }

    // ---- drain behaviour (needs a registry with a fake transport) -----------

    /// Channel that records every delivered text in order and fails any text
    /// listed in `failing` with a *duplicate-safe* transient error.
    struct RecordingChannel {
        info: super::super::channel::ChannelInfo,
        state: super::super::channel::ChannelState,
        sent: Arc<Mutex<Vec<String>>>,
        failing: Arc<Mutex<Vec<String>>>,
        terminal: bool,
    }

    impl RecordingChannel {
        fn new(sent: Arc<Mutex<Vec<String>>>, failing: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                info: super::super::channel::ChannelInfo {
                    id: ChannelId::new("rec"),
                    name: "rec".to_string(),
                    channel_type: "test".to_string(),
                    status: super::super::channel::ChannelStatus::Connected,
                    capabilities: super::super::channel::ChannelCapabilities::default(),
                },
                state: super::super::channel::ChannelState::new(8),
                sent,
                failing,
                terminal: false,
            }
        }

        fn terminal(sent: Arc<Mutex<Vec<String>>>) -> Self {
            let mut c = Self::new(sent, Arc::new(Mutex::new(Vec::new())));
            c.terminal = true;
            c
        }
    }

    #[async_trait::async_trait]
    impl super::super::channel::Channel for RecordingChannel {
        fn info(&self) -> &super::super::channel::ChannelInfo {
            &self.info
        }
        fn state(&self) -> &super::super::channel::ChannelState {
            &self.state
        }
        async fn start(&mut self) -> super::super::channel::ChannelResult<()> {
            Ok(())
        }
        async fn stop(&mut self) -> super::super::channel::ChannelResult<()> {
            Ok(())
        }
        async fn send(
            &self,
            message: OutboundMessage,
        ) -> super::super::channel::ChannelResult<super::super::channel::SendResult> {
            if self.terminal {
                return Err(ChannelError::SendFailed("permanent".into()));
            }
            if self
                .failing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&message.text)
            {
                return Err(ChannelError::NotConnected("down".into()));
            }
            self.sent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(message.text.clone());
            Ok(super::super::channel::SendResult {
                message_id: super::super::channel::MessageId::new("ok"),
                timestamp: chrono::Utc::now(),
            })
        }
    }

    fn conv_msg(conv: &str, text: &str) -> OutboundMessage {
        OutboundMessage::text(conv, text)
    }

    #[tokio::test]
    async fn a_conversation_drains_in_order_and_stops_at_its_first_failure() {
        // Two messages queued for the same chat: if the first cannot go out,
        // sending the second anyway delivers them in the wrong order — and
        // permanently, since the failed one is rescheduled behind it.
        let sent = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(Mutex::new(vec!["a1".to_string()]));
        let registry = Arc::new(ChannelRegistry::new());
        registry
            .register(Box::new(RecordingChannel::new(
                Arc::clone(&sent),
                Arc::clone(&failing),
            )))
            .await;

        let store = DeliveryStore::open_in_memory(cfg()).unwrap();
        let now = now_secs();
        store
            .enqueue_id("rec", &conv_msg("A", "a1"), "NotConnected", now)
            .unwrap();
        store
            .enqueue_id("rec", &conv_msg("A", "a2"), "NotConnected", now)
            .unwrap();
        store
            .enqueue_id("rec", &conv_msg("B", "b1"), "NotConnected", now)
            .unwrap();

        drain_once(&registry, &store).await;
        {
            let delivered = sent.lock().unwrap_or_else(|e| e.into_inner()).clone();
            assert_eq!(
                delivered,
                vec!["b1".to_string()],
                "a2 must wait behind a1; an unrelated conversation is not blocked"
            );
        }

        // The follower must have inherited the head's backoff. Without that,
        // the next tick claims a2 alone (a1 is no longer due) and delivers it
        // first — which is exactly what this assertion used to catch.
        assert!(
            store.claim_due(now, 10).unwrap().is_empty(),
            "a2 must not stay due while a1 is backing off"
        );

        // The transport recovers and both become due again.
        failing.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let due_later = now + 10_000;
        drain_once(&registry, &store).await; // nothing due yet: no-op
        assert_eq!(
            sent.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1,
            "backoff is honored"
        );

        // Advance time by making both records due, preserving their relative
        // order (claim_due orders by next_attempt_at, ties broken by id).
        for rec in store.claim_due(due_later, 10).unwrap() {
            store
                .reschedule(rec.id, rec.attempts, now, "test: force due")
                .unwrap();
        }
        drain_once(&registry, &store).await;
        let delivered = sent.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            delivered,
            vec!["b1", "a1", "a2"],
            "order preserved per conversation"
        );
    }

    #[tokio::test]
    async fn a_terminal_drain_failure_leaves_a_forensic_trail() {
        // Previously hard-deleted: the one loss path the dead-letter trail did
        // not cover, and the trail exists so an undelivered push is never
        // silent.
        let sent = Arc::new(Mutex::new(Vec::new()));
        let registry = Arc::new(ChannelRegistry::new());
        registry
            .register(Box::new(RecordingChannel::terminal(Arc::clone(&sent))))
            .await;

        let store = DeliveryStore::open_in_memory(cfg()).unwrap();
        let now = now_secs();
        store
            .enqueue_id("rec", &conv_msg("A", "doomed"), "NotConnected", now)
            .unwrap();

        drain_once(&registry, &store).await;
        assert!(store.is_empty(), "record leaves the live queue");
        let dl = store.recent_dead_letters(10).unwrap();
        assert_eq!(dl.len(), 1);
        assert_eq!(dl[0].message.text, "doomed");
        assert_eq!(
            dl[0].reason,
            DeadLetterReason::Ambiguous,
            "SendFailed may already be on the wire"
        );
        assert!(!dl[0].reason.replay_safe());
    }

    #[tokio::test]
    async fn a_delivered_record_leaves_no_inflight_stamp_behind() {
        // The stamp must be cleared by settling, or the next boot's
        // reconciliation would retire records that were delivered cleanly.
        let sent = Arc::new(Mutex::new(Vec::new()));
        let registry = Arc::new(ChannelRegistry::new());
        registry
            .register(Box::new(RecordingChannel::new(
                Arc::clone(&sent),
                Arc::new(Mutex::new(Vec::new())),
            )))
            .await;

        let store = DeliveryStore::open_in_memory(cfg()).unwrap();
        let now = now_secs();
        store
            .enqueue_id("rec", &conv_msg("A", "fine"), "NotConnected", now)
            .unwrap();
        drain_once(&registry, &store).await;

        assert_eq!(sent.lock().unwrap_or_else(|e| e.into_inner()).len(), 1);
        assert_eq!(store.reconcile_inflight(now + 1).unwrap(), 0);
        assert_eq!(store.stats(now).unwrap().dead_lettered, 0);
    }

    // ---- ordering across the live/queued boundary ---------------------------

    /// Registry wired to `store`, serving the `rec` channel.
    async fn registry_with(
        store: &Arc<DeliveryStore>,
        sent: &Arc<Mutex<Vec<String>>>,
        failing: &Arc<Mutex<Vec<String>>>,
    ) -> Arc<ChannelRegistry> {
        let registry = Arc::new(ChannelRegistry::new().with_delivery_store(Arc::clone(store)));
        registry
            .register(Box::new(RecordingChannel::new(
                Arc::clone(sent),
                Arc::clone(failing),
            )))
            .await;
        registry
    }

    #[test]
    fn claim_conversation_ignores_backoff_and_returns_enqueue_order() {
        // The whole point: the record a live send would overtake is the one
        // sitting in the *future*, so this claim must not filter by due-ness.
        let s = store();
        let far = now_secs() + 10_000;
        s.enqueue_id("rec", &conv_msg("A", "a1"), "NotConnected", far)
            .unwrap();
        s.enqueue_id("rec", &conv_msg("A", "a2"), "NotConnected", far)
            .unwrap();
        s.enqueue_id("rec", &conv_msg("B", "b1"), "NotConnected", far)
            .unwrap();

        assert!(
            s.claim_due(now_secs(), 10).unwrap().is_empty(),
            "nothing is due — which is exactly when the old bug showed"
        );
        let claimed = s.claim_conversation("rec", "A", 10).unwrap();
        assert_eq!(
            claimed.iter().map(|r| &r.message.text).collect::<Vec<_>>(),
            vec!["a1", "a2"],
            "this conversation only, in enqueue order"
        );
    }

    #[tokio::test]
    async fn a_live_send_flushes_its_conversation_backlog_first() {
        // The other half of per-conversation ordering: a message that never
        // failed used to take the live path and land ahead of replies that had
        // been queued minutes earlier.
        let sent = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(DeliveryStore::open_in_memory(cfg()).unwrap());
        let registry = registry_with(&store, &sent, &failing).await;

        // Backlog serving out a long backoff, i.e. not due for a long time.
        let far = now_secs() + 10_000;
        store
            .enqueue_id("rec", &conv_msg("A", "queued1"), "NotConnected", far)
            .unwrap();
        store
            .enqueue_id("rec", &conv_msg("A", "queued2"), "NotConnected", far)
            .unwrap();
        store
            .enqueue_id("rec", &conv_msg("B", "other"), "NotConnected", far)
            .unwrap();

        registry
            .send(&ChannelId::new("rec"), conv_msg("A", "live"))
            .await
            .expect("the live send still reports its own result");

        assert_eq!(
            sent.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec!["queued1", "queued2", "live"],
            "the backlog goes out first; an unrelated conversation is untouched"
        );
        assert_eq!(
            store.claim_conversation("rec", "B", 10).unwrap().len(),
            1,
            "conversation B was not flushed by a send addressed to A"
        );
    }

    #[tokio::test]
    async fn an_inline_flush_never_spends_the_backlogs_retry_budget() {
        // An opportunistic probe must leave the record exactly as it found it.
        // Otherwise ten user messages to a wedged channel burn the head's ten
        // attempts and dead-letter it far ahead of the configured curve.
        let sent = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(Mutex::new(vec!["queued".to_string(), "live".to_string()]));
        let store = Arc::new(DeliveryStore::open_in_memory(cfg()).unwrap());
        let registry = registry_with(&store, &sent, &failing).await;

        let far = now_secs() + 10_000;
        let id = store
            .enqueue_id("rec", &conv_msg("A", "queued"), "NotConnected", far)
            .unwrap();

        for _ in 0..3 {
            let _ = registry
                .send(&ChannelId::new("rec"), conv_msg("A", "live"))
                .await;
        }

        let head = store
            .claim_conversation("rec", "A", 10)
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .expect("head still queued");
        assert_eq!(head.attempts, 0, "probes must not consume attempts");
        assert_eq!(
            store.stats(now_secs()).unwrap().dead_lettered,
            0,
            "and must not retire it early"
        );
        assert_eq!(
            store.reconcile_inflight(now_secs() + 1).unwrap(),
            0,
            "a failed probe leaves no in-flight stamp for the next boot to misread"
        );
        // The failing live sends queued themselves behind the head, in order.
        let conv = store.claim_conversation("rec", "A", 10).unwrap();
        assert_eq!(
            conv.iter().map(|r| &r.message.text).collect::<Vec<_>>(),
            vec!["queued", "live", "live", "live"],
            "order survives even when nothing can be delivered"
        );
    }

    #[tokio::test]
    async fn an_inline_flush_retires_an_ambiguous_failure_rather_than_reattempting_it() {
        // The one failure an inline probe must settle: `SendFailed` may already
        // be on the wire, so handing it back for the drain loop to retry is how
        // at-most-once dies.
        let sent = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(DeliveryStore::open_in_memory(cfg()).unwrap());
        let registry = Arc::new(ChannelRegistry::new().with_delivery_store(Arc::clone(&store)));
        registry
            .register(Box::new(RecordingChannel::terminal(Arc::clone(&sent))))
            .await;

        store
            .enqueue_id(
                "rec",
                &conv_msg("A", "ambiguous"),
                "NotConnected",
                now_secs() + 10_000,
            )
            .unwrap();

        let _ = registry
            .send(&ChannelId::new("rec"), conv_msg("A", "live"))
            .await;

        assert!(
            store.claim_conversation("rec", "A", 10).unwrap().is_empty(),
            "the ambiguous record left the live queue instead of waiting for a second attempt"
        );
        let dl = store.recent_dead_letters(10).unwrap();
        assert_eq!(dl.len(), 1);
        assert_eq!(dl[0].message.text, "ambiguous");
        assert!(!dl[0].reason.replay_safe());
    }

    #[tokio::test]
    async fn the_inline_flush_stands_down_while_the_drainer_holds_the_gate() {
        // Blocking a user-facing send behind a whole drain tick is worse than
        // the one-tick reorder window that skipping leaves.
        let sent = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(DeliveryStore::open_in_memory(cfg()).unwrap());
        let registry = registry_with(&store, &sent, &failing).await;
        store
            .enqueue_id(
                "rec",
                &conv_msg("A", "queued"),
                "NotConnected",
                now_secs() + 10_000,
            )
            .unwrap();

        let held = store.drain_gate.lock().await;
        registry
            .send(&ChannelId::new("rec"), conv_msg("A", "live"))
            .await
            .unwrap();
        drop(held);

        assert_eq!(
            sent.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec!["live"],
            "the send went out without touching the gated queue"
        );
        assert_eq!(
            store.claim_conversation("rec", "A", 10).unwrap().len(),
            1,
            "and the backlog is still there for the drainer"
        );
    }

    #[tokio::test]
    async fn a_backlog_deeper_than_the_cap_flushes_only_the_cap() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(Mutex::new(Vec::new()));
        let mut c = cfg();
        c.inline_flush_max = 2;
        let store = Arc::new(DeliveryStore::open_in_memory(c).unwrap());
        let registry = registry_with(&store, &sent, &failing).await;

        let far = now_secs() + 10_000;
        for i in 0..4 {
            store
                .enqueue_id("rec", &conv_msg("A", &format!("q{i}")), "NotConnected", far)
                .unwrap();
        }
        registry
            .send(&ChannelId::new("rec"), conv_msg("A", "live"))
            .await
            .unwrap();

        assert_eq!(
            sent.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec!["q0", "q1", "live"],
            "bounded: the send is not held hostage by an unbounded backlog"
        );
    }

    #[tokio::test]
    async fn a_zero_cap_disables_the_inline_flush() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(Mutex::new(Vec::new()));
        let mut c = cfg();
        c.inline_flush_max = 0;
        let store = Arc::new(DeliveryStore::open_in_memory(c).unwrap());
        let registry = registry_with(&store, &sent, &failing).await;
        store
            .enqueue_id(
                "rec",
                &conv_msg("A", "queued"),
                "NotConnected",
                now_secs() + 10_000,
            )
            .unwrap();

        registry
            .send(&ChannelId::new("rec"), conv_msg("A", "live"))
            .await
            .unwrap();
        assert_eq!(
            sent.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec!["live"]
        );
    }

    // ---- media custody ------------------------------------------------------

    fn with_attachment(att: super::super::channel::Attachment) -> OutboundMessage {
        let mut m = conv_msg("A", "caption");
        m.attachments = vec![att];
        m
    }

    fn path_attachment(path: &str) -> super::super::channel::Attachment {
        super::super::channel::Attachment {
            id: "att".into(),
            mime_type: "image/png".into(),
            filename: None,
            size: None,
            url: None,
            path: Some(path.to_string()),
            data: None,
        }
    }

    #[tokio::test]
    async fn media_custody_inlines_a_local_file_and_keeps_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shot.png");
        std::fs::write(&file, b"pixels").unwrap();

        let msg = with_attachment(path_attachment(file.to_str().unwrap()));
        let taken = take_media_custody(&msg, 1_048_576)
            .await
            .expect("a path attachment needs custody");

        let att = &taken.attachments[0];
        assert_eq!(att.data.as_deref(), Some(&b"pixels"[..]));
        assert_eq!(att.size, Some(6));
        assert!(
            att.path.is_none(),
            "the row must have exactly one source of truth"
        );
        assert_eq!(
            att.filename.as_deref(),
            Some("shot.png"),
            "inline bytes carry no name of their own; the path's name has to move with them"
        );
    }

    #[tokio::test]
    async fn media_custody_declines_rather_than_degrading_the_message() {
        // Both refusals must leave the message exactly as the caller wrote it,
        // so a queued row is never *less* deliverable than it is today.
        let missing = with_attachment(path_attachment("/nonexistent/aleph-custody-test.png"));
        assert!(
            take_media_custody(&missing, 1_048_576).await.is_none(),
            "an unreadable file leaves the path in place"
        );

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.bin");
        std::fs::write(&file, vec![0u8; 4096]).unwrap();
        let big = with_attachment(path_attachment(file.to_str().unwrap()));
        assert!(
            take_media_custody(&big, 512).await.is_none(),
            "over the cap, a path that might still be there beats a dead letter"
        );

        // Nothing to do at all: no clone, no read.
        let plain = conv_msg("A", "text only");
        assert!(take_media_custody(&plain, 1_048_576).await.is_none());
    }

    #[tokio::test]
    async fn a_queued_row_carries_its_media_rather_than_a_temp_path() {
        // End to end: the file is deleted after the send fails, exactly as the
        // OS would delete it across a restart, and the record still replays.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tts.mp3");
        std::fs::write(&file, b"audio").unwrap();

        let sent = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(Mutex::new(vec!["voice".to_string()]));
        let store = Arc::new(DeliveryStore::open_in_memory(cfg()).unwrap());
        let registry = registry_with(&store, &sent, &failing).await;

        let mut msg = conv_msg("A", "voice");
        msg.attachments = vec![path_attachment(file.to_str().unwrap())];
        let _ = registry.send(&ChannelId::new("rec"), msg).await;

        std::fs::remove_file(&file).unwrap();

        let queued = store.claim_conversation("rec", "A", 10).unwrap();
        assert_eq!(queued.len(), 1);
        let att = &queued[0].message.attachments[0];
        assert_eq!(
            att.data.as_deref(),
            Some(&b"audio"[..]),
            "the row survived the file"
        );
        assert!(att.path.is_none());
    }

    #[test]
    fn every_dead_letter_reason_is_listed_in_all() {
        // Exhaustive match: adding a variant without adding it to `ALL` fails
        // to compile here, which is what keeps the replay-safe projection in
        // `stats` / `redrive_dead_letters` complete.
        for r in DeadLetterReason::ALL {
            match r {
                DeadLetterReason::Exhausted
                | DeadLetterReason::Ambiguous
                | DeadLetterReason::Permanent
                | DeadLetterReason::UnknownOutcome
                | DeadLetterReason::PayloadTooLarge => {}
            }
            assert_eq!(DeadLetterReason::from_str_lossy(r.as_str()), *r);
        }
        assert_eq!(DeadLetterReason::ALL.len(), 5);
    }
}
