//! `InProcessActorSessionService` — the default `SessionService` implementation.
//!
//! Owns a router map from `SessionId` to the mpsc inbox of that session's
//! actor. `attach`/`wake` manage actor lifecycle; `emit_event`/`get_events`/
//! `subscribe` route commands to the right actor; `detach` shuts an actor
//! down. Events survive actor restarts because they live in `SQLite`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, RwLock};
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
    /// Per-session mutex serialising the entire `wake()` critical section.
    ///
    /// Without this, two concurrent `wake()` calls for the same session
    /// would race the "shutdown old / spawn new / emit SessionWoken"
    /// sequence: each removes the old sender, each spawns a fresh actor,
    /// and both `SessionWoken` markers land in the event log. Holding the
    /// per-key mutex for the duration of `wake()` makes the second caller
    /// wait, then re-read the now-current prior_head and skip the marker
    /// emit (it sees the freshly-spawned sender via the fast path).
    wake_locks: Mutex<HashMap<SessionId, Arc<Mutex<()>>>>,
    idle_timeout: Duration,
    observer: Option<Arc<dyn crate::session::observer::SessionEventObserver>>,
}

impl InProcessActorSessionService {
    pub fn new(store: Arc<dyn SessionEventStore>) -> Self {
        Self {
            store,
            senders: RwLock::new(HashMap::new()),
            broadcasters: RwLock::new(HashMap::new()),
            wake_locks: Mutex::new(HashMap::new()),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            observer: None,
        }
    }

    pub const fn with_idle_timeout(mut self, t: Duration) -> Self {
        self.idle_timeout = t;
        self
    }

    pub fn with_observer(
        mut self,
        obs: Arc<dyn crate::session::observer::SessionEventObserver>,
    ) -> Self {
        self.observer = Some(obs);
        self
    }

    async fn sender_for(&self, id: &SessionId) -> Option<mpsc::Sender<ActorCommand>> {
        self.senders
            .read()
            .await
            .get(id)
            .filter(|s| !s.is_closed())
            .cloned()
    }

    async fn spawn_actor(
        &self,
        id: &SessionId,
    ) -> Result<mpsc::Sender<ActorCommand>, SessionError> {
        let mut senders = self.senders.write().await;
        let mut broadcasters = self.broadcasters.write().await;

        // Double-check + stale sender cleanup.
        // A sender whose receiver has been dropped (actor idle-timeout or
        // panic) is useless — evict it before creating a replacement.
        if let Some(sender) = senders.get(id) {
            if !sender.is_closed() {
                // rust-doctor-disable-next-line excessive-clone
                return Ok(sender.clone());
            }
            senders.remove(id);
            broadcasters.remove(id);
        }

        let (tx, rx) = mpsc::channel(COMMAND_BUFFER);
        let (bcast_tx, _) = broadcast::channel(BROADCAST_BUFFER);
        let actor = SessionActor::new(
            // rust-doctor-disable-next-line excessive-clone
            id.clone(),
            // rust-doctor-disable-next-line excessive-clone
            self.store.clone(),
            rx,
            // rust-doctor-disable-next-line excessive-clone
            bcast_tx.clone(),
            // rust-doctor-disable-next-line excessive-clone
            self.observer.clone(),
            self.idle_timeout,
        );
        tokio::spawn(actor.run());

        // rust-doctor-disable-next-line excessive-clone
        senders.insert(id.clone(), tx.clone());
        // rust-doctor-disable-next-line excessive-clone
        broadcasters.insert(id.clone(), bcast_tx);
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
            .map_err(|e| {
                tracing::warn!(session_id = ?id, error = %e, "GetEvents send failed");
                SessionError::ActorShutdown
            })?;
        rx.await.map_err(|e| {
            tracing::warn!(session_id = ?id, error = %e, "GetEvents reply dropped");
            SessionError::ActorShutdown
        })?
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
            .map_err(|e| {
                tracing::warn!(session_id = ?id, error = %e, "EmitEvent send failed");
                SessionError::ActorShutdown
            })?;
        rx.await.map_err(|e| {
            tracing::warn!(session_id = ?id, error = %e, "EmitEvent reply dropped");
            SessionError::ActorShutdown
        })?
    }

