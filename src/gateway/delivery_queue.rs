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
//! [`DeliveryStore`] owns only the SQLite handle — it holds *no* reference to
//! the registry. [`ChannelRegistry`](super::channel_registry::ChannelRegistry)
//! holds `Option<Arc<DeliveryStore>>` and enqueues on transient failure. The
//! drain task ([`drain_loop`] / [`spawn_drain`]) is a free function holding
//! `Arc<ChannelRegistry>` + `Arc<DeliveryStore>`. The store never points back,
//! so there is no `Arc` cycle.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
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

/// A single persisted outbound delivery, rehydrated for a drain attempt.
#[derive(Debug, Clone)]
pub struct DeliveryRecord {
    /// SQLite row id (primary key).
    pub id: i64,
    /// Target channel id (e.g. `telegram`, `signal`).
    pub channel_id: String,
    /// The message to (re)deliver.
    pub message: OutboundMessage,
    /// Number of attempts already made (0 on first enqueue).
    pub attempts: u32,
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
pub fn should_enqueue(err: &ChannelError) -> bool {
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
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
                ON outbound_deliveries(next_attempt_at);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            config,
        })
    }

    /// Access the queue tuning this store was built with.
    pub fn config(&self) -> &DeliveryQueueConfig {
        &self.config
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
            params![attempts as i64, next_attempt_at, last_error, id],
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
        self.len().map(|n| n == 0).unwrap_or(true)
    }
}

/// Drive one pass over all currently-due records: claim, attempt delivery via
/// the enqueue-free [`send_attempt`](ChannelRegistry::send_attempt), then settle
/// each record (delivered → delete, transient → reschedule with backoff,
/// exhausted/ambiguous → drop). Factored out of [`drain_loop`] to keep the loop
/// body readable.
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
                    let _ = store.drop_record(
                        rec.id,
                        &format!("max attempts ({}) exhausted: {:?}", cfg.max_attempts, e),
                    );
                    warn!(
                        id = rec.id,
                        channel = %rec.channel_id,
                        "delivery queue: giving up after exhausting retries"
                    );
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
pub fn spawn_drain(registry: Arc<ChannelRegistry>, store: Arc<DeliveryStore>) {
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
}
