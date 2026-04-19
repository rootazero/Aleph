//! InProcessActorSessionService — the default SessionService implementation.
//!
//! Owns a router map from `SessionId` to the mpsc inbox of that session's
//! actor. `attach`/`wake` manage actor lifecycle; `emit_event`/`get_events`/
//! `subscribe` route commands to the right actor; `detach` shuts an actor
//! down. Events survive actor restarts because they live in SQLite.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tokio::time::{timeout, Duration};

use crate::session::actor::{ActorCommand, SessionActor, DEFAULT_IDLE_TIMEOUT};
use crate::session::events::{now_ms, EventSeq, SessionEvent, SessionEventRecord};
use crate::session::service::{SessionError, SessionHandle, SessionId, SessionService};
use crate::session::store::SessionEventStore;

const COMMAND_BUFFER: usize = 64;
const BROADCAST_BUFFER: usize = 256;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

pub struct InProcessActorSessionService {
    store: Arc<dyn SessionEventStore>,
    senders: RwLock<HashMap<SessionId, mpsc::Sender<ActorCommand>>>,
    broadcasters: RwLock<HashMap<SessionId, broadcast::Sender<SessionEventRecord>>>,
    idle_timeout: Duration,
}

impl InProcessActorSessionService {
    pub fn new(store: Arc<dyn SessionEventStore>) -> Self {
        Self {
            store,
            senders: RwLock::new(HashMap::new()),
            broadcasters: RwLock::new(HashMap::new()),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }

    pub fn with_idle_timeout(mut self, t: Duration) -> Self {
        self.idle_timeout = t;
        self
    }

    async fn sender_for(&self, id: &SessionId) -> Option<mpsc::Sender<ActorCommand>> {
        self.senders.read().await.get(id).cloned()
    }

    async fn spawn_actor(
        &self,
        id: &SessionId,
    ) -> Result<mpsc::Sender<ActorCommand>, SessionError> {
        let (tx, rx) = mpsc::channel(COMMAND_BUFFER);
        let (bcast_tx, _) = broadcast::channel(BROADCAST_BUFFER);
        let actor = SessionActor::new(
            id.clone(),
            self.store.clone(),
            rx,
            bcast_tx.clone(),
            self.idle_timeout,
        );
        tokio::spawn(actor.run());

        self.senders.write().await.insert(id.clone(), tx.clone());
        self.broadcasters.write().await.insert(id.clone(), bcast_tx);
        Ok(tx)
    }
}

#[async_trait]
impl SessionService for InProcessActorSessionService {
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError> {
        if self.sender_for(&id).await.is_none() {
            self.spawn_actor(&id).await?;
        }
        let head = self.store.load_head_seq(&id).await?;
        Ok(SessionHandle { id, head_seq: head })
    }

