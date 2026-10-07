//! Continuous projection host — the long-lived consumer of the two live
//! authority sources.
//!
//! [`ProjectionHost`] is NOT another authority. `mount` reuses the SAME
//! [`ToolHandlerRegistry`] and [`OwnershipTree`] the [`crate::capability::zahir_facade::ZahirFacade`]
//! already reads: it subscribes to both live change streams, keeps a
//! per-subscriber bounded pending queue, and — for its default consumer —
//! APPLIES every delivered snapshot into run-loop-readable state rather than
//! mirroring the publisher's own state back at it.
//!
//! # Two source domains, one cut
//!
//! The registry broadcast (256 slots) is a *notification* tap, never a durable
//! recovery source: a lagged receiver resyncs from a fresh snapshot. Ownership
//! changes (`Bumped` / `Revoked` / `Disposed`) never touch the registry cursor
//! — owner generation, registry revision, and session sequence are three
//! independent domains. Any ownership change or any lag invalidates the
//! projection and pushes a fresh snapshot (without advancing the registry
//! cursor, which only the registry advances).
//!
//! # Ordering invariants (externally observable)
//!
//! * subscribe-before-snapshot — both live receivers are created before the
//!   first snapshot is read, so no mutation between subscribe and snapshot is
//!   lost (it is either reflected in the snapshot or delivered as a later
//!   event);
//! * the initial snapshot is visible before any post-subscribe change —
//!   `attach` enqueues it first, under the same `publish` boundary that
//!   fan-out uses;
//! * `Invalidated` always precedes the replacement `Snapshot`;
//! * a closed host never delivers later mutations — `close` publishes the
//!   closed bit and the source worker delivers a final closed snapshot.

use crate::capability::descriptor::CapabilityId;
use crate::capability::facade::{CapabilityChange, Cursor, Scope};
use crate::capability::ownership::{OwnerGeneration, OwnershipChange, OwnershipTree};
use crate::capability::zahir_facade::{FacadeEntriesSnapshot, ZahirFacade};
use crate::sync_primitives::Arc;
use crate::tools::registry::{RegistryChange, RegistryEntry, ToolHandlerRegistry};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::{broadcast, Notify};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Per-consumer pending-event capacity (Ruling14). The default consumer
/// applies snapshots into run-loop-readable state and effectively never
/// backlogs, but the SAME bound applies uniformly so a stalled applier cannot
/// grow unbounded.
pub const DEFAULT_PENDING_CAPACITY: usize = 64;

/// Minimum representable capacity. `Invalidated` + replacement `Snapshot` are
/// delivered as ONE atomic pair, so a consumer needs room for at least two
/// events; a capacity of `1` cannot represent a valid invalidation and is
/// rejected (debug-asserted in [`ConsumerState::new`]).
pub const MIN_PENDING_CAPACITY: usize = 2;

/// One projected cut of the live authorities.
///
/// `entries` and `owner_generations` share identical id membership (an entry
/// is exposed iff its id still has a live ownership binding under the scope's
/// visibility). The registry cut (`entries` + `registry_cursor` +
/// `registry_closed`) comes from ONE [`RegistrySnapshot`](crate::tools::registry::RegistrySnapshot)
/// load; the owner cut (`owner_generations`) from ONE ownership-tree
/// acquisition. The two cuts are independent authority domains and are not
/// jointly atomic — callers needing cross-authority ordering use the
/// subscribe-before-snapshot path, not this type.
#[derive(Clone)]
pub struct HostSnapshot {
    /// Registry entries in scope, keyed by tool name, SHARED behind an [`Arc`]
    /// so cloning a snapshot is cheap.
    pub entries: Arc<HashMap<String, RegistryEntry>>,
    /// The registry revision `entries` were frozen from (registry cut).
    pub registry_cursor: Cursor,
    /// The registry close flag from the same frozen state (registry cut).
    pub registry_closed: bool,
    /// Owner generation for every id in `entries` (identical membership).
    pub owner_generations: HashMap<CapabilityId, OwnerGeneration>,
}

impl HostSnapshot {
    fn from_facade(f: FacadeEntriesSnapshot) -> Self {
        HostSnapshot {
            entries: f.entries,
            registry_cursor: f.registry_cursor,
            registry_closed: f.registry_closed,
            owner_generations: f.owner_generations,
        }
    }
}

