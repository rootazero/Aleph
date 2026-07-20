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
//! openclaw's `src/infra/session-delivery-queue.ts` (SQLite-backed, survives
//! restarts, drains on backoff). Aleph improves on it with type-safe,
//! **duplicate-safe** error classification: only pre-delivery failures
//! ([`NotConnected`](ChannelError::NotConnected) /
//! [`RateLimited`](ChannelError::RateLimited)) are retried. Ambiguous errors
//! (`SendFailed` — the message may already be on the wire) are never retried,
//! preserving the at-most-once guarantee the registry deliberately maintains.
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
    /// Records that exhausted their retry budget and were dead-lettered rather
    /// than silently deleted (forensic trail; parity-plus over openclaw).
    pub dead_lettered: i64,
}

/// A delivery that exhausted its retry budget, retained for forensics instead
/// of being hard-deleted. Read back via [`DeliveryStore::recent_dead_letters`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetter {
    /// Target channel id the message could never be delivered to.
    pub channel_id: String,
    /// The undelivered message.
    pub message: OutboundMessage,
    /// Attempts made before giving up.
    pub attempts: u32,
    /// Last transient error observed before the budget was exhausted.
    pub last_error: String,
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
        Ok(Self {
            conn: Mutex::new(conn),
            config,
            drain_spawned: AtomicBool::new(false),
        })
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
    /// cap by evicting the oldest rows first. Returns the new row id.
    pub fn enqueue(
        &self,
        channel_id: &str,
        message: &OutboundMessage,
        last_error: &str,
        next_attempt_at: i64,
    ) -> rusqlite::Result<i64> {
        let payload = serde_json::to_string(message)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let now = now_secs();
        let conn = self.guard();

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
                (channel_id, payload, attempts, next_attempt_at, created_at, last_error)
             VALUES (?1, ?2, 0, ?3, ?4, ?5)",
            params![channel_id, payload, next_attempt_at, now, last_error],
        )?;
        Ok(conn.last_insert_rowid())
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
        Ok(out)
    }

    /// Delete a record after a successful delivery.
    pub fn mark_delivered(&self, id: i64) -> rusqlite::Result<()> {
        self.guard()
            .execute("DELETE FROM outbound_deliveries WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Bump attempt count and push the next attempt into the future.
    pub fn reschedule(
        &self,
        id: i64,
        attempts: u32,
        next_attempt_at: i64,
        last_error: &str,
    ) -> rusqlite::Result<()> {
        self.guard().execute(
            "UPDATE outbound_deliveries
             SET attempts = ?1, next_attempt_at = ?2, last_error = ?3
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

    /// Move a record that exhausted its retry budget into the `dead_letters`
    /// table instead of hard-deleting it, so an undelivered proactive push
    /// leaves a forensic trail (channel, attempts, last error, age). The move is
    /// transactional — the row is never in both tables nor lost between them —
    /// and the dead-letter table is bounded by the same `max_queue_len`
    /// (CWE-400), evicting the oldest dead letters first. A missing source row
    /// (already drained) is a no-op.
    pub fn record_dead_letter(
        &self,
        id: i64,
        attempts: u32,
        last_error: &str,
    ) -> rusqlite::Result<()> {
        let died_at = now_secs();
        let mut conn = self.guard();
        let tx = conn.transaction()?;

        // Snapshot the source row (preserving the original created_at so the
        // dead letter reports true age, not move time) before deleting it.
        let row: Option<(String, String, i64)> = tx
            .query_row(
                "SELECT channel_id, payload, created_at FROM outbound_deliveries WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        if let Some((channel_id, payload, created_at)) = row {
            let len: i64 = tx.query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))?;
            if len >= self.config.max_queue_len {
                let overflow = len - self.config.max_queue_len + 1;
                tx.execute(
                    "DELETE FROM dead_letters WHERE id IN (
                        SELECT id FROM dead_letters ORDER BY died_at ASC LIMIT ?1)",
                    params![overflow],
                )?;
            }
            tx.execute(
                "INSERT INTO dead_letters
                    (channel_id, payload, attempts, last_error, created_at, died_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    channel_id,
                    payload,
                    i64::from(attempts),
                    last_error,
                    created_at,
                    died_at
                ],
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

        Ok(DeliveryQueueStats {
            pending,
            due_now,
            oldest_age_secs,
            per_channel,
            dead_lettered,
        })
    }

    /// Most-recently dead-lettered deliveries, newest first, for troubleshooting
    /// (parity-plus over openclaw, which retains failed rows but exposes no
    /// inspection path). Rows whose payload no longer deserializes are skipped.
    pub fn recent_dead_letters(&self, limit: usize) -> rusqlite::Result<Vec<DeadLetter>> {
        let conn = self.guard();
        let raw: Vec<(String, String, i64, Option<String>, i64, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT channel_id, payload, attempts, last_error, created_at, died_at
                 FROM dead_letters ORDER BY died_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::with_capacity(raw.len());
        for (channel_id, payload, attempts, last_error, created_at, died_at) in raw {
            if let Ok(message) = serde_json::from_str::<OutboundMessage>(&payload) {
                out.push(DeadLetter {
                    channel_id,
                    message,
                    attempts: attempts.max(0) as u32,
                    last_error: last_error.unwrap_or_default(),
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
    /// **Safe by construction**: a record only reaches `dead_letters` after
    /// exhausting retries for a *duplicate-safe* transient error
    /// ([`should_enqueue`] admits only `NotConnected` / `RateLimited`), so
    /// replaying it can never produce a duplicate — the property that lets Aleph
    /// expose a redrive neither openclaw nor hermes offers.
    ///
    /// Each moved record is reset to a fresh budget (`attempts = 0`) and made
    /// immediately due (`next_attempt_at = created_at = now`), preserving its
    /// prior `last_error` as context. The whole batch moves in one transaction,
    /// and the live queue's `max_queue_len` cap is enforced once afterward
    /// (oldest `created_at` evicted first, matching [`enqueue`](Self::enqueue));
    /// since redriven rows carry `created_at = now` they never starve genuinely
    /// older pending work.
    pub fn redrive_dead_letters(&self, now: i64, channel: Option<&str>) -> rusqlite::Result<u64> {
        fn map_row(r: &rusqlite::Row) -> rusqlite::Result<(i64, String, String, Option<String>)> {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        }

        let mut conn = self.guard();
        let tx = conn.transaction()?;

        // Snapshot the dead letters to move (optionally filtered by channel)
        // before mutating either table, so the statement is released first.
        let rows: Vec<(i64, String, String, Option<String>)> = match channel {
            Some(ch) => {
                let mut stmt = tx.prepare(
                    "SELECT id, channel_id, payload, last_error
                     FROM dead_letters WHERE channel_id = ?1",
                )?;
                let mapped = stmt.query_map(params![ch], map_row)?;
                mapped.collect::<rusqlite::Result<Vec<_>>>()?
            }
            None => {
                let mut stmt =
                    tx.prepare("SELECT id, channel_id, payload, last_error FROM dead_letters")?;
                let mapped = stmt.query_map([], map_row)?;
                mapped.collect::<rusqlite::Result<Vec<_>>>()?
            }
        };

        let mut moved = 0u64;
        for (id, channel_id, payload, last_error) in &rows {
            tx.execute(
                "INSERT INTO outbound_deliveries
                    (channel_id, payload, attempts, next_attempt_at, created_at, last_error)
                 VALUES (?1, ?2, 0, ?3, ?3, ?4)",
                params![channel_id, payload, now, last_error],
            )?;
            tx.execute("DELETE FROM dead_letters WHERE id = ?1", params![id])?;
            moved += 1;
        }

        // Enforce the live-queue bound once after the batch (CWE-400).
        let len: i64 =
            tx.query_row("SELECT COUNT(*) FROM outbound_deliveries", [], |r| r.get(0))?;
        if len > self.config.max_queue_len {
            let overflow = len - self.config.max_queue_len;
            tx.execute(
                "DELETE FROM outbound_deliveries WHERE id IN (
                    SELECT id FROM outbound_deliveries ORDER BY created_at ASC LIMIT ?1)",
                params![overflow],
            )?;
        }

        tx.commit()?;
        Ok(moved)
    }
}

/// Drive one pass over all currently-due records: claim, attempt delivery via
/// the enqueue-free [`send_attempt`](ChannelRegistry::send_attempt), then settle
/// each record (delivered → delete, transient → reschedule with backoff,
/// exhausted → dead-letter, ambiguous/permanent → drop). Factored out of
/// [`drain_loop`] to keep the loop body readable.
async fn drain_once(registry: &ChannelRegistry, store: &DeliveryStore) {
    let cfg = store.config();
    let now = now_secs();
    let due = match store.claim_due(now, cfg.batch) {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "delivery queue: claim_due failed");
            return;
        }
    };

    for rec in due {
        let channel = ChannelId(rec.channel_id.clone());
        // Call the enqueue-free send path: the public `send` would re-persist
        // the record on transient failure, doubling rows on every drain tick.
        match registry.send_attempt(&channel, rec.message.clone()).await {
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
            }
            Err(e) if should_enqueue(&e) => {
                let attempts = rec.attempts + 1;
                if attempts >= cfg.max_attempts {
                    // Exhausted: dead-letter rather than hard-delete so the lost
                    // push leaves a forensic trail (R5 correctness). Fall back to
                    // a plain drop if the move itself fails so the record can
                    // never wedge the queue — and report the outcome accurately
                    // (a fallen-back drop is *not* a dead letter to inspect later).
                    match store.record_dead_letter(rec.id, attempts, &format!("{e:?}")) {
                        Ok(()) => warn!(
                            id = rec.id,
                            channel = %rec.channel_id,
                            attempts,
                            "delivery queue: giving up after exhausting retries (dead-lettered)"
                        ),
                        Err(dl_err) => {
                            warn!(id = rec.id, error = %dl_err, "delivery queue: dead-letter failed; dropping");
                            let _ = store.drop_record(rec.id, "dead-letter failed");
                        }
                    }
                } else {
                    // Floor at 1s: a sub-second backoff truncates to 0 through
                    // `as_secs()`, which would reschedule the record as
                    // immediately-due and hot-retry a still-down channel.
                    let next = now + backoff_delay(cfg, attempts).as_secs().max(1) as i64;
                    let _ = store.reschedule(rec.id, attempts, next, &format!("{e:?}"));
                }
            }
            // Ambiguous (may already be on the wire) or permanent — drop rather
            // than risk a duplicate.
            Err(e) => {
                let _ = store.drop_record(rec.id, &format!("non-retryable: {e:?}"));
            }
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
pub fn spawn_drain(registry: Arc<ChannelRegistry>, store: Arc<DeliveryStore>) {
    if !store.try_claim_drainer() {
        warn!(
            "delivery queue: a drain task is already running for this store; \
             refusing to spawn a second drainer (claim_due is not lease-atomic)"
        );
        return;
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
            .enqueue("telegram", &msg("hello"), "NotConnected", now)
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
        s.enqueue("signal", &msg("later"), "NotConnected", now + 3600)
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
            .enqueue("telegram", &msg("bye"), "NotConnected", now)
            .unwrap();
        s.mark_delivered(id).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn reschedule_bumps_attempts_and_defers() {
        let s = store();
        let now = now_secs();
        let id = s
            .enqueue("telegram", &msg("retry"), "NotConnected", now)
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
        let first = s.enqueue("ch", &msg("1"), "NotConnected", now).unwrap();
        s.enqueue("ch", &msg("2"), "NotConnected", now).unwrap();
        s.enqueue("ch", &msg("3"), "NotConnected", now).unwrap();
        // Fourth enqueue must evict the oldest (id == first) to stay at the cap.
        s.enqueue("ch", &msg("4"), "NotConnected", now).unwrap();

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
        };
        let rt = bad.to_runtime();
        assert_eq!(rt.max_attempts, 1);
        assert_eq!(rt.initial_backoff, Duration::from_secs(1));
        // max_backoff is raised to at least initial_backoff.
        assert_eq!(rt.max_backoff, Duration::from_secs(1));
        assert!(rt.backoff_factor >= 1.0, "curve must never shrink");
        assert_eq!(rt.tick, Duration::from_secs(1));
        assert_eq!(rt.batch, 1);
        assert_eq!(rt.max_queue_len, 1);
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
        };
        let rt = cfg.to_runtime();
        assert_eq!(rt.max_attempts, 5);
        assert_eq!(rt.initial_backoff, Duration::from_secs(10));
        assert_eq!(rt.max_backoff, Duration::from_secs(600));
        assert_eq!(rt.backoff_factor, 3.0);
        assert_eq!(rt.tick, Duration::from_secs(15));
        assert_eq!(rt.batch, 64);
        assert_eq!(rt.max_queue_len, 50_000);
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
        s.enqueue("telegram", &msg("a"), "NotConnected", now)
            .unwrap();
        s.enqueue("telegram", &msg("b"), "NotConnected", now + 3600)
            .unwrap();
        s.enqueue("signal", &msg("c"), "NotConnected", now).unwrap();

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
            .enqueue("telegram", &msg("lost"), "NotConnected", now)
            .unwrap();
        // Backdate created_at so we can prove the original age is preserved.
        s.guard()
            .execute(
                "UPDATE outbound_deliveries SET created_at = ?1 WHERE id = ?2",
                params![now - 500, id],
            )
            .unwrap();

        s.record_dead_letter(id, 10, "NotConnected(\"down\")")
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
        s.record_dead_letter(999, 5, "gone").unwrap();
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
            let id = s.enqueue("ch", &msg(t), "NotConnected", now).unwrap();
            s.record_dead_letter(id, 10, "exhausted").unwrap();
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
            let id = s.enqueue("telegram", &msg(t), "NotConnected", now).unwrap();
            s.record_dead_letter(id, 10, "exhausted").unwrap();
        }
        assert_eq!(s.stats(now).unwrap().dead_lettered, 2);
        assert!(s.is_empty(), "no live records before redrive");

        // Redrive: both move back, immediately due, fresh budget.
        let moved = s.redrive_dead_letters(now, None).unwrap();
        assert_eq!(moved, 2);
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
            let id = s.enqueue(ch, &msg(t), "NotConnected", now).unwrap();
            s.record_dead_letter(id, 10, "exhausted").unwrap();
        }

        // Only telegram is redriven; signal stays dead-lettered.
        let moved = s.redrive_dead_letters(now, Some("telegram")).unwrap();
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
        assert_eq!(s.redrive_dead_letters(now_secs(), None).unwrap(), 0);
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
            let id = s.enqueue("ch", &msg(t), "NotConnected", now).unwrap();
            s.record_dead_letter(id, 10, "exhausted").unwrap();
        }
        assert_eq!(s.stats(now).unwrap().dead_lettered, 2);

        let moved = s.redrive_dead_letters(now, None).unwrap();
        assert_eq!(moved, 2);
        // Live queue honors its own cap.
        assert!(s.len().unwrap() <= 2, "redrive cannot blow the live bound");
    }
}
