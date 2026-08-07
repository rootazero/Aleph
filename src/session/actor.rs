//! `SessionActor` — one tokio task per session.
//!
//! The actor replays the event log from storage on startup, then serves
//! `ActorCommand`s until its inbox closes or the idle timeout fires. It owns
//! a `broadcast` channel used to fan out newly-appended events to any
//! subscribers (e.g. UI live views).

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{Duration, Instant};

use crate::session::events::{now_ms, EventSeq, SessionEvent, SessionEventRecord};
use crate::session::observer::SessionEventObserver;
use crate::session::service::{SessionError, SessionId};
use crate::session::store::SessionEventStore;

/// How long an idle actor survives before self-terminating.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub enum ActorCommand {
    EmitEvent {
        event: SessionEvent,
        reply: oneshot::Sender<Result<EventSeq, SessionError>>,
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

    /// Replays all persisted events and rebuilds `head_seq`.
    async fn replay(&mut self) -> Result<(), SessionError> {
        let records = self.store.load_all_events(&self.id).await?;
        for record in &records {
            self.head_seq = record.seq;
        }
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
                    Some(ActorCommand::EmitEvent { event, reply }) => {
                        let mut seq = self.head_seq + 1;
                        let at = now_ms();
                        let mut append_result = self.store.append(&self.id, seq, &event, at).await;

                        // Self-heal: an append failure (typically a `(session_id, seq)`
                        // UNIQUE collision from a direct-store writer racing the actor —
                        // audit 4.1) must not permanently wedge this session's writes.
                        // Resync `head_seq` from the store and retry ONCE. Bounded: no
                        // loop, propagate on second failure. `append` is a single atomic
                        // INSERT, so a failed attempt wrote nothing and the retry cannot
                        // double-write.
                        if append_result.is_err() {
                            if let Ok(stored_head) = self.store.load_head_seq(&self.id).await {
                                self.head_seq = stored_head;
                                seq = self.head_seq + 1;
                                append_result =
                                    self.store.append(&self.id, seq, &event, at).await;
                            }
                        }

                        match append_result {
                            Ok(()) => {
                                let record = SessionEventRecord {
                                    seq,
                                    event,
                                    created_at_ms: at,
                                };
                                self.head_seq = seq;
                                if let Some(obs) = &self.observer {
                                    obs.on_appended(&self.id, &record);
                                }
                                let _ = self.broadcaster.send(record);
                                let _ = reply.send(Ok(seq));
                                idle_deadline = Instant::now() + self.idle_timeout;
                            }
                            Err(e) => {
                                let _ = reply.send(Err(e));
                            }
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
                    tracing::debug!(id = ?self.id, "SessionActor idle timeout — detaching");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{MessageContent, TurnTrigger};
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
        tx.send(ActorCommand::EmitEvent {
            event: SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
            reply: rtx,
        })
        .await
        .unwrap();
        let seq = rrx.await.unwrap().unwrap();
        assert_eq!(seq, 1);

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

        let (rtx, _rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitEvent {
            event: SessionEvent::UserMessage {
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
            },
            reply: rtx,
        })
        .await
        .unwrap();

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
        tx.send(ActorCommand::EmitEvent {
            event: SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
                trigger: TurnTrigger::UserMessage,
                at,
            },
            reply: rtx,
        })
        .await
        .unwrap();
        let seq = rrx.await.unwrap().unwrap();
        assert_eq!(seq, 4);
    }

    /// Regression test for audit 4.1: a direct-store writer racing the actor
    /// can take the seq the actor was about to use, causing a `(session_id,
    /// seq)` UNIQUE collision on `append`. Before the fix, the `Err` arm just
    /// replied `Err` without resyncing `head_seq`, so the actor would recompute
    /// the same colliding seq on every subsequent emit — permanently wedging
    /// that session's writes. The fix resyncs `head_seq` from the store and
    /// retries once. This controlled store makes the first `append` (seq=1)
    /// collide, then reports the direct writer's seq via `load_head_seq`, so
    /// the retried `append` (seq=2) should succeed.
    #[tokio::test]
    async fn actor_self_heals_seq_after_append_collision() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CollideOnceStore {
            appends: AtomicUsize,
            head: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl SessionEventStore for CollideOnceStore {
            async fn append(
                &self,
                _id: &SessionId,
                seq: EventSeq,
                _e: &SessionEvent,
                _at: i64,
            ) -> Result<(), SessionError> {
                let n = self.appends.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // First attempt: seq=1 collides with a direct-store writer
                    // that already landed seq=1.
                    assert_eq!(seq, 1);
                    self.head.store(1, Ordering::SeqCst);
                    Err(SessionError::Storage("UNIQUE constraint failed".into()))
                } else {
                    // Retry after resync: head_seq is now 1, so this should be seq=2.
                    assert_eq!(seq, 2);
                    self.head.store(2, Ordering::SeqCst);
                    Ok(())
                }
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
                Ok(self.head.load(Ordering::SeqCst) as EventSeq)
            }

            async fn load_run_markers(
                &self,
            ) -> Result<Vec<(SessionId, Vec<SessionEventRecord>)>, SessionError> {
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

        let store: Arc<dyn SessionEventStore> = Arc::new(CollideOnceStore {
            appends: AtomicUsize::new(0),
            head: AtomicUsize::new(0),
        });
        let id = sample_id();
        let (tx, rx) = mpsc::channel(8);
        let (bcast, _) = broadcast::channel(16);
        let actor = SessionActor::new(id.clone(), store, rx, bcast, None, DEFAULT_IDLE_TIMEOUT);
        tokio::spawn(actor.run());

        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitEvent {
            event: SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
            reply: rtx,
        })
        .await
        .unwrap();

        let seq = rrx.await.unwrap().unwrap();
        assert_eq!(
            seq, 2,
            "actor should self-heal past the seq=1 collision and land at seq=2"
        );
    }
}