    async fn subscribe(
        &self,
        id: &SessionId,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError> {
        // Fast path: verify sender is alive before handing out a broadcaster.
        // An idle-timed-out actor leaves its broadcaster in the map; without
        // this check subscribers would receive a dead channel.
        {
            let senders = self.senders.read().await;
            let broadcasters = self.broadcasters.read().await;
            if let (Some(sender), Some(bcast)) = (senders.get(id), broadcasters.get(id)) {
                if !sender.is_closed() {
                    return Ok(bcast.subscribe());
                }
            }
        }

        // Slow path: spawn (or re-spawn) the actor atomically.
        self.spawn_actor(id).await?;

        if self.sender_for(id).await.is_none() {
            return Err(SessionError::ActorShutdown);
        }

        let bcast = self
            .broadcasters
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| SessionError::Other("broadcaster missing after spawn".into()))?;
        Ok(bcast.subscribe())
    }

    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError> {
        // Serialise concurrent wakes for the same session. The per-key
        // mutex covers the entire shutdown-old / spawn-new / emit-marker
        // sequence so two racing wake() callers cannot each spawn a fresh
        // actor and emit duplicate SessionWoken markers.
        let wake_lock = {
            let mut locks = self.wake_locks.lock().await;
            locks
                .entry(id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = wake_lock.lock().await;

        // 1. Shutdown old actor if present.
        if let Some(sender) = self.senders.write().await.remove(id) {
            let (tx, rx) = oneshot::channel();
            if let Err(e) = sender.send(ActorCommand::Shutdown { reply: tx }).await {
                tracing::warn!(session_id = ?id, error = %e, "Wake shutdown send failed");
            }
            match timeout(SHUTDOWN_GRACE, rx).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(session_id = ?id, error = %e, "Wake shutdown reply dropped")
                }
                Err(_) => {
                    // The old actor may still be processing a command (the
                    // SQLite sequence counter is per-actor, so two live actors
                    // for the same session could clobber each other's inserts).
                    // Refuse to spawn a replacement and surface the timeout so
                    // the caller can decide whether to retry.
                    tracing::warn!(session_id = ?id, "Wake shutdown timed out after {:?}", SHUTDOWN_GRACE);
                    return Err(SessionError::ShutdownTimeout);
                }
            }
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
            .map_err(|e| {
                tracing::warn!(session_id = ?id, error = %e, "Wake EmitEvent send failed");
                SessionError::ActorShutdown
            })?;
        let new_head = rx.await.map_err(|e| {
            tracing::warn!(session_id = ?id, error = %e, "Wake EmitEvent reply dropped");
            SessionError::ActorShutdown
        })??;

        Ok(SessionHandle {
            // rust-doctor-disable-next-line excessive-clone
            id: id.clone(),
            head_seq: new_head,
        })
    }

    async fn detach(&self, id: &SessionId) -> Result<(), SessionError> {
        if let Some(sender) = self.senders.write().await.remove(id) {
            let (tx, rx) = oneshot::channel();
            if let Err(e) = sender.send(ActorCommand::Shutdown { reply: tx }).await {
                tracing::warn!(session_id = ?id, error = %e, "Detach shutdown send failed");
            }
            match timeout(SHUTDOWN_GRACE, rx).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(session_id = ?id, error = %e, "Detach shutdown reply dropped")
                }
                Err(_) => {
                    // Caller asked for the actor to stop; returning Ok here
                    // would let it perform direct store writes while the old
                    // actor may still be appending events. Surface the timeout.
                    tracing::warn!(session_id = ?id, "Detach shutdown timed out after {:?}", SHUTDOWN_GRACE);
                    return Err(SessionError::ShutdownTimeout);
                }
            }
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

    /// Bug #4 regression: `chat.clear` used to wipe only the `messages`
    /// projection, so the model kept replaying the whole event log. Retiring the
    /// log must make the harness's real replay path — `get_events` → `build_prompt`
    /// — come back with zero prior turns, while a message sent afterwards still
    /// lands and is the only thing the model sees.
    #[tokio::test]
    async fn clearing_the_log_empties_the_harness_replay() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let svc = InProcessActorSessionService::new(store.clone());
        let id = sample_id("clear");
        let tid = uuid::Uuid::new_v4();

        let user = |text: &str| SessionEvent::UserMessage {
            turn_id: tid,
            content: MessageContent {
                text: text.to_string(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic: false,
            author_user_id: None,
        };

        svc.emit_event(&id, user("my bank pin is 1234"))
            .await
            .unwrap();
        svc.emit_event(
            &id,
            SessionEvent::AssistantMessage {
                turn_id: tid,
                content: MessageContent {
                    text: "noted".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                usage: None,
                at: now_ms(),
            },
        )
        .await
        .unwrap();

        let before = svc.get_events(&id, None, None).await.unwrap();
        let replayed = crate::harness::agent::prompt::build_prompt(&before, before.len());
        assert_eq!(replayed.len(), 2);

        store.retire_from(&id, 1).await.unwrap();

        let after = svc.get_events(&id, None, None).await.unwrap();
        assert!(after.is_empty(), "cleared events must not replay");
        assert!(
            crate::harness::agent::prompt::build_prompt(&after, after.len()).is_empty(),
            "the model must see no prior turns after a clear"
        );

        // The live actor keeps allocating past the retired seqs, and the new
        // message is the only thing left in the prompt.
        svc.emit_event(&id, user("what is my pin?")).await.unwrap();
        let resumed = svc.get_events(&id, None, None).await.unwrap();
        let prompt = crate::harness::agent::prompt::build_prompt(&resumed, resumed.len());
        assert_eq!(prompt.len(), 1);
        assert_eq!(resumed[0].seq, 3, "seq must advance past the retired rows");
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
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
                author_user_id: None,
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
    async fn observer_fires_on_new_append_not_on_replay() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counter(Arc<AtomicUsize>);
        impl crate::session::observer::SessionEventObserver for Counter {
            fn on_appended(&self, _id: &SessionId, _rec: &SessionEventRecord) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let count = Arc::new(AtomicUsize::new(0));
        let svc = InProcessActorSessionService::new(store)
            .with_observer(Arc::new(Counter(count.clone())));
        let id = sample_id("obs");
        svc.emit_event(
            &id,
            SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "one new append => one callback"
        );
        // wake() forces actor restart; replay must NOT re-fire observer;
        // only the new SessionWoken append fires it once.
        svc.wake(&id).await.unwrap();
        assert_eq!(
            count.load(Ordering::SeqCst),
            2,
            "replay must NOT re-fire; only the new SessionWoken append does"
        );
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
