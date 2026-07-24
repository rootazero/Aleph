//! Per-(from, to-set) message aggregation — `ClawTeam` `RuntimeRouter` parity.
//!
//! Wraps a [`MessageRouter`] with a small in-memory state machine that
//! coalesces same-key messages arriving within a time window into a single
//! delivery. This is opt-in: callers pick `send_batched()` instead of the
//! inner `send()` when they're emitting potentially-chatty status updates
//! (e.g. agent progress pings). Question/Approval traffic should NEVER be
//! batched — those are routed verbatim through `send()`.
//!
//! State is purely in-memory: process restart drops any pending window.
//! That's intentional — these are best-effort progress pings, not durable
//! commitments. The auth journal lives in `MessageStore` regardless of
//! how the message reached it.
//!
//! Two flush triggers:
//! 1. Window expiry — a timer task fires after `window_ms` from the first
//!    queued message in a key, drains the bucket, and dispatches one
//!    combined message.
//! 2. Hard cap — when `max_batch` is reached, flush immediately so a
//!    runaway loop can't pin unbounded memory.
//!
//! There is deliberately no external force-flush surface: a disband only
//! races the 500 ms window, and anything still pending is a best-effort
//! progress ping for a team that no longer exists.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::warn;

use super::router::{MessageRouter, SendRequest};
use super::types::MessageType;
use crate::sync_primitives::Arc;

// =============================================================================
// Config + key types
// =============================================================================

/// Throttle knobs. Defaults come from `ClawTeam`'s `RuntimeRouter`: 500 ms window,
/// hard cap at 50 messages per batch.
#[derive(Debug, Clone)]
pub struct AggregatorConfig {
    /// How long a bucket stays open after its first message lands.
    pub window: Duration,
    /// Safety cap — flush immediately once a bucket reaches this many entries.
    pub max_batch: usize,
    /// Master switch. When false, `send_batched` is a pure passthrough.
    pub enabled: bool,
}

impl Default for AggregatorConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_millis(500),
            max_batch: 50,
            enabled: true,
        }
    }
}

/// Stable identity for batching. Two requests with the same key are merged.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct BatchKey {
    team_id: String,
    from_agent: String,
    to_sorted: Vec<String>,
    cc_sorted: Vec<String>,
    /// Stringified `MessageType` — avoids hand-rolling Hash on the Custom(String) variant.
    msg_type: String,
    subject: String,
}

impl BatchKey {
    fn from_request(req: &SendRequest) -> Self {
        let mut to = req.to.clone();
        to.sort();
        to.dedup();
        let mut cc = req.cc.clone();
        cc.sort();
        cc.dedup();
        Self {
            team_id: req.team_id.clone(),
            from_agent: req.from_agent.clone(),
            to_sorted: to,
            cc_sorted: cc,
            msg_type: msg_type_key(&req.msg_type),
            subject: req.subject.clone(),
        }
    }
}

fn msg_type_key(t: &MessageType) -> String {
    match t {
        MessageType::Message => "message".into(),
        MessageType::SystemNotification => "system_notification".into(),
        MessageType::Idle => "idle".into(),
        MessageType::PlanApprovalRequest => "plan_approval_request".into(),
        MessageType::PlanApproved => "plan_approved".into(),
        MessageType::PlanRejected => "plan_rejected".into(),
        MessageType::ShutdownRequest => "shutdown_request".into(),
        MessageType::ShutdownApproved => "shutdown_approved".into(),
        MessageType::ShutdownRejected => "shutdown_rejected".into(),
        MessageType::Custom(s) => format!("custom:{s}"),
    }
}

/// Message types that the aggregator will batch. Anything decision-loaded
/// (approvals, shutdowns) bypasses — those need to land synchronously and
/// individually so their replies can be threaded.
const fn is_batchable(t: &MessageType) -> bool {
    matches!(
        t,
        MessageType::Message | MessageType::SystemNotification | MessageType::Idle
    )
}