/// A single event delivered to a subscriber.
///
/// `Invalidated` carries no payload: it means "the projection you hold is no
/// longer derivable from delivered deltas — the NEXT event is a full
/// replacement `Snapshot`". It is distinct from a registry `Lagged` (which is
/// an internal broadcast signal, not a projection event) and from a closed
/// source (which terminates the stream).
#[derive(Clone)]
pub enum ProjectionEvent {
    Snapshot(HostSnapshot),
    Change(CapabilityChange),
    Invalidated,
}

impl std::fmt::Debug for HostSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `RegistryEntry` has no Debug (it wraps `Arc<dyn ToolHandler>`), so
        // surface the entry names instead of the entries themselves.
        f.debug_struct("HostSnapshot")
            .field("entry_names", &self.entries.keys().collect::<Vec<_>>())
            .field("registry_cursor", &self.registry_cursor)
            .field("registry_closed", &self.registry_closed)
            .field("owner_generations", &self.owner_generations)
            .finish()
    }
}

impl std::fmt::Debug for ProjectionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectionEvent::Snapshot(s) => f.debug_tuple("Snapshot").field(s).finish(),
            ProjectionEvent::Change(c) => f.debug_tuple("Change").field(c).finish(),
            ProjectionEvent::Invalidated => f.write_str("Invalidated"),
        }
    }
}

/// The bounded, replaceable pending queue of one subscriber.
///
/// This is a replaceable bounded buffer, NOT a `tokio::sync::mpsc`: an mpsc
/// `try_send` on a full channel only drops the NEWEST item and cannot remove
/// the stale backlog. Overflow therefore atomically clears the stale PENDING
/// backlog and enqueues `Invalidated` + a fresh `Snapshot` (or a fresh
/// `Snapshot` alone, for the snapshot-only default consumer).
struct QueueState {
    items: VecDeque<ProjectionEvent>,
    last_cursor: Cursor,
}

/// The delivery state of one subscriber (default or attached handle).
struct ConsumerState {
    scope: Scope,
    capacity: usize,
    queue: Mutex<QueueState>,
    wake: Notify,
    closed: AtomicBool,
}

