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
    // All three maps are wrapped in `Arc` so the service can be cheaply
    // cloned (Fix #3): the cleanup task spawned inside `spawn_actor` needs
    // a `'static + Send` handle to prune the maps after an actor exits,
    // and tests in Fix #2 share the service across concurrent tasks.
    senders: Arc<RwLock<HashMap<SessionId, mpsc::Sender<ActorCommand>>>>,
    broadcasters: Arc<RwLock<HashMap<SessionId, broadcast::Sender<SessionEventRecord>>>>,
    /// Per-session mutex serialising the entire `wake()` critical section.
    ///
    /// Without this, two concurrent `wake()` calls for the same session
    /// would race the "shutdown old / spawn new / emit SessionWoken"
    /// sequence: each removes the old sender, each spawns a fresh actor,
    /// and both `SessionWoken` markers land in the event log. Holding the
    /// per-key mutex for the duration of `wake()` makes the second caller
    /// wait, then re-read the now-current prior_head and skip the marker
    /// emit (it sees the freshly-spawned sender via the fast path).
    wake_locks: Arc<Mutex<HashMap<SessionId, Arc<Mutex<()>>>>>,
    idle_timeout: Duration,
    observer: Option<Arc<dyn crate::session::observer::SessionEventObserver>>,
}

// Manual `Clone` would be the same; derive sees through `Arc<_>` fields
// and `Copy` `Duration` / `Option<Arc<_>>`.
impl Clone for InProcessActorSessionService {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            senders: Arc::clone(&self.senders),
            broadcasters: Arc::clone(&self.broadcasters),
            wake_locks: Arc::clone(&self.wake_locks),
            idle_timeout: self.idle_timeout,
            observer: self.observer.as_ref().map(Arc::clone),
        }
    }
}

impl InProcessActorSessionService {
    pub fn new(store: Arc<dyn SessionEventStore>) -> Self {
        Self {
            store,
            senders: Arc::new(RwLock::new(HashMap::new())),
            broadcasters: Arc::new(RwLock::new(HashMap::new())),
            wake_locks: Arc::new(Mutex::new(HashMap::new())),
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
        prior_head: Option<EventSeq>,
    ) -> Result<mpsc::Sender<ActorCommand>, SessionError> {
        // Phase 1: inspect the maps under write lock and decide whether to
        // (a) hand back an already-live sender, or (b) clear the way for a
        // fresh actor. For the wake path (`prior_head.is_some()`) we always
        // remove any existing sender: a concurrent `emit_event` may have
        // created a foreign actor between `wake()`'s sender-remove and its
        // `spawn_actor` call, and wake() must still emit SessionWoken as
        // the next append in the log. The foreign actor is shut down
        // below, outside the lock, before we install our replacement.
        let to_shutdown: Option<mpsc::Sender<ActorCommand>> = {
            let mut senders = self.senders.write().await;
            let mut broadcasters = self.broadcasters.write().await;

            if prior_head.is_some() {
                // Wake path: always create a fresh actor.
                let existing = senders.remove(id);
                broadcasters.remove(id);
                existing
            } else {
                // Normal path: double-check + stale sender cleanup.
                if let Some(sender) = senders.get(id) {
                    if !sender.is_closed() {
                        // rust-doctor-disable-next-line excessive-clone
                        return Ok(sender.clone());
                    }
                    senders.remove(id);
                    broadcasters.remove(id);
                }
                None
            }
        };

        // Phase 2: shut down the previous actor (best-effort) outside the
        // maps lock so the awaited Shutdown reply does not block other
        // operations on unrelated sessions.
        if let Some(old_sender) = to_shutdown {
            let (stx, srx) = oneshot::channel();
            if old_sender
                .send(ActorCommand::Shutdown { reply: stx })
                .await
                .is_ok()
            {
                match timeout(SHUTDOWN_GRACE, srx).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!(
                            session_id = ?id,
                            error = %e,
                            "spawn_actor Shutdown reply dropped"
                        );
                    }
                    Err(_) => {
                        tracing::warn!(
                            session_id = ?id,
                            "spawn_actor Shutdown timed out — old actor may \
                             still be running; replacing anyway"
                        );
                    }
                }
            }
        }