// =============================================================================
// Bucket state
// =============================================================================

struct Bucket {
    pending: Vec<SendRequest>,
    /// Flag set when a flush task is already running for this bucket. New
    /// `send_batched` calls append to `pending` and rely on that task to
    /// drain.
    timer_running: bool,
}

impl Bucket {
    const fn new() -> Self {
        Self {
            pending: Vec::new(),
            timer_running: false,
        }
    }
}

// =============================================================================
// Aggregator
// =============================================================================

/// Aggregates same-key messages within a time window into a single delivery.
pub struct Aggregator {
    inner: Arc<MessageRouter>,
    config: AggregatorConfig,
    buckets: Arc<Mutex<HashMap<BatchKey, Bucket>>>,
}

impl Aggregator {
    #[must_use]
    pub fn new(inner: Arc<MessageRouter>, config: AggregatorConfig) -> Arc<Self> {
        Arc::new(Self {
            inner,
            config,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Direct passthrough for code paths that shouldn't be aggregated.
    #[must_use]
    pub fn router(&self) -> &MessageRouter {
        &self.inner
    }

    /// Opt-in batched send. Returns immediately; errors during the actual
    /// flush are logged but not surfaced (the caller has no thread to receive
    /// them by the time the window closes). For synchronous semantics, use
    /// `self.router().send(...)` directly.
    pub async fn send_batched(self: &Arc<Self>, req: SendRequest) {
        if !self.config.enabled || !is_batchable(&req.msg_type) {
            // Hot-path: bypass entirely so the disabled overhead is one bool
            // check + one async branch.
            if let Err(e) = self.inner.send(req).await {
                warn!(error = %e, "aggregator: passthrough send failed");
            }
            return;
        }

        let key = BatchKey::from_request(&req);
        let need_timer;
        let force_flush;
        {
            let mut buckets = self.buckets.lock().await;
            let bucket = buckets.entry(key.clone()).or_insert_with(Bucket::new);
            bucket.pending.push(req);
            need_timer = !bucket.timer_running;
            force_flush = bucket.pending.len() >= self.config.max_batch;
            if need_timer {
                bucket.timer_running = true;
            }
        }

        if need_timer {
            self.spawn_flush(key.clone());
        }
        if force_flush {
            // Drain right now; the timer task (if any) will find an empty
            // bucket and noop.
            self.flush_one(&key).await;
        }
    }

    /// Drain a bucket and dispatch one combined message. Idempotent — an
    /// already-empty bucket is a noop.
    async fn flush_one(self: &Arc<Self>, key: &BatchKey) {
        let drained = {
            let mut buckets = self.buckets.lock().await;
            match buckets.remove(key) {
                Some(b) if !b.pending.is_empty() => b.pending,
                _ => return,
            }
        };

        let merged = match merge_requests(drained) {
            Some(req) => req,
            None => return,
        };
        if let Err(e) = self.inner.send(merged).await {
            warn!(error = %e, team_id = %key.team_id, from = %key.from_agent, "aggregator: batched send failed");
        }
    }

    fn spawn_flush(self: &Arc<Self>, key: BatchKey) {
        let me = self.clone();
        let window = self.config.window;
        tokio::spawn(async move {
            tokio::time::sleep(window).await;
            me.flush_one(&key).await;
        });
    }
}

/// Combine N same-key requests into one. Content lines are joined with a
/// blank-line separator so the receiver sees a clear delimiter between
/// batched chunks. Attachments + `reply_to` are unioned across the batch
/// (dedup-aware so identical re-sends collapse cleanly).
fn merge_requests(reqs: Vec<SendRequest>) -> Option<SendRequest> {
    let mut iter = reqs.into_iter();
    let mut head = iter.next()?;
    if iter.len() == 0 {
        // Single message: no aggregation needed. Return as-is.
        return Some(head);
    }

    let mut bodies: Vec<String> = Vec::new();
    bodies.push(std::mem::take(&mut head.content));
    let mut attachments: Vec<String> = std::mem::take(&mut head.attachments);

    for r in iter {
        if !r.content.is_empty() {
            bodies.push(r.content);
        }
        for a in r.attachments {
            if !attachments.contains(&a) {
                attachments.push(a);
            }
        }
        // reply_to: keep the head's choice; if head had none, adopt the first
        // non-None we encounter. (Threading should be consistent within a
        // batch — they share subject + recipients.)
        if head.reply_to.is_none() && r.reply_to.is_some() {
            head.reply_to = r.reply_to;
        }
    }

    head.content = bodies.join("\n\n");
    head.attachments = attachments;
    Some(head)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::router::EscalationRule;
    use crate::teams::messages::store::SqliteMessageStore;
    use crate::teams::messages::types::{MessageType, NewMessage, Recipient, RecipientRole};
    use rusqlite::Connection;

    async fn setup() -> (
        Arc<MessageRouter>,
        Arc<dyn crate::teams::messages::MessageStore>,
    ) {
        let msg_conn = Connection::open_in_memory().unwrap();
        let msg_store = Arc::new(SqliteMessageStore::new(msg_conn));
        msg_store.migrate().await.unwrap();
        let trait_store: Arc<dyn crate::teams::messages::MessageStore> = msg_store.clone();

        let evt_conn = Connection::open_in_memory().unwrap();
        let evt_store = Arc::new(SqliteEventLogStore::new(evt_conn));
        evt_store.migrate().await.unwrap();

        let router = Arc::new(MessageRouter::new(
            trait_store.clone(),
            evt_store as Arc<dyn crate::teams::events::EventLogStore>,
            EscalationRule {
                thread_message_threshold: 100, // disable escalation in tests
                enabled: false,
            },
            None,
        ));
        (router, trait_store)
    }

    fn req(team: &str, from: &str, to: &str, content: &str) -> SendRequest {
        SendRequest {
            team_id: team.into(),
            from_agent: from.into(),
            to: vec![to.into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "status".into(),
            content: content.into(),
            reply_to: None,
            attachments: vec![],
        }
    }

    /// Count messages that landed in `agent_id`'s inbox under `team_id`.
    /// We track the recipient inbox because the trait exposes no team-wide
    /// listing — the inbox is the closest legitimate side-effect probe.
    async fn inbox_count(
        store: &Arc<dyn crate::teams::messages::MessageStore>,
        agent_id: &str,
        team_id: &str,
    ) -> usize {
        store
            .read_inbox(agent_id, team_id, None)
            .await
            .map(|v| v.len())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn disabled_aggregator_is_passthrough() {
        let (router, store) = setup().await;
        let agg = Aggregator::new(
            router,
            AggregatorConfig {
                window: Duration::from_secs(60),
                max_batch: 10,
                enabled: false,
            },
        );
        agg.send_batched(req("t1", "a", "b", "hello")).await;
        // Disabled → fired through immediately, no batching.
        assert_eq!(inbox_count(&store, "b", "t1").await, 1);
    }

    #[tokio::test]
    async fn unbatchable_msg_types_bypass_window() {
        let (router, store) = setup().await;
        let agg = Aggregator::new(
            router,
            AggregatorConfig {
                window: Duration::from_secs(60),
                ..Default::default()
            },
        );
        // PlanApprovalRequest must not be batched — even with a giant window
        // it should fire instantly.
        let mut r = req("t1", "a", "b", "please review");
        r.msg_type = MessageType::PlanApprovalRequest;
        agg.send_batched(r).await;
        assert_eq!(inbox_count(&store, "b", "t1").await, 1);
    }

    #[tokio::test]
    async fn window_aggregates_same_key_into_one_delivery() {
        let (router, store) = setup().await;
        let agg = Aggregator::new(
            router,
            AggregatorConfig {
                window: Duration::from_millis(120),
                max_batch: 100,
                enabled: true,
            },
        );
        agg.send_batched(req("t1", "a", "b", "first")).await;
        agg.send_batched(req("t1", "a", "b", "second")).await;
        agg.send_batched(req("t1", "a", "b", "third")).await;
        // Before window expiry: nothing dispatched yet.
        assert_eq!(inbox_count(&store, "b", "t1").await, 0);
        // Wait past window.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let msgs = store.read_inbox("b", "t1", None).await.unwrap();
        assert_eq!(msgs.len(), 1, "three sends → one merged delivery");
        assert!(msgs[0].content.contains("first"));
        assert!(msgs[0].content.contains("second"));
        assert!(msgs[0].content.contains("third"));
    }

    #[tokio::test]
    async fn different_keys_stay_isolated() {
        let (router, store) = setup().await;
        let agg = Aggregator::new(
            router,
            AggregatorConfig {
                window: Duration::from_millis(120),
                ..Default::default()
            },
        );
        // Two distinct from-agents = two buckets.
        agg.send_batched(req("t1", "a", "x", "from-a")).await;
        agg.send_batched(req("t1", "b", "x", "from-b")).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        // Both messages target agent "x" — its inbox should now hold 2,
        // one per from-agent bucket.
        let msgs = store.read_inbox("x", "t1", None).await.unwrap();
        assert_eq!(msgs.len(), 2, "different from agents = different buckets");
    }

    #[tokio::test]
    async fn max_batch_forces_immediate_flush() {
        let (router, store) = setup().await;
        let agg = Aggregator::new(
            router,
            AggregatorConfig {
                window: Duration::from_secs(60),
                max_batch: 3, // tight cap
                enabled: true,
            },
        );
        agg.send_batched(req("t1", "a", "b", "m1")).await;
        agg.send_batched(req("t1", "a", "b", "m2")).await;
        agg.send_batched(req("t1", "a", "b", "m3")).await;
        // Cap reached on the 3rd — flushes synchronously before this await
        // returns. Don't depend on the still-pending timer.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(inbox_count(&store, "b", "t1").await, 1);
    }

    #[test]
    fn merge_dedups_attachments() {
        let r1 = SendRequest {
            team_id: "t".into(),
            from_agent: "a".into(),
            to: vec!["b".into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "s".into(),
            content: "one".into(),
            reply_to: None,
            attachments: vec!["x".into(), "y".into()],
        };
        let r2 = SendRequest {
            content: "two".into(),
            attachments: vec!["y".into(), "z".into()],
            ..r1.clone()
        };
        let merged = merge_requests(vec![r1, r2]).unwrap();
        assert_eq!(merged.content, "one\n\ntwo");
        assert_eq!(merged.attachments, vec!["x", "y", "z"]);
    }

    #[test]
    fn merge_single_request_is_identity() {
        let r1 = SendRequest {
            team_id: "t".into(),
            from_agent: "a".into(),
            to: vec!["b".into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "s".into(),
            content: "only".into(),
            reply_to: None,
            attachments: vec!["x".into()],
        };
        let merged = merge_requests(vec![r1]).unwrap();
        assert_eq!(merged.content, "only");
        assert_eq!(merged.attachments, vec!["x"]);
    }

    #[test]
    fn batch_key_normalizes_recipient_order() {
        let r1 = SendRequest {
            team_id: "t".into(),
            from_agent: "a".into(),
            to: vec!["x".into(), "y".into()],
            cc: vec!["m".into(), "n".into()],
            msg_type: MessageType::Message,
            subject: "s".into(),
            content: String::new(),
            reply_to: None,
            attachments: vec![],
        };
        let r2 = SendRequest {
            to: vec!["y".into(), "x".into()],
            cc: vec!["n".into(), "m".into()],
            ..r1.clone()
        };
        assert_eq!(BatchKey::from_request(&r1), BatchKey::from_request(&r2));
    }

    // Dummy use to keep Recipient import alive in case future tests need it.
    #[allow(dead_code)]
    fn _types_used(_r: Recipient, _n: NewMessage) {
        let _ = RecipientRole::To;
    }
}