impl ConsumerState {
    fn new(scope: Scope, capacity: usize) -> Self {
        debug_assert!(
            capacity >= MIN_PENDING_CAPACITY,
            "capacity {capacity} cannot represent an Invalidated+Snapshot pair"
        );
        ConsumerState {
            scope,
            capacity,
            queue: Mutex::new(QueueState {
                items: VecDeque::new(),
                last_cursor: Cursor(0),
            }),
            wake: Notify::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn try_pop(&self) -> Option<ProjectionEvent> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .items
            .pop_front()
    }

    /// Enqueue a full snapshot. A snapshot is self-contained, so on overflow
    /// the stale backlog is dropped and only the fresh snapshot remains.
    fn enqueue_snapshot(&self, snapshot: HostSnapshot) {
        let mut q = self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.items.len() >= self.capacity {
            q.items.clear();
        }
        q.items.push_back(ProjectionEvent::Snapshot(snapshot.clone()));
        q.last_cursor = snapshot.registry_cursor;
        drop(q);
        self.wake.notify_one();
    }

    /// Enqueue an invalidation pair (`Invalidated` + replacement snapshot) as
    /// ONE atomic unit: the pair is guaranteed to fit together, clearing the
    /// stale backlog first when it would not.
    fn enqueue_invalidation(&self, snapshot: HostSnapshot) {
        let mut q = self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.items.len() + 2 > self.capacity {
            q.items.clear();
        }
        q.items.push_back(ProjectionEvent::Invalidated);
        q.items.push_back(ProjectionEvent::Snapshot(snapshot.clone()));
        q.last_cursor = snapshot.registry_cursor;
        drop(q);
        self.wake.notify_one();
    }

    /// Enqueue a mapped registry change for an in-scope consumer. A change
    /// whose cursor is not strictly newer than the last delivered cursor is
    /// stale (already reflected in a delivered snapshot) and dropped — the
    /// registry cursor only advances, so delivering it would regress state.
    /// On overflow the backlog is cleared and replaced with `Invalidated` +
    /// a fresh snapshot.
    fn enqueue_change(&self, change: CapabilityChange, cursor: Cursor, replacement: HostSnapshot) {
        let mut q = self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if cursor <= q.last_cursor {
            return;
        }
        if q.items.len() >= self.capacity {
            q.items.clear();
            q.items.push_back(ProjectionEvent::Invalidated);
            q.items.push_back(ProjectionEvent::Snapshot(replacement.clone()));
            q.last_cursor = replacement.registry_cursor;
        } else {
            q.items.push_back(ProjectionEvent::Change(change));
            q.last_cursor = cursor;
        }
        drop(q);
        self.wake.notify_one();
    }
}

/// Run-loop-readable applied state of the default consumer. The default
/// consumer's queue is drained by a long-lived applier task that writes
/// snapshots here — the publisher's own state is never presented AS the
/// delivered state.
struct AppliedState {
    snapshot: Option<HostSnapshot>,
    closed: bool,
}

/// Non-default consumers, keyed by attach id (the default consumer lives in
/// its own field so its applier has a stable handle).
struct ConsumerMap {
    next_id: u64,
    by_id: HashMap<u64, Arc<ConsumerState>>,
}

/// Shared host state, owned by [`ProjectionHost`] and the two long-lived tasks.
struct HostInner {
    facade: Arc<ZahirFacade>,
    registry: ToolHandlerRegistry,
    #[allow(dead_code)] // held to keep the ownership sender alive for the worker
    tree: Arc<OwnershipTree>,
    /// ONE explicit sync boundary: serializes `attach`'s consumer-insert +
    /// initial snapshot with the source worker's per-event fan-out, so the
    /// initial snapshot is visible before any post-cut mutation and every
    /// post-cut mutation is delivered or explicitly replaced.
    ///
    /// Lock order: `publish` → (facade/ownership mutexes reached via
    /// `build_snapshot`) AND `publish` → `consumers` → a consumer's `queue`.
    /// Never held across an `.await`. Authority mutation (register / replace /
    /// unregister / close / bump / revoke / dispose) never takes `publish`, so
    /// no authority mutation can block on a consumer.
    publish: Mutex<()>,
    consumers: Mutex<ConsumerMap>,
    default_consumer: Arc<ConsumerState>,
    applied: Mutex<AppliedState>,
    cancel: CancellationToken,
    /// Task-control scaffolding for Task4's cancellation-safe completion /
    /// drain proof. The source worker owns `source_task`; the default applier
    /// owns `applier_task`. Both are `tokio::task::JoinHandle<()>`s stored in
    /// the host so `close_and_await` can await them in teardown order.
    source_task: Mutex<Option<JoinHandle<()>>>,
    applier_task: Mutex<Option<JoinHandle<()>>>,
}

/// The live, long-lived projection of the two authority sources.
pub struct ProjectionHost {
    inner: Arc<HostInner>,
}

impl ProjectionHost {
    /// Mount the projection host on the SAME registry + ownership tree the
    /// facade already reads. Never creates a second authority: `ZahirFacade`
    /// is constructed from the passed registry (it owns no copy), and both
    /// live change streams are subscribed before the first snapshot.
    #[must_use]
    pub fn mount(registry: ToolHandlerRegistry, tree: Arc<OwnershipTree>) -> Arc<Self> {
        let facade = ZahirFacade::new(registry.clone(), Arc::clone(&tree));
        // Subscribe to BOTH live sources BEFORE reading the initial snapshot
        // (subscribe-before-snapshot).
        let registry_rx = registry.subscribe();
        let ownership_rx = tree.subscribe_changes();

        let default_scope = Scope::default();
        let initial = HostSnapshot::from_facade(facade.snapshot_entries_in_scope(&default_scope));
        let default_consumer = Arc::new(ConsumerState::new(default_scope, DEFAULT_PENDING_CAPACITY));

        let inner = Arc::new(HostInner {
            facade,
            registry: registry.clone(),
            tree,
            publish: Mutex::new(()),
            consumers: Mutex::new(ConsumerMap {
                next_id: 1,
                by_id: HashMap::new(),
            }),
            default_consumer: Arc::clone(&default_consumer),
            applied: Mutex::new(AppliedState {
                snapshot: Some(initial),
                closed: false,
            }),
            cancel: CancellationToken::new(),
            source_task: Mutex::new(None),
            applier_task: Mutex::new(None),
        });

        let host = Arc::new(ProjectionHost {
            inner: Arc::clone(&inner),
        });

        // Long-lived tasks. The source worker observes both live streams plus
        // the registry close signal; the applier drains the default consumer
        // queue into run-loop-readable applied state. JoinHandles are retained
        // for `close_and_await` (Task4 completes the drain proof).
        let source = tokio::spawn(source_worker(Arc::clone(&inner), registry_rx, ownership_rx));
        let applier = tokio::spawn(applier_worker(
            Arc::clone(&default_consumer),
            Arc::clone(&inner),
        ));
        *inner.source_task.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source);
        *inner.applier_task.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(applier);