    async fn get_events(
        &self,
        id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError> {
        let sender = match self.sender_for(id).await {
            Some(s) => s,
            None => {
                // Allow reads without a live actor — go direct to store.
                return self.store.load_events_range(id, from, to).await;
            }
        };
        let (tx, rx) = oneshot::channel();
        sender
            .send(ActorCommand::GetEvents {
                from,
                to,
                reply: tx,
            })
            .await
            .map_err(|_| SessionError::ActorShutdown)?;
        rx.await.map_err(|_| SessionError::ActorShutdown)?
    }

    async fn emit_event(
        &self,
        id: &SessionId,
        event: SessionEvent,
    ) -> Result<EventSeq, SessionError> {
        let sender = match self.sender_for(id).await {
            Some(s) => s,
            None => self.spawn_actor(id).await?,
        };
        let (tx, rx) = oneshot::channel();
        sender
            .send(ActorCommand::EmitEvent { event, reply: tx })
            .await
            .map_err(|_| SessionError::ActorShutdown)?;
        rx.await.map_err(|_| SessionError::ActorShutdown)?
    }

    async fn subscribe(
        &self,
        id: &SessionId,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError> {
        if self.sender_for(id).await.is_none() {
            self.spawn_actor(id).await?;
        }
        let bcast = {
            self.broadcasters
                .read()
                .await
                .get(id)
                .cloned()
                .ok_or_else(|| SessionError::Other("broadcaster missing".into()))?
        };
        Ok(bcast.subscribe())
    }

    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError> {
        // 1. Shutdown old actor if present.
        if let Some(sender) = self.senders.write().await.remove(id) {
            let (tx, rx) = oneshot::channel();
            let _ = sender.send(ActorCommand::Shutdown { reply: tx }).await;
            let _ = timeout(SHUTDOWN_GRACE, rx).await;
        }
        self.broadcasters.write().await.remove(id);

        // 2. Capture prior_head BEFORE spawning the new actor so the marker
        //    records the exact head the consumer observed pre-wake.
        let prior_head = self.store.load_head_seq(id).await?;

        // 3. Spawn fresh actor — it replays from SQLite.
        let sender = self.spawn_actor(id).await?;

        // 4. Emit SessionWoken marker; new_head = prior_head + 1.
        let (tx, rx) = oneshot::channel();
        sender
            .send(ActorCommand::EmitEvent {
                event: SessionEvent::SessionWoken {
                    at: now_ms(),
                    prior_head,
                },
                reply: tx,
            })
            .await
            .map_err(|_| SessionError::ActorShutdown)?;
        let new_head = rx.await.map_err(|_| SessionError::ActorShutdown)??;

        Ok(SessionHandle {
            id: id.clone(),
            head_seq: new_head,
        })
    }

    async fn detach(&self, id: &SessionId) -> Result<(), SessionError> {
        if let Some(sender) = self.senders.write().await.remove(id) {
            let (tx, rx) = oneshot::channel();
            let _ = sender.send(ActorCommand::Shutdown { reply: tx }).await;
            let _ = timeout(SHUTDOWN_GRACE, rx).await;
        }
        self.broadcasters.write().await.remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{MessageContent, TurnTrigger};
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};

    async fn fresh_service() -> InProcessActorSessionService {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        InProcessActorSessionService::new(store)
    }

    fn sample_id(label: &str) -> SessionId {
        crate::routing::session_key::SessionKey::ephemeral(label)
    }

    #[tokio::test]
    async fn attach_is_idempotent() {
        let svc = fresh_service().await;
        let id = sample_id("idempo");
        let h1 = svc.attach(id.clone()).await.unwrap();
        let h2 = svc.attach(id.clone()).await.unwrap();
        assert_eq!(h1.id, h2.id);
        assert_eq!(h1.head_seq, h2.head_seq);
    }

    #[tokio::test]
    async fn emit_then_get_roundtrip() {
        let svc = fresh_service().await;
        let id = sample_id("rt");
        let tid = uuid::Uuid::new_v4();
        let seq = svc
            .emit_event(
                &id,
                SessionEvent::TurnStarted {
                    turn_id: tid,
                    trigger: TurnTrigger::UserMessage,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        assert_eq!(seq, 1);
        let events = svc.get_events(&id, None, None).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn wake_writes_session_woken_with_prior_head() {
        let svc = fresh_service().await;
        let id = sample_id("wake");
        let tid = uuid::Uuid::new_v4();
        for _ in 0..3 {
            svc.emit_event(
                &id,
                SessionEvent::TurnStarted {
                    turn_id: tid,
                    trigger: TurnTrigger::UserMessage,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        }
        let handle = svc.wake(&id).await.unwrap();
        assert_eq!(
            handle.head_seq, 4,
            "SessionWoken lands at seq 4 after 3 events"
        );

        let events = svc.get_events(&id, None, None).await.unwrap();
        let woken = events
            .iter()
            .find(|r| matches!(r.event, SessionEvent::SessionWoken { .. }));
        assert!(woken.is_some(), "expected SessionWoken marker in log");
        if let Some(r) = woken {
            if let SessionEvent::SessionWoken { prior_head, .. } = &r.event {
                assert_eq!(*prior_head, 3);
            } else {
                panic!("matched SessionWoken but destructure failed");
            }
        }
    }

    #[tokio::test]
    async fn subscribe_delivers_post_subscribe_events() {
        let svc = fresh_service().await;
        let id = sample_id("sub");
        svc.attach(id.clone()).await.unwrap();
        let mut rx = svc.subscribe(&id).await.unwrap();

        svc.emit_event(
            &id,
            SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: "hi".into(),
                    blocks: vec![],
                },
                at: now_ms(),
            },
        )
        .await
        .unwrap();

        let record = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(record.event, SessionEvent::UserMessage { .. }));
    }

    #[tokio::test]
    async fn detach_stops_actor_but_keeps_events() {
        let svc = fresh_service().await;
        let id = sample_id("det");
        let tid = uuid::Uuid::new_v4();
        svc.emit_event(
            &id,
            SessionEvent::TurnStarted {
                turn_id: tid,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
        )
        .await
        .unwrap();
        svc.detach(&id).await.unwrap();

        // Events remain in the store.
        let events = svc.get_events(&id, None, None).await.unwrap();
        assert_eq!(events.len(), 1);

        // Re-attach works; head_seq reflects persisted events.
        let handle = svc.attach(id.clone()).await.unwrap();
        assert_eq!(handle.head_seq, 1);
    }
}
