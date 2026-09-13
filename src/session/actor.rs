//! `SessionActor` — one tokio task per session.
//!
//! The actor replays the event log from storage on startup, then serves
//! `ActorCommand`s until its inbox closes or the idle timeout fires. It owns
//! a `broadcast` channel used to fan out newly-appended events to any
//! subscribers (e.g. UI live views).

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{Duration, Instant};

use crate::session::events::{
    batch_durability, now_ms, EventSeq, Retire, SessionEvent, SessionEventRecord,
};
use crate::session::observer::SessionEventObserver;
use crate::session::service::{SessionError, SessionId};
use crate::session::store::SessionEventStore;

/// How long an idle actor survives before self-terminating.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub enum ActorCommand {
    /// The only write command. Append `events` at consecutive seqs and apply
    /// `retire` in ONE store transaction. The reply carries every seq
    /// allocated, in batch order (empty when the batch only retires). A
    /// single event is a batch of one — there is no separate single-event
    /// arm, so one writer cannot drift from another.
    EmitBatch {
        events: Vec<SessionEvent>,
        retire: Option<Retire>,
        reply: oneshot::Sender<Result<Vec<EventSeq>, SessionError>>,
    },
    GetEvents {
        from: Option<EventSeq>,
        to: Option<EventSeq>,
        reply: oneshot::Sender<Result<Vec<SessionEventRecord>, SessionError>>,
    },
    Subscribe {
        reply: oneshot::Sender<broadcast::Receiver<SessionEventRecord>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

pub struct SessionActor {
    pub(crate) id: SessionId,
    pub(crate) store: Arc<dyn SessionEventStore>,
    head_seq: EventSeq,
    inbox: mpsc::Receiver<ActorCommand>,
    broadcaster: broadcast::Sender<SessionEventRecord>,
    observer: Option<Arc<dyn SessionEventObserver>>,
    idle_timeout: Duration,
}

impl SessionActor {
    pub fn new(
        id: SessionId,
        store: Arc<dyn SessionEventStore>,
        inbox: mpsc::Receiver<ActorCommand>,
        broadcaster: broadcast::Sender<SessionEventRecord>,
        observer: Option<Arc<dyn SessionEventObserver>>,
        idle_timeout: Duration,
    ) -> Self {
        Self {
            id,
            store,
            head_seq: 0,
            inbox,
            broadcaster,
            observer,
            idle_timeout,
        }
    }

    /// The one store call behind both emit arms. Seqs are `head_seq + 1 ..`;
    /// on failure (typically a `(session_id, seq)` UNIQUE collision from a
    /// direct-store writer racing the actor — audit 4.1) resync `head_seq`
    /// from the store and retry the WHOLE batch once. Bounded: no loop,
    /// propagate on second failure. `append_batch` is one transaction, so a
    /// failed attempt wrote nothing and the retry cannot double-write.
    ///
    /// Returns the first seq of the batch; the caller derives the rest.
    /// `head_seq` is NOT advanced here — `finish_emitted` does that per row.
    async fn write_batch(
        &mut self,
        events: &[(SessionEvent, i64)],
        retire: Option<Retire>,
    ) -> Result<EventSeq, SessionError> {
        let durability = batch_durability(events.iter().map(|(e, _)| e));
        let mut first = self.head_seq + 1;
        let mut result = self
            .store
            .append_batch(&self.id, first, events, retire, durability)
            .await;
        if result.is_err() {
            if let Ok(stored_head) = self.store.load_head_seq(&self.id).await {
                self.head_seq = stored_head;
                first = stored_head + 1;
                result = self
                    .store
                    .append_batch(&self.id, first, events, retire, durability)
                    .await;
            }
        }
        result.map(|()| first)
    }

    /// `EmitBatch` handler shared by the hot arm and the idle-drain arm.
    /// Returns `true` iff the write landed (the hot arm resets its idle
    /// deadline on that; the drain arm is already exiting).
    ///
    /// An empty batch with nothing to retire is refused here, before the
    /// store: a "batch" that could do nothing must not report success for
    /// nothing. A retire-only batch (no events) is legitimate and replies
    /// with an empty seq list.
    async fn handle_emit_batch(
        &mut self,
        events: Vec<SessionEvent>,
        retire: Option<Retire>,
        reply: oneshot::Sender<Result<Vec<EventSeq>, SessionError>>,
    ) -> bool {
        if events.is_empty() && retire.is_none() {
            let _ = reply.send(Err(SessionError::Other(
                "emit_batch: empty batch with nothing to retire".into(),
            )));
            return false;
        }
        let at = now_ms();
        let pairs: Vec<(SessionEvent, i64)> = events.into_iter().map(|e| (e, at)).collect();
        match self.write_batch(&pairs, retire).await {
            Ok(first) => {
                // Derive each row's seq ONCE; the reply and the per-row
                // finish read the same list (criterion #12).
                let seqs: Vec<EventSeq> = (0..pairs.len() as u64).map(|i| first + i).collect();
                for (seq, (event, at)) in seqs.iter().copied().zip(pairs) {
                    self.finish_emitted(seq, event, at);
                }
                let _ = reply.send(Ok(seqs));
                true
            }
            Err(e) => {
                let _ = reply.send(Err(e));
                false
            }
        }
    }

    /// Common post-append success path, once per appended row, in seq order:
    /// `head_seq`, the observer, the broadcast. The reply is the handler's
    /// job: a batch replies once for all its rows.
    ///
    /// The hot arm was the only site that wrapped `obs.on_appended` in
    /// `catch_unwind`; the drain arm silently skipped observer + broadcast,
    /// which left `MessageProjector` (and any other observer-side consumer)
    /// out of sync with `session_events` for any event that landed during
    /// the brief window between the idle sleep firing and `run()` returning.
    /// Funnelling both arms through this helper closes that gap.
    fn finish_emitted(&mut self, seq: EventSeq, event: SessionEvent, at: i64) {
        self.head_seq = seq;
        let record = SessionEventRecord {
            seq,
            event,
            created_at_ms: at,
        };
        // Observer notification must not be allowed
        // to kill the actor: a panicking observer
        // used to strand the broadcast and the
        // caller's reply, with the event durable
        // in storage but neither subscriber nor
        // caller told. `catch_unwind` is sound
        // here because the closure only takes
        // `&self.id` and `&record`, both
        // `RefUnwindSafe`.
        if let Some(obs) = &self.observer {
            let id = &self.id;
            let record_ref = &record;
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                obs.on_appended(id, record_ref);
            }))
            .is_err()
            {
                tracing::error!(
                    id = ?self.id,
                    seq,
                    "SessionActor observer panicked; \
                     continuing (event already durable)"
                );
            }
        }
        // Broadcast send. A `SendError` here means
        // *zero receivers* — tokio's
        // `broadcast::Sender::send` ONLY fails when
        // `rx_cnt == 0`; a full
        // `BROADCAST_BUFFER` (=256) does NOT error
        // and instead lets the lagging receiver
        // observe `RecvError::Lagged` on its next
        // `recv()` (which is exactly where it can
        // tell — the producer side cannot, which is
        // why there is no separate "lagger" warn
        // here). The previous `&& receiver_count >
        // 0` guard was dead because `Err` ⇔ `rx_cnt
        // == 0`; an audit caught it.
        if let Err(_record) = self.broadcaster.send(record) {
            tracing::debug!(
                id = ?self.id,
                seq,
                "SessionActor broadcast had no receivers; \
                 event is durable in the SSOT log only"
            );
        }
    }

    /// Rebuilds `head_seq` from the store's own allocation counter.
    ///
    /// `load_head_seq` never decodes a payload, so the actor still boots on a
    /// session whose log holds a row this build cannot read — which is what
    /// lets the doctor's retire reach that log at all. It also counts retired
    /// rows, as the seq allocator must (a retired seq is still taken).
    async fn replay(&mut self) -> Result<(), SessionError> {
        self.head_seq = self.store.load_head_seq(&self.id).await?;
        Ok(())
    }

    pub async fn run(mut self) {
        if let Err(e) = self.replay().await {
            tracing::error!(?e, "SessionActor replay failed; actor terminating");
            return;
        }

        let mut idle_deadline = Instant::now() + self.idle_timeout;
        loop {
            tokio::select! {
                biased;
                cmd = self.inbox.recv() => match cmd {
                    // Reset the idle deadline after a successful write: a
                    // session whose only traffic is emits (e.g. a
                    // long-running harness writer with no concurrent
                    // readers) must not be reaped while events are still
                    // flowing. Without this, the deadline set once at the
                    // top of `run` would fire `idle_timeout` after `run`
                    // started, regardless of how many emits happened in
                    // between — see severed-wire-2026-09-05-modules2
                    // session I-1.
                    Some(ActorCommand::EmitBatch { events, retire, reply }) => {
                        if self.handle_emit_batch(events, retire, reply).await {
                            idle_deadline = Instant::now() + self.idle_timeout;
                        }
                    }
                    Some(ActorCommand::GetEvents { from, to, reply }) => {
                        let result = self.store.load_events_range(&self.id, from, to).await;
                        let _ = reply.send(result);
                        idle_deadline = Instant::now() + self.idle_timeout;
                    }
                    Some(ActorCommand::Subscribe { reply }) => {
                        let _ = reply.send(self.broadcaster.subscribe());
                        idle_deadline = Instant::now() + self.idle_timeout;
                    }
                    Some(ActorCommand::Shutdown { reply }) => {
                        let _ = reply.send(());
                        return;
                    }
                    None => {
                        // All senders dropped; exit cleanly.
                        return;
                    }
                },
                _ = tokio::time::sleep_until(idle_deadline) => {
                    // Drain any commands already buffered in the inbox
                    // before exiting. Without this, a command that lands
                    // between the sleep returning and `run()` returning
                    // (a tiny but real window — the mpsc buffer is 64)
                    // has its `oneshot::Sender` dropped with `run()`,
                    // surfacing as `SessionError::ActorShutdown` to the
                    // caller with no signal that the event was never
                    // appended. Drain until the inbox is empty or the
                    // next recv would block.
                    let mut drained = 0u32;
                    while let Ok(cmd) = self.inbox.try_recv() {
                        match cmd {
                            // Same handler as the hot arm: the drain arm
                            // races the hot arm and any direct-store writer
                            // just like the hot arm does, so it needs the
                            // same self-heal — and sharing the handler is
                            // what keeps it from being forgotten here.
                            ActorCommand::EmitBatch { events, retire, reply } => {
                                self.handle_emit_batch(events, retire, reply).await;
                            }
                            ActorCommand::GetEvents { from, to, reply } => {
                                let result =
                                    self.store.load_events_range(&self.id, from, to).await;
                                let _ = reply.send(result);
                            }
                            ActorCommand::Subscribe { reply } => {
                                let _ = reply.send(self.broadcaster.subscribe());
                            }
                            ActorCommand::Shutdown { reply } => {
                                let _ = reply.send(());
                                if drained > 0 {
                                    tracing::debug!(
                                        id = ?self.id,
                                        drained,
                                        "SessionActor idle-timeout drained buffered commands",
                                    );
                                }
                                return;
                            }
                        }
                        drained = drained.saturating_add(1);
                    }
                    if drained > 0 {
                        tracing::debug!(
                            id = ?self.id,
                            drained,
                            "SessionActor idle-timeout drained buffered commands",
                        );
                    }
                    tracing::debug!(id = ?self.id, "SessionActor idle timeout — detaching");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use super::*;
    use crate::session::events::{Durability, MessageContent, TurnTrigger};
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};

    async fn test_store() -> Arc<dyn SessionEventStore> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        Arc::new(SqliteEventStore::new(conn))
    }

    fn sample_id() -> SessionId {
        crate::routing::session_key::SessionKey::ephemeral("actor-test")
    }

    #[tokio::test]
    async fn emit_then_get_returns_same_event() {
        let store = test_store().await;
        let id = sample_id();
        let (tx, rx) = mpsc::channel(8);
        let (bcast, _) = broadcast::channel(16);
        let actor = SessionActor::new(id.clone(), store, rx, bcast, None, DEFAULT_IDLE_TIMEOUT);
        let handle = tokio::spawn(actor.run());

        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitBatch {
            events: vec![SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            }],
            retire: None,
            reply: rtx,
        })
        .await
        .unwrap();
        let seqs = rrx.await.unwrap().unwrap();
        assert_eq!(seqs, vec![1]);

        let (gtx, grx) = oneshot::channel();
        tx.send(ActorCommand::GetEvents {
            from: None,
            to: None,
            reply: gtx,
        })
        .await
        .unwrap();
        let events = grx.await.unwrap().unwrap();
        assert_eq!(events.len(), 1);

        let (stx, srx) = oneshot::channel();
        tx.send(ActorCommand::Shutdown { reply: stx })
            .await
            .unwrap();
        srx.await.unwrap();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn subscribe_receives_subsequent_events() {
        let store = test_store().await;
        let id = sample_id();
        let (tx, rx) = mpsc::channel(8);
        let (bcast, _) = broadcast::channel(16);
        let actor = SessionActor::new(id.clone(), store, rx, bcast, None, DEFAULT_IDLE_TIMEOUT);
        tokio::spawn(actor.run());

        let (stx, srx) = oneshot::channel();
        tx.send(ActorCommand::Subscribe { reply: stx })
            .await
            .unwrap();
        let mut sub = srx.await.unwrap();

        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitBatch {
            events: vec![SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: "hi".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
                author_user_id: None,
            }],
            retire: None,
            reply: rtx,
        })
        .await
        .unwrap();
        assert_eq!(rrx.await.unwrap().unwrap().len(), 1);

        let record = sub.recv().await.unwrap();
        assert!(matches!(record.event, SessionEvent::UserMessage { .. }));
    }

    #[tokio::test]
    async fn replay_rebuilds_head_seq() {
        let store = test_store().await;
        let id = sample_id();
        let at = now_ms();
        // Seed 3 events directly in the store.
        for seq in 1..=3 {
            store
                .append(
                    &id,
                    seq,
                    &SessionEvent::TurnStarted {
                        turn_id: uuid::Uuid::new_v4(),
                        trigger: TurnTrigger::UserMessage,
                        at,
                    },
                    at,
                )
                .await
                .unwrap();
        }

        let (tx, rx) = mpsc::channel(8);
        let (bcast, _) = broadcast::channel(16);
        let actor = SessionActor::new(id.clone(), store, rx, bcast, None, DEFAULT_IDLE_TIMEOUT);
        tokio::spawn(actor.run());

        // Emit one more event; it should land at seq=4
        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitBatch {
            events: vec![SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
                trigger: TurnTrigger::UserMessage,
                at,
            }],
            retire: None,
            reply: rtx,
        })
        .await
        .unwrap();
        let seqs = rrx.await.unwrap().unwrap();
        assert_eq!(seqs, vec![4]);
    }

    /// The point of `replay` reading the seq counter and not the log: an
    /// actor still BOOTS on a session whose log holds a row this build cannot
    /// read, so the doctor's retire and a new append can reach that log. A
    /// boot that decoded the log would terminate the actor instead, and every
    /// command's reply channel would drop. Pinned at the effect: an `Emit`
    /// lands PAST the bad row (the counter saw it), and `GetEvents` answers
    /// the store's own refusal — `Err(UndecodableRecord)`, not
    /// `ActorShutdown`.
    #[tokio::test]
    async fn the_actor_boots_on_a_log_with_a_row_it_cannot_read() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store = Arc::new(SqliteEventStore::new(conn));
        let id = sample_id();
        let at = now_ms();
        store
            .append(
                &id,
                1,
                &SessionEvent::TurnStarted {
                    turn_id: uuid::Uuid::new_v4(),
                    trigger: TurnTrigger::UserMessage,
                    at,
                },
                at,
            )
            .await
            .unwrap();
        store
            .insert_raw_row_for_test(&id, 2, "from_the_future", r#"{"type":"from_the_future"}"#)
            .await;

        let (tx, rx) = mpsc::channel(8);
        let (bcast, _) = broadcast::channel(16);
        let actor = SessionActor::new(
            id.clone(),
            store as Arc<dyn SessionEventStore>,
            rx,
            bcast,
            None,
            DEFAULT_IDLE_TIMEOUT,
        );
        tokio::spawn(actor.run());

        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitBatch {
            events: vec![SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
                trigger: TurnTrigger::UserMessage,
                at,
            }],
            retire: None,
            reply: rtx,
        })
        .await
        .unwrap();
        let seqs = rrx
            .await
            .expect("the actor is alive: its reply channel was not dropped")
            .unwrap();
        assert_eq!(seqs, vec![3], "the append lands past the bad row");

        let (gtx, grx) = oneshot::channel();
        tx.send(ActorCommand::GetEvents {
            from: None,
            to: None,
            reply: gtx,
        })
        .await
        .unwrap();
        let err = grx.await.unwrap().unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::UndecodableRecord(crate::session::store::UndecodableRecord {
                    seq: 2,
                    ..
                })
            ),
            "the read is the store's refusal, naming the row: {err}"
        );
    }

    /// A store double for the actor's write path. `append_batch` records every
    /// call as `(first_seq, rows)`, fails the first `fail_first` calls with a
    /// numbered `Storage` error — each failure also advances `head` by one,
    /// the way a direct-store writer landing the contested seq would — and
    /// succeeds after that, advancing `head` by the batch's row count. Every
    /// read returns empty. Tests assert on the recording, not inside the
    /// double, so a wrong call shape fails the test instead of panicking the
    /// actor task.
    struct ScriptedStore {
        calls: std::sync::Mutex<Vec<(EventSeq, usize)>>,
        head: AtomicU64,
        fail_first: usize,
    }

    impl ScriptedStore {
        fn new(fail_first: usize) -> Arc<Self> {
            Arc::new(Self {
                calls: std::sync::Mutex::new(vec![]),
                head: AtomicU64::new(0),
                fail_first,
            })
        }

        /// `(first_seq, rows)` per `append_batch` call, in call order.
        fn calls(&self) -> Vec<(EventSeq, usize)> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    #[async_trait::async_trait]
    impl SessionEventStore for ScriptedStore {
        async fn append_batch(
            &self,
            _id: &SessionId,
            first_seq: EventSeq,
            events: &[(SessionEvent, i64)],
            _retire: Option<Retire>,
            _durability: Durability,
        ) -> Result<(), SessionError> {
            let n = {
                let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
                calls.push((first_seq, events.len()));
                calls.len()
            };
            if n <= self.fail_first {
                self.head.fetch_add(1, Ordering::SeqCst);
                return Err(SessionError::Storage(format!("scripted failure #{n}")));
            }
            self.head.fetch_add(events.len() as u64, Ordering::SeqCst);
            Ok(())
        }

        async fn load_all_events(
            &self,
            _id: &SessionId,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            Ok(vec![])
        }

        async fn load_events_range(
            &self,
            _id: &SessionId,
            _from: Option<EventSeq>,
            _to: Option<EventSeq>,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            Ok(vec![])
        }

        async fn load_head_seq(&self, _id: &SessionId) -> Result<EventSeq, SessionError> {
            Ok(self.head.load(Ordering::SeqCst))
        }

        async fn load_run_markers(
            &self,
        ) -> Result<Vec<(SessionId, crate::session::store::MarkerSlice)>, SessionError> {
            Ok(vec![])
        }

        async fn retire_from(
            &self,
            _id: &SessionId,
            _from: EventSeq,
        ) -> Result<usize, SessionError> {
            Ok(0)
        }
    }

    /// Spawn an actor over `store` with an observer that counts `on_appended`
    /// calls and one broadcast subscriber, so a test can assert that a failed
    /// write fired NEITHER.
    fn spawn_scripted(
        store: &Arc<ScriptedStore>,
    ) -> (
        mpsc::Sender<ActorCommand>,
        Arc<AtomicUsize>,
        broadcast::Receiver<SessionEventRecord>,
    ) {
        struct Counter(Arc<AtomicUsize>);
        impl SessionEventObserver for Counter {
            fn on_appended(&self, _id: &SessionId, _rec: &SessionEventRecord) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let observed = Arc::new(AtomicUsize::new(0));
        let dyn_store: Arc<dyn SessionEventStore> = store.clone();
        let (tx, rx) = mpsc::channel(8);
        let (bcast, sub) = broadcast::channel(16);
        let actor = SessionActor::new(
            sample_id(),
            dyn_store,
            rx,
            bcast,
            Some(Arc::new(Counter(observed.clone()))),
            DEFAULT_IDLE_TIMEOUT,
        );
        tokio::spawn(actor.run());
        (tx, observed, sub)
    }

    fn two_turn_starts() -> Vec<SessionEvent> {
        let turn_id = uuid::Uuid::new_v4();
        (0..2)
            .map(|_| SessionEvent::TurnStarted {
                turn_id,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            })
            .collect()
    }

    /// Regression test for audit 4.1: a direct-store writer racing the actor
    /// can take the seq the actor was about to use, causing a `(session_id,
    /// seq)` UNIQUE collision on the store write. Before the fix, the `Err`
    /// arm just replied `Err` without resyncing `head_seq`, so the actor would
    /// recompute the same colliding seq on every subsequent emit — permanently
    /// wedging that session's writes. The fix resyncs `head_seq` from the
    /// store and retries once: the first `append_batch` (first_seq=1) fails
    /// and the store's head reads 1, so the retry goes out at first_seq=2.
    ///
    /// Sent as an `EmitBatch` of TWO events so the recording also pins that
    /// the retry re-sends the WHOLE batch (the failed transaction wrote
    /// nothing): both calls carry two rows, and the reply is `[2, 3]`.
    #[tokio::test]
    async fn actor_self_heals_seq_after_append_collision() {
        let store = ScriptedStore::new(1);
        let (tx, observed, _sub) = spawn_scripted(&store);

        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitBatch {
            events: two_turn_starts(),
            retire: None,
            reply: rtx,
        })
        .await
        .unwrap();

        let seqs = rrx.await.unwrap().unwrap();
        assert_eq!(
            seqs,
            vec![2, 3],
            "actor should self-heal past the seq=1 collision and land the whole batch at [2, 3]"
        );
        assert_eq!(
            store.calls(),
            vec![(1, 2), (2, 2)],
            "one collision, one retry, the whole batch both times"
        );
        assert_eq!(
            observed.load(Ordering::SeqCst),
            2,
            "the observer fires once per landed row, never for the failed attempt"
        );
    }

    /// The self-heal is bounded: when the retry ALSO fails, the caller gets
    /// that second attempt's error verbatim (not the first's, not a generic
    /// one), the store saw exactly two attempts, and — because nothing
    /// landed — neither the observer nor the broadcast fired.
    #[tokio::test]
    async fn a_second_failure_is_the_callers_error_and_nothing_fires() {
        let store = ScriptedStore::new(2);
        let (tx, observed, mut sub) = spawn_scripted(&store);

        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitBatch {
            events: two_turn_starts(),
            retire: None,
            reply: rtx,
        })
        .await
        .unwrap();

        match rrx.await.unwrap() {
            Err(SessionError::Storage(msg)) => assert_eq!(msg, "scripted failure #2"),
            other => panic!("expected the second attempt's Storage error, got {other:?}"),
        }
        assert_eq!(
            store.calls(),
            vec![(1, 2), (2, 2)],
            "exactly two attempts, no loop"
        );
        assert_eq!(observed.load(Ordering::SeqCst), 0);
        assert!(
            matches!(sub.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "a failed write must not be broadcast"
        );
    }

    /// The actor refuses an empty batch with nothing to retire BEFORE the
    /// store: the store's recording stays empty. (The real store refuses the
    /// same predicate with the same variant, so a test over `SqliteEventStore`
    /// could not tell the two apart — this double can.)
    #[tokio::test]
    async fn an_empty_batch_is_refused_before_the_store() {
        let store = ScriptedStore::new(0);
        let (tx, observed, _sub) = spawn_scripted(&store);

        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitBatch {
            events: vec![],
            retire: None,
            reply: rtx,
        })
        .await
        .unwrap();

        assert!(matches!(rrx.await.unwrap(), Err(SessionError::Other(_))));
        assert!(store.calls().is_empty(), "the store must not be reached");
        assert_eq!(observed.load(Ordering::SeqCst), 0);
    }
}