        host
    }

    /// The run-loop-readable applied snapshot, or `None` when the projection
    /// has failed closed (the installed source is closed, or the default
    /// applier observed a closed snapshot). Never returns stale entries for a
    /// closed source.
    #[must_use]
    pub fn current_snapshot(&self) -> Option<HostSnapshot> {
        // Fail closed on an installed-but-closed source.
        if self.inner.registry.is_closed() {
            return None;
        }
        let applied = self.inner.applied.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if applied.closed {
            return None;
        }
        applied.snapshot.clone()
    }

    /// Attach a new subscriber with its own independent bounded queue, scoped
    /// by `scope`. The initial snapshot is enqueued FIRST, under the same
    /// `publish` boundary fan-out uses, so it is visible before any
    /// post-attach change.
    #[must_use]
    pub fn attach(&self, scope: Scope) -> ProjectionHandle {
        let _publish = self.inner.publish.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = build_snapshot(&self.inner, &scope);
        let consumer = Arc::new(ConsumerState::new(scope, DEFAULT_PENDING_CAPACITY));
        consumer.enqueue_snapshot(snapshot);

        let mut map = self
            .inner
            .consumers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = map.next_id;
        map.next_id += 1;
        map.by_id.insert(id, Arc::clone(&consumer));
        drop(map);
        drop(_publish);

        ProjectionHandle {
            host: Arc::clone(&self.inner),
            id,
            consumer,
            cancel: CancellationToken::new(),
        }
    }

    /// Close the host and await the background tasks.
    ///
    /// Order: publish registry closure (the source worker's registered
    /// `close_signal` waiter wakes it to deliver the final closed projection),
    /// await the source worker, then await the applier (which drains the final
    /// closed snapshot). This is the single teardown entry point; the
    /// cancellation-safe completion/drain PROOF is Task4.
    pub async fn close_and_await(self: Arc<Self>) {
        // Publish the closed bit; the source worker observes it via
        // `close_signal` and delivers the final closed snapshot before exiting.
        self.inner.registry.close();

        if let Some(handle) = self
            .inner
            .source_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = handle.await;
        }

        // Fallback: ensure the default consumer is closed so the applier can
        // finish even if the source worker already exited (e.g. cancellation).
        self.inner.default_consumer.mark_closed();

        if let Some(handle) = self
            .inner
            .applier_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = handle.await;
        }

        // Belt-and-braces for any straggler.
        self.inner.cancel.cancel();
    }
}

/// A subscriber's receipt end of the projection stream.
pub struct ProjectionHandle {
    host: Arc<HostInner>,
    id: u64,
    consumer: Arc<ConsumerState>,
    cancel: CancellationToken,
}

impl ProjectionHandle {
    /// Receive the next event: the initial snapshot, then changes / invalidations
    /// in delivery order. `None` once the consumer is closed (or cancelled) and
    /// its pending queue is drained.
    pub async fn recv(&mut self) -> Option<ProjectionEvent> {
        loop {
            if let Some(event) = self.consumer.try_pop() {
                return Some(event);
            }
            if self.consumer.is_closed() || self.cancel.is_cancelled() {
                return None;
            }
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return None,
                _ = self.consumer.wake.notified() => {}
            }
        }
    }

    /// Stop delivery: the consumer is marked closed, so `recv` drains any
    /// already-pending events and then returns `None`.
    pub fn cancel(&self) {
        self.consumer.mark_closed();
    }

    /// Close this subscriber and remove it from the host fan-out map, so its
    /// independent bounded queue can be dropped and future fan-outs skip it.
    /// Delivery-drain completion proof is Task4.
    pub async fn close(self) {
        self.consumer.mark_closed();
        let _publish = self.host.publish.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut map = self
            .host
            .consumers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.by_id.remove(&self.id);
    }
}