        // Phase 3: open the new inbox + broadcaster, and — only when
        // `wake()` is the caller — append `SessionWoken` at
        // `prior_head + 1` directly to the store, fire the observer, and
        // fan out on the broadcaster. Doing it here (instead of from
        // `wake()` after spawning) removes the race window in which a
        // concurrent `emit_event` could slip an event into the inbox of
        // the freshly-spawned actor ahead of the SessionWoken marker.
        let (tx, rx) = mpsc::channel(COMMAND_BUFFER);
        let (bcast_tx, _) = broadcast::channel(BROADCAST_BUFFER);

        if let Some(prior) = prior_head {
            let seq = prior + 1;
            let at = now_ms();
            let evt = SessionEvent::SessionWoken {
                at,
                prior_head: prior,
            };
            match self.store.append(id, seq, &evt, at).await {
                Ok(()) => {
                    let record = SessionEventRecord {
                        seq,
                        event: evt,
                        created_at_ms: at,
                    };
                    if let Some(obs) = &self.observer {
                        let id_ref = id;
                        let rec_ref = &record;
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            obs.on_appended(id_ref, rec_ref);
                        }))
                        .is_err()
                        {
                            tracing::error!(
                                session_id = ?id,
                                seq,
                                "SessionEventObserver panicked during \
                                 SessionWoken emission"
                            );
                        }
                    }
                    if let Err(_record) = bcast_tx.send(record) {
                        tracing::debug!(
                            session_id = ?id,
                            seq,
                            "broadcast had no receivers during SessionWoken emission"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        session_id = ?id,
                        error = %e,
                        "SessionWoken append failed; falling through to fresh actor"
                    );
                }
            }
        }

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

        // Phase 5 (set up first so the cleanup task is wired before the
        // actor can possibly exit): signal `done_tx` when the actor's
        // `run()` returns. The cleanup task then removes the entry iff
        // the entry's sender is `is_closed()` — i.e. iff the entry is
        // still our (now-dead) actor and has NOT been replaced by a
        // subsequent `wake()` or `spawn_actor`. Without the `is_closed`
        // guard, a wake() racing the old actor's exit would install a
        // fresh actor that the cleanup task then wrongly evicts.
        let (done_tx, done_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            actor.run().await;
            let _ = done_tx.send(());
        });

        // Phase 4: install the new sender / broadcaster in the maps. The
        // map write is done under the maps lock so a concurrent
        // sender_for / wake / emit_event observes a consistent snapshot.
        {
            let mut senders = self.senders.write().await;
            let mut broadcasters = self.broadcasters.write().await;
            // rust-doctor-disable-next-line excessive-clone
            senders.insert(id.clone(), tx.clone());
            // rust-doctor-disable-next-line excessive-clone
            broadcasters.insert(id.clone(), bcast_tx);
        }

        // Phase 5 (continued): spawn the map-prune task. Awaiting
        // `done_rx` here means the cleanup happens after the actor has
        // fully exited. The check-and-remove is performed under the
        // maps' write locks in a single critical section to avoid a
        // TOCTOU race where a concurrent `wake()` could replace the
        // closed sender with a fresh one between our `is_closed()` check
        // and our `remove()` call.
        let cleanup_svc = self.clone();
        let cleanup_id = id.clone();
        tokio::spawn(async move {
            let _ = done_rx.await;
            let mut senders = cleanup_svc.senders.write().await;
            let mut broadcasters = cleanup_svc.broadcasters.write().await;
            let should_remove = match senders.get(&cleanup_id) {
                None => false,
                Some(s) => s.is_closed(),
            };
            if should_remove {
                senders.remove(&cleanup_id);
                broadcasters.remove(&cleanup_id);
            }
        });

        Ok(tx)
    }
}

