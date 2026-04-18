//! SessionActor — one tokio task per session.
//!
//! The actor replays the event log from storage on startup, then serves
//! `ActorCommand`s until its inbox closes or the idle timeout fires. It owns
//! the in-memory `SessionState` reducer and a `broadcast` channel used to
//! fan out newly-appended events to any subscribers (e.g. UI live views).

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{Duration, Instant};

use crate::session::events::{now_ms, EventSeq, SessionEvent, SessionEventRecord};
use crate::session::service::{SessionError, SessionId};
use crate::session::state::SessionState;
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
    pub id: SessionId,
    pub store: Arc<dyn SessionEventStore>,
    state: SessionState,
    head_seq: EventSeq,
    inbox: mpsc::Receiver<ActorCommand>,
    broadcaster: broadcast::Sender<SessionEventRecord>,
    idle_timeout: Duration,
}

impl SessionActor {
    pub fn new(
        id: SessionId,
        store: Arc<dyn SessionEventStore>,
        inbox: mpsc::Receiver<ActorCommand>,
        broadcaster: broadcast::Sender<SessionEventRecord>,
        idle_timeout: Duration,
    ) -> Self {
        Self {
            id,
            store,
            state: SessionState::default(),
            head_seq: 0,
            inbox,
            broadcaster,
            idle_timeout,
        }
    }

    /// Replays all persisted events and rebuilds `state` + `head_seq`.
    async fn replay(&mut self) -> Result<(), SessionError> {
        let records = self.store.load_all_events(&self.id).await?;
        for record in &records {
            self.state.apply(&record.event);
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
                        let seq = self.head_seq + 1;
                        let at = now_ms();
                        match self.store.append(&self.id, seq, &event, at).await {
                            Ok(()) => {
                                let record = SessionEventRecord {
                                    seq,
                                    event,
                                    created_at_ms: at,
                                };
                                self.state.apply(&record.event);
                                self.head_seq = seq;
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
        let actor = SessionActor::new(id.clone(), store, rx, bcast, DEFAULT_IDLE_TIMEOUT);
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
        let actor = SessionActor::new(id.clone(), store, rx, bcast, DEFAULT_IDLE_TIMEOUT);
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
                },
                at: now_ms(),
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
        let actor = SessionActor::new(id.clone(), store, rx, bcast, DEFAULT_IDLE_TIMEOUT);
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
}