/// Build a fresh [`HostSnapshot`] for `scope` from the live authorities. Only
/// ever called while holding `publish`, so it observes a quiescent fan-out
/// boundary (though the two authority cuts are still independent domains).
fn build_snapshot(inner: &HostInner, scope: &Scope) -> HostSnapshot {
    HostSnapshot::from_facade(inner.facade.snapshot_entries_in_scope(scope))
}

/// Source worker: the single subscriber to both live streams.
///
/// * registry change → default consumer gets a fresh snapshot; non-default
///   in-scope consumers get the mapped change (overflow → replaced);
/// * any ownership change or any lag → invalidate every consumer (fresh
///   snapshot, registry cursor untouched);
/// * close → deliver a final closed snapshot and terminate.
async fn source_worker(
    inner: Arc<HostInner>,
    mut registry_rx: broadcast::Receiver<RegistryChange>,
    mut ownership_rx: broadcast::Receiver<OwnershipChange>,
) {
    // Register the close waiter BEFORE reading the closed bit (register-then-
    // check): close() publishes the bit then notify_waiters, so a waiter that
    // registers first can never lose the wake.
    let close_signal = inner.registry.close_signal();
    let notified = close_signal.notified();
    tokio::pin!(notified);
    if inner.registry.is_closed() {
        deliver_close(&inner);
        return;
    }

    loop {
        tokio::select! {
            biased;
            _ = inner.cancel.cancelled() => {
                // Teardown without a close projection; Task4 owns the drain proof.
                return;
            }
            _ = &mut notified => {
                deliver_close(&inner);
                return;
            }
            res = registry_rx.recv() => {
                match res {
                    Ok(change) => process_registry_change(&inner, change),
                    Err(broadcast::error::RecvError::Lagged(_)) => invalidate_all(&inner),
                    Err(broadcast::error::RecvError::Closed) => {
                        // The sender dropped: treat as a closed source.
                        deliver_close(&inner);
                        return;
                    }
                }
            }
            res = ownership_rx.recv() => {
                match res {
                    Ok(_change) => invalidate_all(&inner),
                    Err(broadcast::error::RecvError::Lagged(_)) => invalidate_all(&inner),
                    Err(broadcast::error::RecvError::Closed) => {
                        // The host holds the ownership tree, so its sender
                        // outlives this worker; unreachable in practice.
                    }
                }
            }
        }
    }
}

/// Fan out one registry change to every consumer.
fn process_registry_change(inner: &HostInner, change: RegistryChange) {
    let (cursor, cap_change) = inner.facade.map_registry_change(change);
    let _publish = inner.publish.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

    // Default consumer: apply a fresh snapshot so run-loop-readable state stays
    // current (the applier drains this queue into `applied`).
    if !inner.default_consumer.is_closed() {
        let snapshot = build_snapshot(inner, &inner.default_consumer.scope);
        inner.default_consumer.enqueue_snapshot(snapshot);
    }

    // Non-default consumers: deliver the mapped change iff in scope AND newer
    // than the consumer's last cursor; overflow replaces the backlog.
    let consumers = inner.consumers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for consumer in consumers.by_id.values() {
        if consumer.is_closed() {
            continue;
        }
        if inner.facade.change_in_scope(&cap_change, &consumer.scope) {
            let replacement = build_snapshot(inner, &consumer.scope);
            consumer.enqueue_change(cap_change.clone(), cursor, replacement);
        }
    }
}

/// Invalidate every consumer: enqueue `Invalidated` + a fresh snapshot (or a
/// fresh snapshot alone for the snapshot-only default consumer). The registry
/// cursor is NOT advanced — only the registry advances it.
fn invalidate_all(inner: &HostInner) {
    let _publish = inner.publish.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

    if !inner.default_consumer.is_closed() {
        let snapshot = build_snapshot(inner, &inner.default_consumer.scope);
        inner.default_consumer.enqueue_snapshot(snapshot);
    }

    let consumers = inner.consumers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for consumer in consumers.by_id.values() {
        if consumer.is_closed() {
            continue;
        }
        let snapshot = build_snapshot(inner, &consumer.scope);
        consumer.enqueue_invalidation(snapshot);
    }
}