#[async_trait]
impl SessionService for InProcessActorSessionService {
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError> {
        if self.sender_for(&id).await.is_none() {
            self.spawn_actor(&id, None).await?;
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
            None => self.spawn_actor(id, None).await?,
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
        self.spawn_actor(id, None).await?;

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
        // mutex covers the shutdown-old / capture-prior_head sequence so
        // two racing wake() callers cannot each spawn a fresh actor and
        // emit duplicate SessionWoken markers. The lock is RELEASED
        // before `spawn_actor` (which is fast — no 5 s wait), so a
        // second wake() does not block the full SHUTDOWN_GRACE.
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
        //    records the exact head the consumer observed pre-wake. We use
        //    `.ok()` so a transient store error degrades to "no marker"
        //    rather than failing wake(); the actor will simply come back
        //    with whatever head the store reports.
        let prior_head = self.store.load_head_seq(id).await.ok();

        // 3. Spawn fresh actor — spawn_actor appends SessionWoken at
        //    `prior_head + 1` atomically with the actor's first command,
        //    closing the race window where a concurrent `emit_event`
        //    could land between the shutdown and the marker. The
        //    `wake_lock` MUST be held across `spawn_actor`: previously
        //    the lock was dropped here, which let `emit_event` observe
        //    no sender in `self.senders`, fall through to
        //    `spawn_actor(id, None)`, and create a SECOND actor racing
        //    this one on `(session_id, seq)` — the documented
        //    `SessionWoken.seq == prior_head + 1` invariant could then
        //    be broken by the store's self-heal advancing SessionWoken
        //    to `prior_head + 2`. Holding the lock makes the whole
        //    sequence (shutdown → capture → spawn) atomic with respect
        //    to any concurrent wake/emit on the same session.
        self.spawn_actor(id, prior_head).await?;
        drop(_guard);

        // The new head the caller sees is `prior_head + 1` (SessionWoken
        // appended). If SessionWoken's append failed (store error), the
        // actor's own self-heal will resolve the next collision; the
        // returned handle is still a correct "upper bound on observed
        // state" for the caller.
        let new_head = prior_head.map(|p| p + 1).unwrap_or(1);

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
        // Mirror `senders`/`broadcasters`: a detached session will not call
        // `wake()` again, so leaving its per-key mutex in the map is a slow
        // unbounded leak across many distinct ephemeral sessions (subagents,
        // compaction children). A future `wake()` simply re-creates the entry.
        self.wake_locks.lock().await.remove(id);
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

    /// Regression for the slow unbounded leak audit found in `wake_locks`:
    /// `detach()` removes the actor's sender/broadcaster, but every `wake()`
    /// that ever ran on a session also created a per-key `Arc<Mutex<()>>`
    /// here. A long-running daemon that wakes and detaches many distinct
    /// sessions would accumulate them forever. The fix is a single
    /// `wake_locks.remove(id)` at the end of `detach()`, mirroring the
    /// senders/broadcasters cleanup.
    ///
    /// Future `wake()` calls simply re-create the entry, so the leak fix has
    /// no behavioural effect on rewake of the same session.
    #[tokio::test]
    async fn detach_releases_the_wake_lock_entry() {
        let svc = fresh_service().await;
        let id = sample_id("wake-lock-leak");

        // First wake() allocates the per-key wake_lock.
        svc.wake(&id).await.unwrap();
        {
            let locks = svc.wake_locks.lock().await;
            assert!(
                locks.contains_key(&id),
                "wake() must populate the wake_lock map"
            );
        }

        svc.detach(&id).await.unwrap();
        {
            let locks = svc.wake_locks.lock().await;
            assert!(
                !locks.contains_key(&id),
                "detach() must release the wake_lock entry to avoid \
                 unbounded growth across many distinct sessions"
            );
        }
    }

    /// Fix #1 regression: when an actor exits via the idle-timeout path,
    /// the `finish_emitted` helper extracted from the hot arm must still
    /// fire the observer, broadcast, and reply — and persist the event
    /// — for events processed during the drain. With `idle_timeout = 0`
    /// the actor's first `select!` iteration either picks the inbox or
    /// immediately fires the idle timer; either way the seeded event
    /// must (a) increment the observer counter exactly once, and (b) be
    /// queryable through `get_events`.
    #[tokio::test]
    async fn idle_timeout_zero_observer_and_persistence() {
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
            .with_idle_timeout(Duration::from_millis(0))
            .with_observer(Arc::new(Counter(count.clone())));
        let id = sample_id("idle-zero");
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
        // Allow the actor to idle-out (it has now processed the event).
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "observer must fire on every newly appended event, even with idle_timeout=0"
        );
        let events = svc.get_events(&id, None, None).await.unwrap();
        assert_eq!(events.len(), 1, "the event must be persisted");
    }

    /// Fix #2 regression: `wake()` and `emit_event` racing for the same
    /// session must never let a foreign event land before the
    /// `SessionWoken` marker for that wake. Pre-fix, the window between
    /// `wake()`'s shutdown and its own `SpawnActor(...) → send(EmitEvent
    /// { SessionWoken })` allowed a concurrent `emit_event` to insert
    /// itself into the freshly-spawned actor's inbox first. We assert
    /// the post-fix invariant: every `SessionWoken` in the log has
    /// `seq == prior_head + 1` and no foreign event interleaves between
    /// `prior_head` and the marker.
    #[tokio::test]
    async fn concurrent_wake_and_emit_keeps_session_woken_first() {
        let svc = fresh_service().await;
        let id = sample_id("race");
        let tid = uuid::Uuid::new_v4();
        for _ in 0..5 {
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

        let k = 20;
        for _ in 0..k {
            let svc1 = svc.clone();
            let id1 = id.clone();
            let wake_handle = tokio::spawn(async move { svc1.wake(&id1).await });
            let svc2 = svc.clone();
            let id2 = id.clone();
            let emit_handle = tokio::spawn(async move {
                svc2.emit_event(
                    &id2,
                    SessionEvent::TurnStarted {
                        turn_id: uuid::Uuid::new_v4(),
                        trigger: TurnTrigger::UserMessage,
                        at: now_ms(),
                    },
                )
                .await
            });

            let _ = wake_handle.await.unwrap();
            let _ = emit_handle.await.unwrap();

            let events = svc.get_events(&id, None, None).await.unwrap();
            for ev in &events {
                if let SessionEvent::SessionWoken { prior_head, .. } = &ev.event {
                    let expected_seq = prior_head + 1;
                    assert_eq!(
                        ev.seq, expected_seq,
                        "SessionWoken seq must equal prior_head + 1"
                    );
                    // No foreign event should interleave between
                    // `prior_head` and `SessionWoken`'s seq.
                    for other in &events {
                        if other.seq > *prior_head && other.seq < ev.seq {
                            panic!(
                                "foreign event interleaved before SessionWoken: \
                                 {:?} at seq {} (prior_head={}, woken seq={})",
                                other.event, other.seq, prior_head, ev.seq
                            );
                        }
                    }
                }
            }
        }
    }

    /// Fix #3 regression: an actor that reaches the idle-timeout exit
    /// must have its `senders[id]` / `broadcasters[id]` entries pruned
    /// by the cleanup task spawned in `spawn_actor`. Without the
    /// cleanup, a long-running daemon that creates many distinct
    /// sessions (subagents, ephemeral keys) accumulates entries
    /// indefinitely — the same leak shape that was caught and fixed
    /// for `wake_locks` (see `detach_releases_the_wake_lock_entry`).
    #[tokio::test]
    async fn idle_timeout_prunes_senders_and_broadcasters() {
        let svc = fresh_service()
            .await
            .with_idle_timeout(Duration::from_millis(0));
        let id = sample_id("idle-prune");

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

        // Verify the maps are populated immediately after emit.
        {
            let senders = svc.senders.read().await;
            let broadcasters = svc.broadcasters.read().await;
            assert!(senders.contains_key(&id));
            assert!(broadcasters.contains_key(&id));
        }

        // Wait for the actor to idle-out and the cleanup task to prune.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let senders = svc.senders.read().await;
        let broadcasters = svc.broadcasters.read().await;
        assert!(
            !senders.contains_key(&id),
            "senders map must be pruned after actor exit"
        );
        assert!(
            !broadcasters.contains_key(&id),
            "broadcasters map must be pruned after actor exit"
        );
    }
}