/// Deliver the terminal close projection to every consumer and mark each closed.
fn deliver_close(inner: &HostInner) {
    let _publish = inner.publish.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

    if !inner.default_consumer.is_closed() {
        let snapshot = build_snapshot(inner, &inner.default_consumer.scope);
        inner.default_consumer.enqueue_snapshot(snapshot);
        inner.default_consumer.mark_closed();
    }

    let consumers = inner.consumers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for consumer in consumers.by_id.values() {
        if consumer.is_closed() {
            continue;
        }
        let snapshot = build_snapshot(inner, &consumer.scope);
        consumer.enqueue_snapshot(snapshot);
        consumer.mark_closed();
    }
}

/// Default delivery applier: drains the default consumer's queue and APPLIES
/// each snapshot into `applied` (run-loop-readable state). `Change` /
/// `Invalidated` are meaningless for a snapshot-only consumer and ignored.
async fn applier_worker(default: Arc<ConsumerState>, inner: Arc<HostInner>) {
    loop {
        if let Some(event) = default.try_pop() {
            match event {
                ProjectionEvent::Snapshot(snapshot) => {
                    let mut applied =
                        inner.applied.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    applied.snapshot = Some(snapshot.clone());
                    if snapshot.registry_closed {
                        applied.closed = true;
                    }
                }
                ProjectionEvent::Change(_) | ProjectionEvent::Invalidated => {}
            }
            continue;
        }
        if default.is_closed() {
            let mut applied =
                inner.applied.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            applied.closed = true;
            return;
        }
        if inner.cancel.is_cancelled() {
            return;
        }
        tokio::select! {
            biased;
            _ = inner.cancel.cancelled() => return,
            _ = default.wake.notified() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::backend::TOOL_NAMESPACE;
    use crate::capability::descriptor::{CapabilityId, CapabilityRevision};
    use crate::capability::ownership::LifetimeScope;
    use crate::session::events::ToolOutput;
    use crate::tools::descriptor::{ReplayPolicy, ToolCapabilityDescriptor, ToolKind, SCHEMA_VERSION};
    use crate::tools::handlers::ToolHandler;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
    use async_trait::async_trait;
    use serde_json::Value;

    struct FakeHandler {
        name: String,
        source: ToolSource,
        idempotent: bool,
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: serde_json::json!({ "tool": self.name }),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: self.source.clone(),
                metadata: ToolDefinitionMetadata {
                    idempotent: self.idempotent,
                    ..Default::default()
                },
            }
        }
    }

    fn fake(name: &str) -> Arc<dyn ToolHandler> {
        Arc::new(FakeHandler {
            name: name.into(),
            source: ToolSource::Builtin,
            idempotent: false,
        })
    }

    fn desc(name: &str) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.into(),
            kind: ToolKind::Tool,
            schema_version: SCHEMA_VERSION,
            description: String::new(),
            input_schema: serde_json::json!({}),
            source: ToolSource::Builtin,
            replay_policy: ReplayPolicy::Unsafe,
            requires_confirmation: false,
            idempotent: false,
            concurrent_safe: false,
            max_duration_ms: None,
            revision: 0,
            implementation_contract: None,
        }
    }

    fn tool_id(name: &str) -> CapabilityId {
        CapabilityId {
            namespace: TOOL_NAMESPACE.to_string(),
            name: name.to_string(),
        }
    }

    /// Deterministic single-event receive with a hard bound (no sleeps).
    async fn recv_bounded(handle: &mut ProjectionHandle) -> ProjectionEvent {
        tokio::time::timeout(std::time::Duration::from_secs(10), handle.recv())
            .await
            .expect("timed out waiting for a projection event")
            .expect("stream closed unexpectedly")
    }

    #[tokio::test]
    async fn attach_receives_initial_snapshot_then_post_subscribe_change() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => {
                assert_eq!(snap.entries.len(), 1);
                assert!(snap.entries.contains_key("a"));
                assert_eq!(snap.registry_cursor, Cursor(1));
            }
            other => panic!("expected initial snapshot, got {other:?}"),
        }

        reg.register(desc("b"), fake("b")).unwrap();
        match recv_bounded(&mut handle).await {
            ProjectionEvent::Change(CapabilityChange::Registered { id, revision }) => {
                assert_eq!(id.name, "b");
                assert_eq!(revision, CapabilityRevision(2));
            }
            other => panic!("expected registered change, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn attach_snapshot_is_handler_descriptor_atomic() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));

        let mut h1 = host.attach(Scope::default());
        let snap1 = match recv_bounded(&mut h1).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected snapshot, got {other:?}"),
        };
        let e1 = snap1.entries.get("a").unwrap();
        assert_eq!(e1.descriptor.revision, 1, "handler+descriptor from one generation");
        assert_eq!(e1.handler.definition().name, "a");

        // Replace advances the registry revision to 2.
        reg.replace(desc("a"), fake("a")).unwrap();

        let mut h2 = host.attach(Scope::default());
        let snap2 = match recv_bounded(&mut h2).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected snapshot, got {other:?}"),
        };
        assert_eq!(snap2.entries.get("a").unwrap().descriptor.revision, 2);
        // The first handle's frozen snapshot is untouched by the later replace.
        assert_eq!(snap1.entries.get("a").unwrap().descriptor.revision, 1);
    }

    #[tokio::test]
    async fn stale_cursor_is_not_durable_recovery() {
        // A fresh host on an independent authority incarnation starts from that
        // incarnation's own revision — there is NO input-cursor API, so a stale
        // cursor can never be injected as a recovery seed.
        let reg_a = ToolHandlerRegistry::new();
        reg_a.register(desc("x"), fake("x")).unwrap();
        reg_a.register(desc("y"), fake("y")).unwrap(); // rev 2

        let tree = Arc::new(OwnershipTree::new());
        let host_a = ProjectionHost::mount(reg_a.clone(), Arc::clone(&tree));
        let mut h_a = host_a.attach(Scope::default());
        let snap_a = match recv_bounded(&mut h_a).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected snapshot, got {other:?}"),
        };
        assert_eq!(snap_a.registry_cursor, Cursor(2));

        // A completely independent registry (fresh incarnation) with one tool.
        let reg_b = ToolHandlerRegistry::new();
        reg_b.register(desc("x"), fake("x")).unwrap(); // its own rev 1
        let host_b = ProjectionHost::mount(reg_b.clone(), Arc::clone(&tree));
        let mut h_b = host_b.attach(Scope::default());
        let snap_b = match recv_bounded(&mut h_b).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected snapshot, got {other:?}"),
        };
        // The cursor derives from THIS authority, not from any persisted value.
        assert_eq!(snap_b.registry_cursor, Cursor(1));
    }

    #[tokio::test]
    async fn registry_lag_delivers_invalidated_then_replacement_snapshot() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected initial snapshot, got {other:?}"),
        }

        // Register > 256 tools synchronously (no await) so the source worker's
        // 256-slot broadcast receiver lags before it can drain.
        for i in 0..300 {
            reg.register(desc(&format!("t{i}")), fake(&format!("t{i}")))
                .unwrap();
        }

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Invalidated => {}
            other => panic!("expected Invalidated after lag, got {other:?}"),
        }
        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => {
                assert_eq!(snap.entries.len(), 300, "replacement snapshot must be complete");
            }
            other => panic!("expected replacement snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ownership_bump_invalidates_without_registry_revision() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());

        let snap = match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected snapshot, got {other:?}"),
        };
        assert_eq!(snap.registry_cursor, Cursor(1));

        tree.bump(LifetimeScope::Runtime);

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Invalidated => {}
            other => panic!("expected Invalidated after bump, got {other:?}"),
        }
        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => {
                assert_eq!(snap.registry_cursor, Cursor(1), "registry revision must not advance");
                assert!(snap.entries.contains_key("a"));
            }
            other => panic!("expected replacement snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn revoke_delivers_without_registry_revision_change() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected snapshot, got {other:?}"),
        }

        assert!(tree.revoke(&tool_id("a")), "revoke must be a real transition");

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Invalidated => {}
            other => panic!("expected Invalidated after revoke, got {other:?}"),
        }
        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => {
                assert_eq!(snap.registry_cursor, Cursor(1), "registry revision must not advance");
                assert!(!snap.entries.contains_key("a"), "revoked id must be absent");
            }
            other => panic!("expected replacement snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispose_delivers_without_registry_revision_change() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected snapshot, got {other:?}"),
        }

        assert!(tree.dispose(LifetimeScope::Runtime), "dispose must be a real transition");

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Invalidated => {}
            other => panic!("expected Invalidated after dispose, got {other:?}"),
        }
        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => {
                assert_eq!(snap.registry_cursor, Cursor(1), "registry revision must not advance");
                assert!(!snap.entries.contains_key("a"), "disposed id must be absent");
            }
            other => panic!("expected replacement snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn overflow_replaces_backlog_with_invalidated_snapshot() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());

        // Drain the initial snapshot; the queue is now empty.
        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected initial snapshot, got {other:?}"),
        }

        // 70 changes against a 64-capacity queue: changes 1..=64 fill it, the
        // 65th triggers overflow (clear backlog + Invalidated + fresh snapshot).
        for i in 0..70 {
            reg.register(desc(&format!("t{i}")), fake(&format!("t{i}")))
                .unwrap();
        }

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Invalidated => {}
            other => panic!("overflow must replace the backlog with Invalidated, got {other:?}"),
        }
        let snap = match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected replacement snapshot, got {other:?}"),
        };
        // All 70 registrations completed before the source worker ran, so the
        // replacement snapshot is a COMPLETE resync of the current registry
        // state (not the 65-entry cut at the overflow moment): it carries all
        // 70 entries at the current cursor, and the trailing deltas (66..=70)
        // are stale and dropped.
        assert_eq!(snap.entries.len(), 70);
        assert_eq!(snap.registry_cursor, Cursor(70));
    }

    #[tokio::test]
    async fn overflow_isolated_to_one_consumer() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut slow = host.attach(Scope::default());
        let mut fast = host.attach(Scope::default());

        // Drain both initial snapshots.
        match recv_bounded(&mut slow).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected snapshot, got {other:?}"),
        }
        match recv_bounded(&mut fast).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected snapshot, got {other:?}"),
        }

        // Register 70 tools; `fast` drains each change, `slow` never does, so
        // `slow` overflows while `fast` keeps a small queue and never sees
        // Invalidated.
        let mut fast_changes = 0usize;
        for i in 0..70 {
            reg.register(desc(&format!("t{i}")), fake(&format!("t{i}")))
                .unwrap();
            match recv_bounded(&mut fast).await {
                ProjectionEvent::Change(CapabilityChange::Registered { .. }) => {
                    fast_changes += 1;
                }
                other => panic!("fast consumer must keep receiving changes, got {other:?}"),
            }
        }

        // The slow consumer overflowed and its backlog was replaced.
        match recv_bounded(&mut slow).await {
            ProjectionEvent::Invalidated => {}
            other => panic!("slow consumer must overflow to Invalidated, got {other:?}"),
        }
        let snap = match recv_bounded(&mut slow).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected replacement snapshot, got {other:?}"),
        };
        assert!(!snap.entries.is_empty() && snap.entries.len() < 70);
        let mut slow_remaining = snap.entries.len();
        while slow_remaining < 70 {
            match recv_bounded(&mut slow).await {
                ProjectionEvent::Change(CapabilityChange::Registered { .. }) => slow_remaining += 1,
                other => panic!("expected registered delta for slow consumer, got {other:?}"),
            }
        }

        assert_eq!(fast_changes, 70, "the fast consumer must receive every change");
    }

    #[tokio::test]
    async fn closing_one_subscriber_does_not_affect_others() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut h1 = host.attach(Scope::default());
        let mut h2 = host.attach(Scope::default());

        match recv_bounded(&mut h1).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected snapshot, got {other:?}"),
        }
        match recv_bounded(&mut h2).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected snapshot, got {other:?}"),
        }

        h1.close().await;

        reg.register(desc("x"), fake("x")).unwrap();

        match recv_bounded(&mut h2).await {
            ProjectionEvent::Change(CapabilityChange::Registered { id, .. }) => {
                assert_eq!(id.name, "x");
            }
            other => panic!("h2 must be unaffected by h1's close, got {other:?}"),
        }
    }

    /// A host whose installed source is closed must fail closed: no stale
    /// entries are returned even though the registry still holds its last
    /// snapshot's entries.
    #[tokio::test]
    async fn current_snapshot_fails_closed_on_closed_source() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));

        assert!(host.current_snapshot().is_some());

        // Close the source and give the applier a chance to observe it.
        host.clone().close_and_await().await;

        assert!(host.current_snapshot().is_none(), "closed source must fail closed");
    }
}
