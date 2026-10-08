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

use crate::capability::backend::TOOL_NAMESPACE;
use crate::capability::descriptor::CapabilityId;
use crate::capability::diagnostic_control::{
    DiagnosticError, DiagnosticLifecycle, DiagnosticStatus,
};
use crate::capability::facade::{CapabilityChange, Cursor, Scope};
use crate::capability::ownership::{OwnerGeneration, OwnershipChange, OwnershipTree};
use crate::capability::zahir_facade::{FacadeEntriesSnapshot, ZahirFacade};
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;
use crate::tools::registry::{RegistryChange, RegistryEntry, ToolHandlerRegistry};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, Weak};
use std::time::Duration;
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

/// Process-wide projection host installed by the real server bootstrap.
///
/// The slot retains the host beyond the local startup binding so every
/// post-mount exit still has one explicit lifecycle owner. It is intentionally
/// install-once, matching the process lifetime of the server registry.
static PROJECTION_HOST: CapabilitySlot<Arc<ProjectionHost>> =
    CapabilitySlot::new("capability/projection-host", MissingSemantics::FailsClosed);

/// Install the process-wide projection host. Called once by server startup.
pub fn set_projection_host(host: Arc<ProjectionHost>) {
    let _ = PROJECTION_HOST.install(host);
}

/// Return the installed projection host, if startup mounted one.
pub fn projection_host() -> Option<&'static Arc<ProjectionHost>> {
    PROJECTION_HOST.get()
}

/// Type-erased view for the process capability roster.
pub(crate) const fn projection_host_slot() -> &'static dyn SlotStatus {
    &PROJECTION_HOST
}

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
/// `Snapshot`.
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

    /// Close this receipt endpoint and explicitly discard pending external
    /// delivery. The default consumer does not use this path: its captured
    /// queue is drained by the real applier before it joins.
    fn close_and_discard(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .items
            .clear();
        self.wake.notify_waiters();
    }

    fn try_pop(&self) -> Option<ProjectionEvent> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .items
            .pop_front()
    }

    /// Enqueue a full snapshot. On overflow, atomically replace the stale
    /// backlog with `Invalidated` + the fresh replacement `Snapshot`.
    /// Returns `true` iff overflow fired and the backlog was cleared.
    fn enqueue_snapshot(&self, snapshot: HostSnapshot) -> bool {
        let mut q = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let backlog_cleared = if q.items.len() >= self.capacity {
            q.items.clear();
            q.items.push_back(ProjectionEvent::Invalidated);
            true
        } else {
            false
        };
        q.items
            .push_back(ProjectionEvent::Snapshot(snapshot.clone()));
        q.last_cursor = snapshot.registry_cursor;
        drop(q);
        self.wake.notify_one();
        backlog_cleared
    }

    /// Enqueue an invalidation pair (`Invalidated` + replacement snapshot) as
    /// ONE atomic unit: the pair is guaranteed to fit together, clearing the
    /// stale backlog first when it would not.
    fn enqueue_invalidation(&self, snapshot: HostSnapshot) {
        let mut q = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.items.len() + 2 > self.capacity {
            q.items.clear();
        }
        q.items.push_back(ProjectionEvent::Invalidated);
        q.items
            .push_back(ProjectionEvent::Snapshot(snapshot.clone()));
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
        let mut q = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cursor <= q.last_cursor {
            return;
        }
        if q.items.len() >= self.capacity {
            q.items.clear();
            q.items.push_back(ProjectionEvent::Invalidated);
            q.items
                .push_back(ProjectionEvent::Snapshot(replacement.clone()));
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
///
/// # Replacement receipts (observational only)
///
/// The four receipt fields below are populated by the REAL default applier
/// (the long-lived task that drains the default consumer's queue) under the
/// same lock that writes `snapshot`. They are pure read-only observation;
/// no authority, dispatcher, resolver or session path ever consults them.
/// `awaiting_replacement` is internal: it gates whether the next consumed
/// `Snapshot` counts as a replacement (true) or as a normal apply (false).
/// `close` / `closed` transitions MUST NOT touch these fields (no
/// fabrication, no reset, no restore) so a `Closing` lifecycle never lies
/// about previously consumed invalidations / replacements.
struct AppliedState {
    snapshot: Option<HostSnapshot>,
    closed: bool,
    /// Real default-applier `Invalidated` consumption count. Bumped once
    /// per consumed `Invalidated` event (the default consumer's
    /// `Invalidated` arm under the applied lock). Plain Snapshot applies
    /// (initial mount, close snapshot) do NOT bump this counter.
    applied_invalidation_count: u64,
    /// Real default-applier replacement `Snapshot` consumption count.
    /// Bumped once per consumed `Snapshot` that follows an
    /// `Invalidated`-cleared slot (i.e. `awaiting_replacement == true` on
    /// entry). Plain `Snapshot`s that are NOT replacement (initial mount,
    /// close snapshot, and any direct `Snapshot` that races a non-
    /// invalidated apply) MUST NOT bump this counter.
    applied_replacement_count: u64,
    /// Internal gate: `true` after consuming `Invalidated` (and clearing
    /// `snapshot`); cleared on the very next consumed `Snapshot` after the
    /// replacement receipt is recorded. `false` means a consumed `Snapshot`
    /// is a normal apply and must NOT be treated as a replacement.
    awaiting_replacement: bool,
    /// Registry cursor of the most recent replacement snapshot the
    /// default applier actually applied. `None` until the first consumed
    /// `Invalidated` is followed by a replacement `Snapshot`. Stays put
    /// across close / reset (no fabrication).
    last_replacement_registry_cursor: Option<Cursor>,
    /// Canon tool-id names (just the `name` half of the existing
    /// `applied_tool_ids` shape — same vector shape that the host already
    /// exposes, so no new namespace-keyed authority is introduced) carried
    /// by the most recent applied replacement. Cleared / overwritten only
    /// on the next replacement receipt; never on close.
    last_replacement_tool_ids: Vec<String>,
}

/// Non-default consumers, keyed by attach id (the default consumer lives in
/// its own field so its applier has a stable handle).
struct ConsumerMap {
    next_id: u64,
    by_id: HashMap<u64, Arc<ConsumerState>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionShutdownOutcome {
    /// Whether the source worker joined without a panic or cancellation.
    pub source_joined: bool,
    /// Whether the default applier joined without a panic or cancellation.
    pub applier_joined: bool,
    /// Whether the source worker returned a `JoinError`.
    pub source_failed: bool,
    /// Whether the default applier returned a `JoinError`.
    pub applier_failed: bool,
}

struct CompletionState {
    started: bool,
    outcome: Option<ProjectionShutdownOutcome>,
}

/// Diagnostic hold plane: which side of the host the hold gates.
///
/// * `SourceIntake` — pause the source worker (registry/ownership fan-out).
///   The default applier continues so delivered state keeps moving.
/// * `Delivery` — pause the default applier. The source continues so the
///   pending depth can be observed to grow (and trigger overflow
///   replacement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticPlane {
    SourceIntake,
    Delivery,
}

/// Active diagnostic hold installed by `ProjectionHost::diagnostic_hold`.
/// While installed, the worker for `plane` suspends event advancement
/// until the release time elapses, an explicit `release()` runs, or the
/// host is closed.
struct DiagnosticHold {
    /// Per-host monotonic id; every clear is conditional on it so a stale
    /// timer or holder can never clear a newer hold.
    id: u64,
    plane: DiagnosticPlane,
    release_at: tokio::time::Instant,
    /// Host-owned expiry alarm. It (not the caller's future) is responsible
    /// for clearing the slot at `release_at`; aborted on explicit release.
    timer: tokio::task::JoinHandle<()>,
}

/// The single hold slot plus its id allocator, guarded by one mutex so
/// install / release / close / timer expiry form one state machine.
#[derive(Default)]
struct DiagnosticHoldState {
    next_id: u64,
    active: Option<DiagnosticHold>,
}

/// Optional diagnostic-only state. Disabled hosts do not allocate the hold
/// mutex or notification primitive at all.
struct DiagnosticMachinery {
    hold: Mutex<DiagnosticHoldState>,
    notify: Notify,
}

impl DiagnosticHoldState {
    /// Clear the slot if (and only if) it still holds hold `id`.
    fn clear_if(&mut self, id: u64) -> bool {
        if self.active.as_ref().is_some_and(|h| h.id == id) {
            self.active = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
struct ApplierTestGate {
    entered: Notify,
    release: Notify,
    hold: AtomicBool,
}

#[cfg(test)]
impl ApplierTestGate {
    fn new() -> Self {
        Self {
            entered: Notify::new(),
            release: Notify::new(),
            hold: AtomicBool::new(false),
        }
    }
}

/// Shared host state, owned by [`ProjectionHost`] and the explicit lifecycle boundary.
struct HostInner {
    facade: Arc<ZahirFacade>,
    registry: ToolHandlerRegistry,
    /// The owner authority this host is mounted on. Also exposed (by Arc
    /// identity) so diagnostic control can refuse a second authority.
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
    /// Private notification-only readiness seam: notified by the default
    /// applier whenever it mutates `applied` (first apply, replacement apply,
    /// invalidated clear). Tests (and Task5's future boot gate) await this to
    /// observe ACTUAL delivery rather than a fabricated publisher-state seed.
    /// No production diagnostic or public control surface.
    readiness: Notify,
    cancel: CancellationToken,
    /// Explicit host-local close state. It is independent of both authority
    /// close bits: closing this projection never closes a shared authority.
    closed: AtomicBool,
    /// Task-control for the cancellation-safe shared completion boundary.
    source_task: Mutex<Option<JoinHandle<()>>>,
    applier_task: Mutex<Option<JoinHandle<()>>>,
    completion: Mutex<CompletionState>,
    completion_notify: Notify,
    #[cfg(test)]
    applier_test_gate: ApplierTestGate,
    /// Diagnostic-only hold/timer machinery. `None` is the production
    /// disabled mode and allocates neither the hold mutex nor its notifier.
    diagnostics: Option<DiagnosticMachinery>,
    /// Total `Invalidated` enqueues on the default consumer (overflow
    /// recovery). Counts both `enqueue_snapshot` overflow and explicit
    /// `invalidate_all` invalidations on the default.
    replacement_count: AtomicU64,
    /// Total `Lagged` recvs observed by the source worker on either
    /// registry or ownership channel.
    lag_count: AtomicU64,
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
        Self::mount_with_diagnostics(registry, tree, true)
    }

    /// Mount the canonical projection host with startup-scoped diagnostics.
    /// `false` leaves the ordinary source/delivery path unchanged while
    /// omitting all diagnostic hold/timer machinery.
    #[must_use]
    pub fn mount_with_diagnostics(
        registry: ToolHandlerRegistry,
        tree: Arc<OwnershipTree>,
        diagnostics_enabled: bool,
    ) -> Arc<Self> {
        let facade = ZahirFacade::new(registry.clone(), Arc::clone(&tree));
        // Subscribe to BOTH live sources BEFORE reading the initial snapshot
        // (subscribe-before-snapshot).
        let registry_rx = registry.subscribe();
        let ownership_rx = tree.subscribe_changes();

        let default_scope = Scope::default();
        let initial = HostSnapshot::from_facade(facade.snapshot_entries_in_scope(&default_scope));
        let default_consumer =
            Arc::new(ConsumerState::new(default_scope, DEFAULT_PENDING_CAPACITY));
        // Enqueue the initial snapshot into the DEFAULT delivery queue BEFORE
        // the source/applier tasks spawn, so the REAL default applier applies
        // it (Ruling16). `applied` starts `None`; no publisher state may
        // masquerade as delivered.
        default_consumer.enqueue_snapshot(initial);

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
                snapshot: None,
                closed: false,
                applied_invalidation_count: 0,
                applied_replacement_count: 0,
                awaiting_replacement: false,
                last_replacement_registry_cursor: None,
                last_replacement_tool_ids: Vec::new(),
            }),
            readiness: Notify::new(),
            cancel: CancellationToken::new(),
            closed: AtomicBool::new(false),
            source_task: Mutex::new(None),
            applier_task: Mutex::new(None),
            completion: Mutex::new(CompletionState {
                started: false,
                outcome: None,
            }),
            completion_notify: Notify::new(),
            #[cfg(test)]
            applier_test_gate: ApplierTestGate::new(),
            diagnostics: diagnostics_enabled.then(|| DiagnosticMachinery {
                hold: Mutex::new(DiagnosticHoldState::default()),
                notify: Notify::new(),
            }),
            replacement_count: AtomicU64::new(0),
            lag_count: AtomicU64::new(0),
        });

        let host = Arc::new(ProjectionHost {
            inner: Arc::clone(&inner),
        });

        // Long-lived tasks. The source worker observes both live streams plus
        // the registry close signal; the applier drains the default consumer
        // queue into run-loop-readable applied state. JoinHandles are retained
        // for `close_and_await` (Task4 completes the drain proof).
        let source = tokio::spawn(source_worker(
            Arc::downgrade(&inner),
            inner.cancel.clone(),
            registry_rx,
            ownership_rx,
        ));
        let applier = tokio::spawn(applier_worker(
            Arc::clone(&default_consumer),
            Arc::downgrade(&inner),
            inner.cancel.clone(),
        ));
        *inner
            .source_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source);
        *inner
            .applier_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(applier);

        host
    }

    /// Mount the projection host with a custom default-consumer capacity.
    /// Used by diagnostic tests that need a small pending queue to exercise
    /// overflow replacement semantics. The capacity is clamped to
    /// `MIN_PENDING_CAPACITY` because `Invalidated` + replacement `Snapshot`
    /// must fit as one atomic pair.
    #[must_use]
    pub fn mount_with_capacity(
        registry: ToolHandlerRegistry,
        tree: Arc<OwnershipTree>,
        capacity: usize,
    ) -> Arc<Self> {
        let facade = ZahirFacade::new(registry.clone(), Arc::clone(&tree));
        let registry_rx = registry.subscribe();
        let ownership_rx = tree.subscribe_changes();

        let default_scope = Scope::default();
        let initial = HostSnapshot::from_facade(facade.snapshot_entries_in_scope(&default_scope));
        let default_consumer = Arc::new(ConsumerState::new(
            default_scope,
            capacity.max(MIN_PENDING_CAPACITY),
        ));
        default_consumer.enqueue_snapshot(initial);

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
                snapshot: None,
                closed: false,
                applied_invalidation_count: 0,
                applied_replacement_count: 0,
                awaiting_replacement: false,
                last_replacement_registry_cursor: None,
                last_replacement_tool_ids: Vec::new(),
            }),
            readiness: Notify::new(),
            cancel: CancellationToken::new(),
            closed: AtomicBool::new(false),
            source_task: Mutex::new(None),
            applier_task: Mutex::new(None),
            completion: Mutex::new(CompletionState {
                started: false,
                outcome: None,
            }),
            completion_notify: Notify::new(),
            #[cfg(test)]
            applier_test_gate: ApplierTestGate::new(),
            diagnostics: Some(DiagnosticMachinery {
                hold: Mutex::new(DiagnosticHoldState::default()),
                notify: Notify::new(),
            }),
            replacement_count: AtomicU64::new(0),
            lag_count: AtomicU64::new(0),
        });

        let host = Arc::new(ProjectionHost {
            inner: Arc::clone(&inner),
        });

        let source = tokio::spawn(source_worker(
            Arc::downgrade(&inner),
            inner.cancel.clone(),
            registry_rx,
            ownership_rx,
        ));
        let applier = tokio::spawn(applier_worker(
            Arc::clone(&default_consumer),
            Arc::downgrade(&inner),
            inner.cancel.clone(),
        ));
        *inner
            .source_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source);
        *inner
            .applier_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(applier);

        host
    }

    /// Wait until the default consumer has actually applied a snapshot.
    ///
    /// This observes the same applied state used by `current_snapshot`, never
    /// the publisher's initial snapshot. `false` means the host or its source
    /// closed before a usable applied snapshot existed.
    pub async fn wait_until_ready(&self) -> bool {
        loop {
            if self.current_snapshot().is_some() {
                return true;
            }
            if self.inner.closed.load(Ordering::SeqCst) || self.inner.registry.is_closed() {
                return false;
            }
            let notified = self.inner.readiness.notified();
            tokio::pin!(notified);
            if self.current_snapshot().is_some() {
                return true;
            }
            if self.inner.closed.load(Ordering::SeqCst) || self.inner.registry.is_closed() {
                return false;
            }
            notified.await;
        }
    }

    /// The run-loop-readable applied snapshot, or `None` when the projection
    /// has failed closed (the installed source is closed, or the default
    /// applier observed a closed snapshot). Never returns stale entries for a
    /// closed source.
    #[must_use]
    pub fn current_snapshot(&self) -> Option<HostSnapshot> {
        // Fail closed on an installed-but-closed source.
        if self.inner.closed.load(Ordering::SeqCst) || self.inner.registry.is_closed() {
            return None;
        }
        let applied = self
            .inner
            .applied
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if applied.closed {
            return None;
        }
        applied.snapshot.clone()
    }

    /// The exact owner tree this host was mounted on. Crate-visible so
    /// diagnostic control can verify authority identity with `Arc::ptr_eq`.
    #[must_use]
    pub(crate) fn owner_tree(&self) -> &Arc<OwnershipTree> {
        &self.inner.tree
    }

    /// `true` once the host has been requested to close or has reached its
    /// shared completion boundary. The diagnostic control uses this gate to
    /// refuse further mutations on a closed host.
    #[must_use]
    pub fn is_closing(&self) -> bool {
        if self.inner.closed.load(Ordering::Acquire) {
            return true;
        }
        self.inner
            .completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outcome
            .is_some()
    }

    /// Attach a new subscriber with its own independent bounded queue, scoped
    /// by `scope`. The initial snapshot is enqueued FIRST, under the same
    /// `publish` boundary fan-out uses, so it is visible before any
    /// post-attach change.
    #[must_use]
    pub fn attach(&self, scope: Scope) -> ProjectionHandle {
        let _publish = self
            .inner
            .publish
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let consumer = Arc::new(ConsumerState::new(scope, DEFAULT_PENDING_CAPACITY));
        let mut id = 0;
        if !self.inner.closed.load(Ordering::SeqCst) {
            let snapshot = build_snapshot(&self.inner, &consumer.scope);
            consumer.enqueue_snapshot(snapshot);
            let mut map = self
                .inner
                .consumers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            id = map.next_id;
            map.next_id += 1;
            map.by_id.insert(id, Arc::clone(&consumer));
        } else {
            consumer.close_and_discard();
        }
        drop(_publish);

        ProjectionHandle {
            host: Arc::clone(&self.inner),
            id,
            consumer,
            cancel: CancellationToken::new(),
        }
    }

    /// Close the host and await the one shared, cancellation-safe teardown.
    ///
    /// Local close is linearized under `publish`, stops source intake, and
    /// closes attached receipt queues without closing either shared authority.
    /// A separately authority-driven registry close still wakes the source
    /// worker and follows the same completion boundary. The teardown task owns
    /// both real worker joins, so dropping one waiter cannot detach them.
    pub async fn close_and_await(self: Arc<Self>) -> ProjectionShutdownOutcome {
        start_shutdown(&self.inner);
        loop {
            let notified = self.inner.completion_notify.notified();
            tokio::pin!(notified);
            if let Some(outcome) = self
                .inner
                .completion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .outcome
                .clone()
            {
                return outcome;
            }
            notified.await;
        }
    }

    /// Read-only diagnostic snapshot of the host's current state. Never
    /// mutates host state. The host-local `closed` bit drives `Closing` /
    /// `Closed` lifecycle; the shared completion outcome drives `Closed` for
    /// callers that awaited `close_and_await`.
    #[must_use]
    pub fn diagnostic_status(&self) -> DiagnosticStatus {
        let completion = self
            .inner
            .completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let lifecycle = if completion.outcome.is_some() {
            DiagnosticLifecycle::Closed
        } else if self.inner.closed.load(Ordering::Acquire) || completion.started {
            DiagnosticLifecycle::Closing
        } else {
            DiagnosticLifecycle::Active
        };
        drop(completion);

        let applied = self
            .inner
            .applied
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let registry_cursor = applied
            .snapshot
            .as_ref()
            .map(|s| s.registry_cursor)
            .unwrap_or_default();
        let applied_tool_ids: Vec<CapabilityId> = applied
            .snapshot
            .as_ref()
            .map(|s| {
                s.entries
                    .keys()
                    .map(|name| CapabilityId {
                        namespace: TOOL_NAMESPACE.to_string(),
                        name: name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let applied_owner_generations = applied
            .snapshot
            .as_ref()
            .map(|s| s.owner_generations.clone())
            .unwrap_or_default();
        // Same lock cut as the snapshot above (Ruling16: no two-cut pairing).
        // Receipts are populated by the REAL default applier; close/reset
        // does NOT touch them, so a Closing / Closed lifecycle never lies
        // about previously consumed invalidations / replacements.
        let applied_invalidation_count = applied.applied_invalidation_count;
        let applied_replacement_count = applied.applied_replacement_count;
        let last_replacement_registry_cursor = applied.last_replacement_registry_cursor;
        let last_replacement_tool_ids = applied.last_replacement_tool_ids.clone();
        drop(applied);

        let q = self
            .inner
            .default_consumer
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending_depth = q.items.len();
        let pending_capacity = self.inner.default_consumer.capacity;
        drop(q);

        DiagnosticStatus {
            lifecycle,
            registry_cursor,
            pending_depth,
            pending_capacity,
            replacement_count: self.inner.replacement_count.load(Ordering::Acquire),
            lag_count: self.inner.lag_count.load(Ordering::Acquire),
            applied_tool_ids,
            applied_owner_generations,
            applied_invalidation_count,
            applied_replacement_count,
            last_replacement_registry_cursor,
            last_replacement_tool_ids,
        }
    }

    /// Install a diagnostic hold for `duration` on the named plane. While
    /// installed, the worker for that plane suspends event advancement until
    /// the hold clears (release-time elapses, `release()` is called, or the
    /// host closes). At most one hold may be active. Duration must be
    /// `1..=5000 ms`.
    pub async fn diagnostic_hold(
        &self,
        plane: DiagnosticPlane,
        duration: Duration,
    ) -> Result<(), DiagnosticError> {
        let Some(diagnostics) = self.inner.diagnostics.as_ref() else {
            return Err(DiagnosticError::Closed);
        };
        if duration.is_zero() || duration > Duration::from_millis(5000) {
            return Err(DiagnosticError::InvalidHoldDuration);
        }
        if self.is_closing() {
            return Err(DiagnosticError::Closed);
        }
        let release_at = tokio::time::Instant::now() + duration;
        let id = {
            let mut guard = diagnostics
                .hold
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.active.is_some() {
                return Err(DiagnosticError::HoldAlreadyActive);
            }
            guard.next_id = guard.next_id.wrapping_add(1);
            let id = guard.next_id;
            // The host owns expiry: the alarm clears the slot even if the
            // caller's future is dropped. It holds only a `Weak` so it never
            // extends the host's lifetime, and clears only its own id. It is
            // spawned under the slot lock, so it cannot observe the slot
            // before this hold is installed.
            let weak = Arc::downgrade(&self.inner);
            let timer = tokio::spawn(async move {
                tokio::time::sleep_until(release_at).await;
                if let Some(inner) = weak.upgrade() {
                    if let Some(diagnostics) = inner.diagnostics.as_ref() {
                        let cleared = diagnostics
                            .hold
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clear_if(id);
                        if cleared {
                            diagnostics.notify.notify_waiters();
                        }
                    }
                }
            });
            guard.active = Some(DiagnosticHold {
                id,
                plane,
                release_at,
                timer,
            });
            id
        };
        diagnostics.notify.notify_waiters();

        // Wait until hold `id` no longer occupies the slot (timer expiry,
        // release, or close). Dropping this future has no effect on the slot.
        loop {
            let notified = diagnostics.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let still_ours = diagnostics
                .hold
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .active
                .as_ref()
                .is_some_and(|h| h.id == id);
            if !still_ours {
                return Ok(());
            }
            notified.await;
        }
    }

    /// Release any active diagnostic hold. No-op if no hold is active.
    pub fn diagnostic_release(&self) {
        let Some(diagnostics) = self.inner.diagnostics.as_ref() else {
            return;
        };
        let taken = diagnostics
            .hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .take();
        if let Some(hold) = taken {
            // The slot is already cleared under the lock; aborting the alarm
            // only avoids a redundant wake (its id-guarded clear is a no-op).
            hold.timer.abort();
            diagnostics.notify.notify_waiters();
        }
    }

    /// Request host-local close (releasing any active hold) and await the
    /// shared completion boundary for up to 5000 ms. Returns
    /// `CloseTimeout { elapsed_ms: 5000 }` on timeout.
    pub async fn diagnostic_close(&self) -> Result<ProjectionShutdownOutcome, DiagnosticError> {
        self.diagnostic_release();
        start_shutdown(&self.inner);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(5000);
        loop {
            let notify = self.inner.completion_notify.notified();
            tokio::pin!(notify);
            notify.as_mut().enable();
            {
                let guard = self
                    .inner
                    .completion
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(outcome) = guard.outcome.clone() {
                    return Ok(outcome);
                }
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(DiagnosticError::CloseTimeout { elapsed_ms: 5000 });
            }
            let remaining = deadline - now;
            tokio::time::timeout(remaining, notify).await.ok();
        }
    }
}

/// Test-only crate-visible handles on the private default-applier barrier
/// ([`ApplierTestGate`]), so sibling modules (e.g. the diagnostics tool) can
/// stall the REAL applier mid-delivery and drive a live `CloseTimeout`
/// without wall-clock sleeps. Compiled out of production builds; the
/// production 5000 ms close contract is untouched.
#[cfg(test)]
impl ProjectionHost {
    /// Arm the barrier and return a future that resolves once the real
    /// default applier has entered it. The `Notified` is created before the
    /// caller triggers the next snapshot, so the wake cannot be missed.
    pub(crate) fn test_arm_applier_gate(&self) -> tokio::sync::futures::Notified<'_> {
        self.inner
            .applier_test_gate
            .hold
            .store(true, Ordering::SeqCst);
        self.inner.applier_test_gate.entered.notified()
    }

    /// Disarm the barrier and wake the stalled applier.
    pub(crate) fn test_release_applier_gate(&self) {
        self.inner
            .applier_test_gate
            .hold
            .store(false, Ordering::SeqCst);
        self.inner.applier_test_gate.release.notify_waiters();
    }

    /// Whether the shared completion boundary has recorded an outcome.
    pub(crate) fn test_completion_recorded(&self) -> bool {
        self.inner
            .completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outcome
            .is_some()
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

    /// Close this subscriber and remove it from the host fan-out map. There is
    /// no per-handle delivery task: the handle owns only a receipt queue, so
    /// close linearizes removal and explicitly discards that queue.
    pub async fn close(self) {
        let _publish = self
            .host
            .publish
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut map = self
            .host
            .consumers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.by_id.remove(&self.id);
        self.consumer.close_and_discard();
    }
}

impl Drop for ProjectionHandle {
    fn drop(&mut self) {
        if self.id == 0 {
            self.consumer.close_and_discard();
            return;
        }
        let _publish = self
            .host
            .publish
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut map = self
            .host
            .consumers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.by_id.remove(&self.id);
        self.consumer.close_and_discard();
    }
}

impl Drop for ProjectionHost {
    fn drop(&mut self) {
        request_local_close(&self.inner);
        // Drop cannot await, but when it occurs on a Tokio runtime it can
        // still install the same owned teardown boundary. Outside a runtime
        // the cancellation request remains valid and the weak workers exit;
        // a later explicit close waiter can perform the joins.
        if tokio::runtime::Handle::try_current().is_ok() {
            start_shutdown(&self.inner);
        }
    }
}

/// Request host-local shutdown at the publish linearization point. Attached
/// receipt queues are explicitly discarded; the default queue is retained so
/// the real applier can finish the event it already captured.
/// Suspend the calling worker while a diagnostic hold is active on `plane`.
/// Returns `true` if it waited (the caller re-checks from the loop top) and
/// `false` when there is nothing to wait for. Cancellation always bypasses
/// the hold so ordinary shutdown is never delayed by a diagnostic hold.
async fn wait_diagnostic_hold(
    inner: &Weak<HostInner>,
    plane: DiagnosticPlane,
    cancel: &CancellationToken,
) -> bool {
    if cancel.is_cancelled() {
        return false;
    }
    let Some(active) = inner.upgrade() else {
        return false;
    };
    let Some(diagnostics) = active.diagnostics.as_ref() else {
        return false;
    };
    // Arm the wakeup BEFORE reading the slot so a concurrent release/expiry
    // between the read and the await cannot be lost.
    let notify = diagnostics.notify.notified();
    tokio::pin!(notify);
    notify.as_mut().enable();
    let release_at = diagnostics
        .hold
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .active
        .as_ref()
        .filter(|h| h.plane == plane)
        .map(|h| h.release_at);
    let Some(release_at) = release_at else {
        return false;
    };
    if release_at <= tokio::time::Instant::now() {
        return false;
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {}
        _ = tokio::time::sleep_until(release_at) => {}
        _ = &mut notify => {}
    }
    true
}

fn request_local_close(inner: &Arc<HostInner>) {
    let _publish = inner
        .publish
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !inner.closed.swap(true, Ordering::SeqCst) {
        let consumers = inner
            .consumers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for consumer in consumers.by_id.values() {
            consumer.close_and_discard();
        }
        inner.default_consumer.mark_closed();
    }
    drop(_publish);
    inner.cancel.cancel();
}

/// Start the one explicit host teardown task. It owns both real worker joins;
/// close callers only observe the shared completion state and cannot detach
/// the joins by being cancelled.
fn start_shutdown(inner: &Arc<HostInner>) {
    request_local_close(inner);
    let should_spawn = {
        let mut state = inner
            .completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.started {
            false
        } else {
            state.started = true;
            true
        }
    };
    if should_spawn {
        let owned = Arc::clone(inner);
        tokio::spawn(async move {
            let source_task = owned
                .source_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let source_failed = match source_task {
                Some(task) => task.await.is_err(),
                None => false,
            };
            owned.default_consumer.mark_closed();
            let applier_task = owned
                .applier_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let applier_failed = match applier_task {
                Some(task) => task.await.is_err(),
                None => false,
            };
            let outcome = ProjectionShutdownOutcome {
                source_joined: !source_failed,
                applier_joined: !applier_failed,
                source_failed,
                applier_failed,
            };
            owned
                .completion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .outcome = Some(outcome);
            owned.completion_notify.notify_waiters();
        });
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
    inner: Weak<HostInner>,
    cancel: CancellationToken,
    mut registry_rx: broadcast::Receiver<RegistryChange>,
    mut ownership_rx: broadcast::Receiver<OwnershipChange>,
) {
    let Some(initial_inner) = inner.upgrade() else {
        return;
    };
    let close_signal = initial_inner.registry.close_signal();
    let already_closed = initial_inner.registry.is_closed();
    drop(initial_inner);
    let notified = close_signal.notified();
    tokio::pin!(notified);
    if already_closed {
        if let Some(inner) = inner.upgrade() {
            deliver_close(&inner);
        }
        return;
    }

    loop {
        // Diagnostic hold: pause source intake if the SourceIntake plane is held.
        if wait_diagnostic_hold(&inner, DiagnosticPlane::SourceIntake, &cancel).await {
            continue;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            _ = &mut notified => {
                if let Some(inner) = inner.upgrade() {
                    deliver_close(&inner);
                }
                return;
            }
            res = registry_rx.recv() => {
                match res {
                    Ok(change) => {
                        if let Some(inner) = inner.upgrade() {
                            process_registry_change(&inner, change);
                        } else {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(inner) = inner.upgrade() {
                            inner.lag_count.fetch_add(1, Ordering::Relaxed);
                            invalidate_all(&inner);
                        } else {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        if let Some(inner) = inner.upgrade() {
                            deliver_close(&inner);
                        }
                        return;
                    }
                }
            }
            res = ownership_rx.recv() => {
                match res {
                    Ok(_) => {
                        if let Some(inner) = inner.upgrade() {
                            invalidate_all(&inner);
                        } else {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(inner) = inner.upgrade() {
                            inner.lag_count.fetch_add(1, Ordering::Relaxed);
                            invalidate_all(&inner);
                        } else {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        if let Some(inner) = inner.upgrade() {
                            deliver_close(&inner);
                        }
                        return;
                    }
                }
            }
        }
    }
}

/// Fan out one registry change to every consumer.
fn process_registry_change(inner: &HostInner, change: RegistryChange) {
    let (cursor, cap_change) = inner.facade.map_registry_change(change);
    let _publish = inner
        .publish
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if inner.closed.load(Ordering::SeqCst) {
        return;
    }

    // Default consumer: apply a fresh snapshot so run-loop-readable state stays
    // current (the applier drains this queue into `applied`).
    if !inner.default_consumer.is_closed() {
        let snapshot = build_snapshot(inner, &inner.default_consumer.scope);
        if inner.default_consumer.enqueue_snapshot(snapshot) {
            inner.replacement_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    // Non-default consumers: deliver the mapped change iff in scope AND newer
    // than the consumer's last cursor; overflow replaces the backlog.
    let consumers = inner
        .consumers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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

/// Invalidate every consumer: enqueue `Invalidated` + a fresh snapshot. The
/// registry cursor is NOT advanced — only the registry advances it.
fn invalidate_all(inner: &HostInner) {
    let _publish = inner
        .publish
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if inner.closed.load(Ordering::SeqCst) {
        return;
    }

    if !inner.default_consumer.is_closed() {
        let snapshot = build_snapshot(inner, &inner.default_consumer.scope);
        // The DEFAULT consumer uses the SAME Invalidated + replacement Snapshot
        // semantics as attached consumers (not a bare Snapshot).
        inner.default_consumer.enqueue_invalidation(snapshot);
        inner.replacement_count.fetch_add(1, Ordering::Relaxed);
    }

    let consumers = inner
        .consumers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    let _publish = inner
        .publish
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if inner.closed.swap(true, Ordering::SeqCst) {
        return;
    }

    if !inner.default_consumer.is_closed() {
        let snapshot = build_snapshot(inner, &inner.default_consumer.scope);
        inner.default_consumer.enqueue_snapshot(snapshot);
        inner.default_consumer.mark_closed();
    }

    let consumers = inner
        .consumers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
/// each snapshot into `applied` (run-loop-readable state). `Change` is
/// meaningless for a snapshot-only consumer and ignored; `Invalidated` clears
/// the applied snapshot until its replacement arrives.
async fn applier_worker(
    default: Arc<ConsumerState>,
    inner: Weak<HostInner>,
    cancel: CancellationToken,
) {
    loop {
        // Diagnostic hold: pause delivery if the Delivery plane is held.
        if wait_diagnostic_hold(&inner, DiagnosticPlane::Delivery, &cancel).await {
            continue;
        }
        if let Some(event) = default.try_pop() {
            let Some(inner) = inner.upgrade() else { return };
            match event {
                ProjectionEvent::Snapshot(snapshot) => {
                    #[cfg(test)]
                    if inner.applier_test_gate.hold.load(Ordering::SeqCst) {
                        inner.applier_test_gate.entered.notify_waiters();
                        let released = inner.applier_test_gate.release.notified();
                        tokio::pin!(released);
                        while inner.applier_test_gate.hold.load(Ordering::SeqCst) {
                            released.as_mut().await;
                        }
                    }
                    {
                        let mut applied = inner
                            .applied
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        applied.snapshot = Some(snapshot.clone());
                        // Replacement receipt (observational only):
                        // write under the SAME lock as the snapshot install
                        // so a status reader cannot pair an old cursor /
                        // old tool-id set with the freshly applied snapshot
                        // (Ruling16: no two-cut pairing). Only counts
                        // Snapshots that follow a consumed Invalidated; a
                        // plain initial-mount Snapshot or a direct apply
                        // without a preceding Invalidated MUST NOT bump the
                        // replacement counter.
                        if applied.awaiting_replacement {
                            applied.applied_replacement_count += 1;
                            applied.last_replacement_registry_cursor =
                                Some(snapshot.registry_cursor);
                            applied.last_replacement_tool_ids =
                                snapshot.entries.keys().cloned().collect();
                            applied.awaiting_replacement = false;
                        }
                        if snapshot.registry_closed {
                            applied.closed = true;
                        }
                    }
                    inner.readiness.notify_one();
                }
                ProjectionEvent::Invalidated => {
                    // Fail closed: clear the applied snapshot so a stale lease
                    // is never served between invalidation and its replacement
                    // Snapshot.
                    {
                        let mut applied = inner
                            .applied
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        applied.snapshot = None;
                        // Observational receipt under the SAME lock cut as
                        // the snapshot clear, so the bumped counter / set
                        // gate and the cleared snapshot are observed by a
                        // status reader as one cut. `awaiting_replacement`
                        // is internal — it does NOT leak through the wire;
                        // it just gates whether the next consumed Snapshot
                        // counts as a replacement.
                        applied.applied_invalidation_count += 1;
                        applied.awaiting_replacement = true;
                    }
                    inner.readiness.notify_one();
                }
                ProjectionEvent::Change(_) => {}
            }
            continue;
        }
        let Some(inner) = inner.upgrade() else { return };
        if default.is_closed() {
            let mut applied = inner
                .applied
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            applied.closed = true;
            return;
        }
        drop(inner);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
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
    use crate::tools::descriptor::{
        ReplayPolicy, ToolCapabilityDescriptor, ToolKind, SCHEMA_VERSION,
    };
    use crate::tools::handlers::ToolHandler;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
    use async_trait::async_trait;
    use serde_json::Value;

    struct FakeHandler {
        name: String,
        description: String,
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
                description: self.description.clone(),
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
        fake_labeled(name, "")
    }

    /// A handler whose `description` (part of its `definition`) distinguishes
    /// one generation from another, so a test can prove handler + descriptor
    /// are frozen at ONE registry revision.
    fn fake_labeled(name: &str, description: &str) -> Arc<dyn ToolHandler> {
        Arc::new(FakeHandler {
            name: name.into(),
            description: description.into(),
            source: ToolSource::Builtin,
            idempotent: false,
        })
    }

    fn desc(name: &str) -> ToolCapabilityDescriptor {
        desc_labeled(name, "")
    }

    fn desc_labeled(name: &str, description: &str) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.into(),
            kind: ToolKind::Tool,
            schema_version: SCHEMA_VERSION,
            description: description.into(),
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

    /// Deterministically await until `current_snapshot()` satisfies `pred`,
    /// registering the private readiness `Notified` (re-checking the predicate
    /// after registration) — no sleep and no yield-based ordering guess.
    async fn await_snapshot_where(
        host: &ProjectionHost,
        pred: impl Fn(&HostSnapshot) -> bool,
    ) -> HostSnapshot {
        loop {
            if let Some(snap) = host.current_snapshot() {
                if pred(&snap) {
                    return snap;
                }
            }
            let ready = host.inner.readiness.notified();
            tokio::pin!(ready);
            if let Some(snap) = host.current_snapshot() {
                if pred(&snap) {
                    return snap;
                }
            }
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut ready)
                .await
                .expect("applier made no progress toward the awaited snapshot");
        }
    }

    /// RED (Ruling16): mount must NOT seed the publisher's own snapshot as
    /// already-applied. Before the default applier has run, `current_snapshot()`
    /// is `None` (fail-closed), never the initial snapshot masquerading as
    /// delivered state.
    #[tokio::test]
    async fn default_mount_does_not_fabricate_applied_snapshot() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));

        // No await has happened, so the default applier cannot have run yet.
        assert!(
            host.current_snapshot().is_none(),
            "mount must not present the publisher's own snapshot as delivered"
        );
    }

    /// RED (default replacement): the DEFAULT consumer must receive the same
    /// `Invalidated` + replacement `Snapshot` pair as attached consumers on
    /// invalidation — never a bare `Snapshot`.
    #[tokio::test]
    async fn default_invalidation_enqueues_invalidated_pair() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));

        // invalidate_all runs synchronously (no await), so the spawned applier
        // task has not polled and cannot drain the default queue.
        invalidate_all(&host.inner);

        let q = host
            .inner
            .default_consumer
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // After mount the queue held [Snapshot(initial)]; invalidate_all must
        // have appended the mandated Invalidated + replacement Snapshot pair.
        assert_eq!(
            q.items.len(),
            3,
            "default queue must be [initial Snapshot, Invalidated, replacement Snapshot]"
        );
        assert!(
            matches!(q.items[1], ProjectionEvent::Invalidated),
            "Invalidated must precede the replacement Snapshot"
        );
        assert!(
            matches!(q.items[2], ProjectionEvent::Snapshot(_)),
            "replacement Snapshot must follow Invalidated"
        );
    }

    /// RED (round2): ordinary default-consumer overflow on a registry change
    /// must atomically replace the stale pending backlog with `Invalidated` +
    /// a fresh replacement `Snapshot` (the SAME all-consumer invalidation
    /// contract `invalidate_all` already uses), never a bare `Snapshot` that
    /// silently drops the gap. Drives the REAL fan-out (`process_registry_change`)
    /// with no await, so on the current-thread runtime neither the source
    /// worker nor the default applier has polled; 64 registrations fill the
    /// default queue past its 64-slot bound (all under the 256-slot broadcast
    /// ring, so no lag confound). Then the REAL applier drains and applies the
    /// replacement (no direct applied assignment).
    #[tokio::test]
    async fn default_overflow_replaces_backlog_with_invalidated_pair() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("base"), fake("base")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));

        // Synchronous batch: register 64 tools and drive the real fan-out
        // directly (the same `process_registry_change` the source worker
        // invokes). No await => the default applier has not drained yet, so
        // the queue accumulates exactly one snapshot per registration.
        for i in 0..64 {
            let name = format!("t{i}");
            let description = format!("v{i}");
            reg.register(
                desc_labeled(&name, &description),
                fake_labeled(&name, &description),
            )
            .unwrap();
            process_registry_change(
                &host.inner,
                RegistryChange::Registered {
                    name,
                    revision: (i + 2) as u64,
                    source: ToolSource::Builtin,
                },
            );
        }

        // The default queue must now hold EXACTLY the mandated pair: the stale
        // pending backlog was atomically cleared, then `Invalidated` + the
        // fresh replacement `Snapshot` (not a bare `Snapshot`).
        let replacement = {
            let q = host
                .inner
                .default_consumer
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(
                q.items.len(),
                2,
                "overflow must clear the stale backlog and enqueue Invalidated + Snapshot"
            );
            assert!(
                matches!(q.items[0], ProjectionEvent::Invalidated),
                "overflow must lead with Invalidated"
            );
            match &q.items[1] {
                ProjectionEvent::Snapshot(s) => s.clone(),
                other => panic!("expected replacement snapshot, got {other:?}"),
            }
        };

        // The replacement reflects the CURRENT registry state: base + t0..t63
        // (65 entries) at the current cursor 65, with the just-registered t63
        // handler + descriptor frozen at ONE registry revision.
        assert_eq!(replacement.entries.len(), 65);
        assert_eq!(replacement.registry_cursor, Cursor(65));
        let entry = replacement.entries.get("t63").expect("t63 must be present");
        assert_eq!(
            entry.handler.definition().description,
            "v63",
            "current replacement handler"
        );
        assert_eq!(
            entry.descriptor.description, "v63",
            "current replacement descriptor"
        );
        assert_eq!(
            entry.descriptor.revision, 65,
            "current replacement revision"
        );

        // Let the REAL applier drain and apply the replacement (no direct
        // applied assignment). The source worker's re-processing of the 64
        // broadcasts also lands on the same final registry state (65 @
        // Cursor 65), so the final applied payload is deterministic.
        let applied = await_snapshot_where(&host, |s| s.entries.contains_key("t63")).await;
        assert_eq!(applied.entries.len(), 65);
        assert_eq!(applied.registry_cursor, Cursor(65));
        let entry = applied.entries.get("t63").expect("t63 must be applied");
        assert_eq!(
            entry.handler.definition().description,
            "v63",
            "applied handler"
        );
        assert_eq!(entry.descriptor.description, "v63", "applied descriptor");
    }

    #[tokio::test]
    async fn attach_receives_initial_snapshot_then_post_subscribe_change() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());

        // Mutate AFTER attach has linearized (the initial snapshot is already
        // enqueued) but BEFORE the first recv — the initial snapshot must cut
        // BEFORE this mutation, and the mutation must still be delivered (no
        // hole).
        reg.register(desc("b"), fake("b")).unwrap();

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => {
                assert_eq!(snap.entries.len(), 1, "initial cut predates the mutation");
                assert!(snap.entries.contains_key("a"));
                assert!(
                    !snap.entries.contains_key("b"),
                    "mutation must not leak into the initial cut"
                );
                assert_eq!(snap.registry_cursor, Cursor(1));
            }
            other => panic!("expected initial snapshot, got {other:?}"),
        }

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
        reg.register(desc_labeled("a", "v1"), fake_labeled("a", "v1"))
            .unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));

        let mut h1 = host.attach(Scope::default());
        let snap1 = match recv_bounded(&mut h1).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected snapshot, got {other:?}"),
        };
        let e1 = snap1.entries.get("a").unwrap();
        assert_eq!(e1.descriptor.revision, 1);
        assert_eq!(
            e1.handler.definition().description,
            "v1",
            "original handler"
        );
        assert_eq!(e1.descriptor.description, "v1", "original descriptor");

        // Replace with a DISTINGUISHABLE handler+descriptor (same name "a").
        reg.replace(desc_labeled("a", "v2"), fake_labeled("a", "v2"))
            .unwrap();

        let mut h2 = host.attach(Scope::default());
        let snap2 = match recv_bounded(&mut h2).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected snapshot, got {other:?}"),
        };
        let e2 = snap2.entries.get("a").unwrap();
        assert_eq!(e2.descriptor.revision, 2);
        assert_eq!(
            e2.handler.definition().description,
            "v2",
            "replacement handler"
        );
        assert_eq!(e2.descriptor.description, "v2", "replacement descriptor");

        // Negative control: each snapshot freezes handler AND descriptor at ONE
        // generation. A mixed pair (v1 handler + rev 2 descriptor, or vice
        // versa) would fail these consistency assertions.
        assert_eq!(snap1.entries.get("a").unwrap().descriptor.revision, 1);
        assert_eq!(
            snap1
                .entries
                .get("a")
                .unwrap()
                .handler
                .definition()
                .description,
            "v1"
        );
        assert_eq!(snap1.entries.get("a").unwrap().descriptor.description, "v1");
        assert_eq!(snap2.entries.get("a").unwrap().descriptor.revision, 2);
        assert_eq!(
            snap2
                .entries
                .get("a")
                .unwrap()
                .handler
                .definition()
                .description,
            "v2"
        );
        assert_eq!(snap2.entries.get("a").unwrap().descriptor.description, "v2");
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
                assert_eq!(
                    snap.entries.len(),
                    300,
                    "replacement snapshot must be complete"
                );
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
                assert_eq!(
                    snap.registry_cursor,
                    Cursor(1),
                    "registry revision must not advance"
                );
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

        assert!(
            tree.revoke(&tool_id("a")),
            "revoke must be a real transition"
        );

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Invalidated => {}
            other => panic!("expected Invalidated after revoke, got {other:?}"),
        }
        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => {
                assert_eq!(
                    snap.registry_cursor,
                    Cursor(1),
                    "registry revision must not advance"
                );
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

        assert!(
            tree.dispose(LifetimeScope::Runtime),
            "dispose must be a real transition"
        );

        match recv_bounded(&mut handle).await {
            ProjectionEvent::Invalidated => {}
            other => panic!("expected Invalidated after dispose, got {other:?}"),
        }
        match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snap) => {
                assert_eq!(
                    snap.registry_cursor,
                    Cursor(1),
                    "registry revision must not advance"
                );
                assert!(
                    !snap.entries.contains_key("a"),
                    "disposed id must be absent"
                );
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
        // Invalidated. After each fast receipt we acquire the `publish` mutex:
        // the source worker holds it across its whole fan-out, so acquiring it
        // proves the slow consumer's enqueue for THIS registration completed
        // before the next registration (an exact, not probabilistic, cut).
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
            // Explicit publish-boundary ack: fan-out for registration i is done.
            drop(
                host.inner
                    .publish
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }

        // The slow consumer overflowed at registration 65 (the 65th Change
        // would exceed capacity 64) and its backlog was atomically replaced.
        match recv_bounded(&mut slow).await {
            ProjectionEvent::Invalidated => {}
            other => panic!("slow consumer must overflow to Invalidated, got {other:?}"),
        }
        let snap = match recv_bounded(&mut slow).await {
            ProjectionEvent::Snapshot(snap) => snap,
            other => panic!("expected replacement snapshot, got {other:?}"),
        };
        // Exact cut: 65 registrations had completed when the 65th overflowed.
        assert_eq!(
            snap.entries.len(),
            65,
            "replacement snapshot is the cut at registration 65"
        );
        assert_eq!(snap.registry_cursor, Cursor(65));

        // The remaining 5 registrations (66..=70) arrive as deltas.
        let mut slow_remaining = 65usize;
        while slow_remaining < 70 {
            match recv_bounded(&mut slow).await {
                ProjectionEvent::Change(CapabilityChange::Registered { .. }) => slow_remaining += 1,
                other => panic!("expected registered delta for slow consumer, got {other:?}"),
            }
        }
        assert_eq!(
            slow_remaining, 70,
            "slow consumer reconstructs full membership via deltas"
        );

        assert_eq!(
            fast_changes, 70,
            "the fast consumer must receive every change"
        );
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

    /// Handle-local `close`: the handle owns only a receipt queue (no per-
    /// handle spawned delivery task), so `close` removes the consumer from the
    /// host fan-out map AND explicitly discards its pending queue via
    /// `close_and_discard`. `ProjectionHandle::close(self)` consumes the
    /// handle (so `recv` after `close` is not callable by design); the
    /// documented contract is verified by inspecting the host fan-out map:
    /// after `close`, the consumer is gone, and post-close registry
    /// mutations do not reach any other handle (the still-attached handle
    /// keeps receiving). The pending `queued` event sitting in the
    /// consumer's queue immediately before `close` is what
    /// `close_and_discard` discards — not a per-handle worker that
    /// drains it asynchronously.
    #[tokio::test]
    async fn handle_close_discards_pending_isolated_to_receipt() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut keep = host.attach(Scope::default());
        let mut closed = host.attach(Scope::default());

        // Drain initial snapshots from both attached handles.
        match recv_bounded(&mut keep).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected initial snapshot for keep, got {other:?}"),
        }
        match recv_bounded(&mut closed).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected initial snapshot for closed, got {other:?}"),
        }

        // Drive a change so a pending event sits in `closed`'s queue.
        reg.register(desc("queued"), fake("queued")).unwrap();
        // `keep` drains its queued change so the source-worker fan-out is
        // done (and the pending event for `closed` is enqueued) before we
        // exercise handle-local close.
        match recv_bounded(&mut keep).await {
            ProjectionEvent::Change(CapabilityChange::Registered { id, .. }) => {
                assert_eq!(id.name, "queued");
            }
            other => panic!("keep must drain its queued change, got {other:?}"),
        }

        // The fan-out map contains the `closed` consumer with the next
        // attach id; verify a pending event is sitting in its queue before
        // close (this is what `close_and_discard` must drop).
        let closed_id = {
            let map = host.inner.consumers.lock().unwrap();
            *map.by_id
                .keys()
                .max()
                .expect("closed consumer must be in map")
        };
        {
            let map = host.inner.consumers.lock().unwrap();
            let consumer = map.by_id.get(&closed_id).expect("closed consumer present");
            assert_eq!(
                consumer.queue.lock().unwrap().items.len(),
                1,
                "queued event must be pending in closed's receipt queue"
            );
        }

        // Handle-local close: removes the consumer from the fan-out map AND
        // discards the pending `queued` event the source worker enqueued.
        closed.close().await;

        // After close, the consumer is no longer in the fan-out map (no
        // per-handle delivery task ever carried the discarded event).
        {
            let map = host.inner.consumers.lock().unwrap();
            assert!(
                !map.by_id.contains_key(&closed_id),
                "close must remove the consumer from the fan-out map"
            );
        }

        // Mutate the registry: `keep` is still attached and still on the
        // fan-out map, so it must observe the new event. The closed
        // consumer is gone, so the source worker's fan-out skips it.
        reg.register(desc("after"), fake("after")).unwrap();
        match recv_bounded(&mut keep).await {
            ProjectionEvent::Change(CapabilityChange::Registered { id, .. }) => {
                assert_eq!(id.name, "after");
            }
            other => panic!("keep must receive the post-close mutation, got {other:?}"),
        }
    }

    /// Handle-local `cancel`: `cancel` only marks the receipt endpoint
    /// closed; the source worker observes `is_closed()` and skips enqueueing
    /// to it on subsequent fan-out, so `recv` returns `None` once the queue
    /// is drained. The other attached handle must keep receiving the
    /// post-cancel mutation. This test does NOT exercise a per-handle
    /// delivery task (none exists); the bounded wait proves the documented
    /// drain-then-None contract.
    #[tokio::test]
    async fn handle_cancel_drains_then_none_keeps_other_alive() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut keep = host.attach(Scope::default());
        let mut canceled = host.attach(Scope::default());

        // Drain initial snapshots from both attached handles.
        match recv_bounded(&mut keep).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected initial snapshot for keep, got {other:?}"),
        }
        match recv_bounded(&mut canceled).await {
            ProjectionEvent::Snapshot(_) => {}
            other => panic!("expected initial snapshot for canceled, got {other:?}"),
        }

        // Cancel one handle directly (host is still alive, no host close).
        canceled.cancel();

        // The canceled handle's queue is empty at this point (initial
        // snapshot already drained), so its `recv` must terminate with
        // `None` once the source worker observes the closed flag.
        let canceled_after =
            tokio::time::timeout(std::time::Duration::from_secs(1), canceled.recv())
                .await
                .expect("canceled handle did not terminate");
        assert!(
            canceled_after.is_none(),
            "canceled handle must return None, got {canceled_after:?}"
        );

        // Mutate the registry: the source worker's fan-out must skip the
        // closed consumer, but the other handle must still receive the
        // mutation.
        reg.register(desc("after"), fake("after")).unwrap();
        match recv_bounded(&mut keep).await {
            ProjectionEvent::Change(CapabilityChange::Registered { id, .. }) => {
                assert_eq!(id.name, "after");
            }
            other => panic!("keep must receive the post-cancel mutation, got {other:?}"),
        }
        let canceled_still =
            tokio::time::timeout(std::time::Duration::from_secs(1), canceled.recv())
                .await
                .expect("canceled handle still terminated after mutation");
        assert!(
            canceled_still.is_none(),
            "canceled handle must remain None after registry mutation"
        );
    }

    /// The DEFAULT applier — not a seeded publisher state — delivers the
    /// initial snapshot (Ruling16), and a closed source fails closed: no stale
    /// entries are returned even though the registry still holds its last
    /// snapshot's entries.
    #[tokio::test]
    async fn initial_delivery_via_real_applier_and_fails_closed() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));

        // Deterministic readiness: the REAL applier applies the enqueued
        // initial snapshot (no publisher-state seed).
        let snap = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;
        assert_eq!(snap.entries.len(), 1);
        assert_eq!(snap.registry_cursor, Cursor(1));

        // Close the source and give the applier a chance to observe it.
        host.clone().close_and_await().await;

        assert!(
            host.current_snapshot().is_none(),
            "closed source must fail closed"
        );
    }

    /// The applier clears the applied snapshot on `Invalidated` (fail-closed)
    /// rather than ignoring it: a stale lease must never be served between
    /// invalidation and its replacement snapshot. A bare Invalidated (no
    /// replacement) is the only way to observe the clear, because the
    /// production path pairs Invalidated + replacement Snapshot in one atomic
    /// enqueue the applier drains back-to-back.
    #[tokio::test]
    async fn applier_clears_applied_on_invalidated() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));

        // Initial applied via the real applier.
        let snap = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;
        assert_eq!(snap.registry_cursor, Cursor(1));

        // Enqueue a BARE Invalidated (no replacement snapshot) directly into
        // the default delivery queue.
        {
            let mut q = host
                .inner
                .default_consumer
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            q.items.push_back(ProjectionEvent::Invalidated);
        }
        host.inner.default_consumer.wake.notify_one();

        // Deterministically await the applier observing the Invalidated.
        loop {
            if host.current_snapshot().is_none() {
                break;
            }
            let ready = host.inner.readiness.notified();
            tokio::pin!(ready);
            if host.current_snapshot().is_none() {
                break;
            }
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut ready)
                .await
                .expect("applier must clear applied on Invalidated");
        }
        assert!(
            host.current_snapshot().is_none(),
            "Invalidated must clear the applied snapshot (fail-closed)"
        );
    }

    /// The DEFAULT consumer's owner-invalidation path goes through the real
    /// queue AND the real applier: after `revoke`, the applied state is the
    /// replacement snapshot without the revoked id (not publisher state, not an
    /// attached handle's queue).
    #[tokio::test]
    async fn default_owner_invalidation_replaces_applied_state() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));

        let initial = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;
        assert_eq!(initial.registry_cursor, Cursor(1));

        // Real ownership invalidation (not a registry mutation).
        assert!(
            tree.revoke(&tool_id("a")),
            "revoke must be a real transition"
        );

        // Await the replacement snapshot applied by the real applier.
        let replacement = await_snapshot_where(&host, |s| !s.entries.contains_key("a")).await;
        assert_eq!(
            replacement.registry_cursor,
            Cursor(1),
            "owner invalidation must not advance the registry cursor"
        );
        assert!(replacement.entries.is_empty(), "revoked id must be absent");
    }

    #[tokio::test]
    async fn cancel_awaits_inflight_completion() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let _ = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;

        host.inner
            .applier_test_gate
            .hold
            .store(true, Ordering::SeqCst);
        let entered = host.inner.applier_test_gate.entered.notified();
        tokio::pin!(entered);
        reg.register(desc("b"), fake("b")).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), &mut entered)
            .await
            .expect("real default applier did not enter the barrier");

        start_shutdown(&host.inner);
        assert!(
            host.inner
                .completion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .outcome
                .is_none(),
            "shutdown cannot report completion while captured delivery is held"
        );

        host.inner
            .applier_test_gate
            .hold
            .store(false, Ordering::SeqCst);
        host.inner.applier_test_gate.release.notify_waiters();
        let outcome = host.clone().close_and_await().await;
        assert_eq!(
            outcome,
            ProjectionShutdownOutcome {
                source_joined: true,
                applier_joined: true,
                source_failed: false,
                applier_failed: false,
            }
        );
        assert!(host.current_snapshot().is_none());
    }

    #[tokio::test]
    async fn idle_close_wakeup_joins_both_workers() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let _ = await_snapshot_where(&host, |s| s.registry_cursor == Cursor(0)).await;

        reg.close();
        let outcome = host.clone().close_and_await().await;
        assert!(outcome.source_joined);
        assert!(outcome.applier_joined);
        assert!(!outcome.source_failed && !outcome.applier_failed);
    }

    #[tokio::test]
    async fn post_close_mutation_is_not_delivered() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());
        assert!(matches!(
            recv_bounded(&mut handle).await,
            ProjectionEvent::Snapshot(_)
        ));

        let outcome = host.clone().close_and_await().await;
        assert!(outcome.source_joined && outcome.applier_joined);
        reg.register(desc("after"), fake("after")).unwrap();
        tree.bump(LifetimeScope::Runtime);

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), handle.recv())
            .await
            .expect("closed handle did not terminate")
            .is_some();
        assert!(
            !received,
            "post-close authority mutations reached the handle"
        );
        assert!(
            reg.revision() >= 2,
            "host-local close must not close the registry"
        );
    }

    #[tokio::test]
    async fn dropping_last_host_requests_teardown() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());
        assert!(matches!(
            recv_bounded(&mut handle).await,
            ProjectionEvent::Snapshot(_)
        ));

        drop(host);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), handle.recv())
                .await
                .expect("dropped host did not close the receipt")
                .is_none()
        );
        assert!(
            !reg.is_closed(),
            "dropping a host must not close the registry"
        );
    }

    #[tokio::test]
    async fn registry_close_is_not_false_quiescence() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());
        assert!(matches!(
            recv_bounded(&mut handle).await,
            ProjectionEvent::Snapshot(_)
        ));

        // Keep the authority sender alive: registry close is a bit + wakeup,
        // not broadcast sender closure and not itself host quiescence.
        reg.close();
        let outcome = host.clone().close_and_await().await;
        assert!(outcome.source_joined);
        assert!(outcome.applier_joined);
        assert!(host.current_snapshot().is_none());
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), handle.recv())
                .await
                .expect("closed receipt did not terminate")
                .is_none()
        );
    }

    #[tokio::test]
    async fn concurrent_close_is_idempotent() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        let first = tokio::spawn(Arc::clone(&host).close_and_await());
        let second = tokio::spawn(Arc::clone(&host).close_and_await());
        let first = first.await.unwrap();
        let second = second.await.unwrap();
        assert_eq!(first, second);
        assert!(first.source_joined && first.applier_joined);
    }

    #[tokio::test]
    async fn cancelled_first_waiter_does_not_detach_teardown() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        start_shutdown(&host.inner);
        let first = tokio::spawn(Arc::clone(&host).close_and_await());
        first.abort();

        let outcome = host.close_and_await().await;
        assert!(outcome.source_joined && outcome.applier_joined);
        assert!(!outcome.source_failed && !outcome.applier_failed);
    }

    #[tokio::test]
    async fn already_running_invocation_is_not_cut() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let mut handle = host.attach(Scope::default());
        let snapshot = match recv_bounded(&mut handle).await {
            ProjectionEvent::Snapshot(snapshot) => snapshot,
            other => panic!("expected initial snapshot, got {other:?}"),
        };
        let captured = snapshot.entries.get("a").unwrap().handler.clone();
        assert!(tree.revoke(&tool_id("a")));

        let output = captured.invoke(serde_json::json!({})).await.unwrap();
        assert_eq!(output.value["tool"], "a");
    }

    // ---- Diagnostic hold / close (Task1 fix round) ----

    /// Observed plane of the host-owned hold slot (None = no active hold).
    fn active_hold_plane(host: &ProjectionHost) -> Option<DiagnosticPlane> {
        host.inner
            .diagnostics
            .as_ref()?
            .hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .as_ref()
            .map(|h| h.plane)
    }

    /// Await until the hold slot satisfies `pred`, registering the hold
    /// `Notified` before re-checking the slot (no sleep-as-proof). Bounded.
    async fn await_hold_slot(
        host: &ProjectionHost,
        bound: std::time::Duration,
        pred: impl Fn(Option<DiagnosticPlane>) -> bool,
    ) -> Option<DiagnosticPlane> {
        tokio::time::timeout(bound, async {
            loop {
                let notified = host
                    .inner
                    .diagnostics
                    .as_ref()
                    .expect("enabled test host must have diagnostics")
                    .notify
                    .notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let observed = active_hold_plane(host);
                if pred(observed) {
                    return observed;
                }
                notified.await;
            }
        })
        .await
        .expect("hold slot never reached the expected state within bound")
    }

    #[tokio::test(start_paused = true)]
    async fn dropped_hold_future_is_released_by_host_timer() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let _ = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;

        let holder = Arc::clone(&host);
        let task = tokio::spawn(async move {
            holder
                .diagnostic_hold(
                    DiagnosticPlane::Delivery,
                    std::time::Duration::from_millis(200),
                )
                .await
        });
        let observed = await_hold_slot(&host, std::time::Duration::from_millis(100), |p| {
            p == Some(DiagnosticPlane::Delivery)
        })
        .await;
        assert_eq!(observed, Some(DiagnosticPlane::Delivery));

        // Caller disconnects: the holding future is dropped without release.
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            active_hold_plane(&host),
            Some(DiagnosticPlane::Delivery),
            "dropping the caller must not be what clears the slot"
        );

        // The host-owned timer must clear the slot at expiry (bounded wait).
        let cleared = await_hold_slot(&host, std::time::Duration::from_millis(1000), |p| {
            p.is_none()
        })
        .await;
        assert_eq!(cleared, None);

        // A second hold is accepted after the timer released the first.
        host.diagnostic_hold(
            DiagnosticPlane::SourceIntake,
            std::time::Duration::from_millis(1),
        )
        .await
        .expect("second hold must succeed after timer expiry");
    }

    #[tokio::test(start_paused = true)]
    async fn stale_timer_does_not_clear_newer_hold() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        let _ = await_snapshot_where(&host, |_| true).await;

        let first = Arc::clone(&host);
        let first_task = tokio::spawn(async move {
            first
                .diagnostic_hold(
                    DiagnosticPlane::SourceIntake,
                    std::time::Duration::from_millis(100),
                )
                .await
        });
        await_hold_slot(&host, std::time::Duration::from_millis(50), |p| {
            p == Some(DiagnosticPlane::SourceIntake)
        })
        .await;
        host.diagnostic_release();
        first_task.await.unwrap().unwrap();

        let second = Arc::clone(&host);
        let second_task = tokio::spawn(async move {
            second
                .diagnostic_hold(
                    DiagnosticPlane::Delivery,
                    std::time::Duration::from_millis(1000),
                )
                .await
        });
        await_hold_slot(&host, std::time::Duration::from_millis(50), |p| {
            p == Some(DiagnosticPlane::Delivery)
        })
        .await;

        // Pass the first hold's original expiry; its timer must not clear the
        // newer hold.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(active_hold_plane(&host), Some(DiagnosticPlane::Delivery));

        let cleared = await_hold_slot(&host, std::time::Duration::from_millis(2000), |p| {
            p.is_none()
        })
        .await;
        assert_eq!(cleared, None);
        second_task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn hold_rejects_out_of_range_and_overlapping_requests() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        let _ = await_snapshot_where(&host, |_| true).await;

        for bad in [0u64, 5001] {
            let err = host
                .diagnostic_hold(
                    DiagnosticPlane::Delivery,
                    std::time::Duration::from_millis(bad),
                )
                .await
                .unwrap_err();
            assert_eq!(err, DiagnosticError::InvalidHoldDuration, "duration {bad}");
            assert_eq!(
                active_hold_plane(&host),
                None,
                "rejected hold must not install"
            );
        }

        let holder = Arc::clone(&host);
        let task = tokio::spawn(async move {
            holder
                .diagnostic_hold(
                    DiagnosticPlane::Delivery,
                    std::time::Duration::from_millis(5000),
                )
                .await
        });
        await_hold_slot(&host, std::time::Duration::from_millis(50), |p| p.is_some()).await;
        let overlap = host
            .diagnostic_hold(
                DiagnosticPlane::SourceIntake,
                std::time::Duration::from_millis(1),
            )
            .await
            .unwrap_err();
        assert_eq!(overlap, DiagnosticError::HoldAlreadyActive);
        assert_eq!(
            active_hold_plane(&host),
            Some(DiagnosticPlane::Delivery),
            "overlap rejection must not replace the active hold"
        );

        host.diagnostic_release();
        task.await.unwrap().unwrap();
        assert_eq!(active_hold_plane(&host), None);
    }

    #[tokio::test(start_paused = true)]
    async fn delivery_hold_overflow_queues_invalidated_before_replacement() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount_with_capacity(reg.clone(), Arc::clone(&tree), 2);
        let _ = await_snapshot_where(&host, |_| true).await;

        let holder = Arc::clone(&host);
        let task = tokio::spawn(async move {
            holder
                .diagnostic_hold(
                    DiagnosticPlane::Delivery,
                    std::time::Duration::from_millis(5000),
                )
                .await
        });
        await_hold_slot(&host, std::time::Duration::from_millis(50), |p| {
            p == Some(DiagnosticPlane::Delivery)
        })
        .await;
        for i in 0..10 {
            let n = format!("t{i}");
            reg.register(desc(&n), fake(&n)).unwrap();
        }

        // Observe the real default queue while delivery is held: the source
        // keeps running, overflow replaced the stale backlog, and the queue
        // head is `Invalidated` followed by the final-membership replacement.
        tokio::time::timeout(std::time::Duration::from_millis(1000), async {
            loop {
                {
                    let q = host
                        .inner
                        .default_consumer
                        .queue
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let last_full = matches!(
                        q.items.back(),
                        Some(ProjectionEvent::Snapshot(s)) if s.entries.len() == 10
                    );
                    if last_full {
                        assert!(
                            matches!(q.items.front(), Some(ProjectionEvent::Invalidated)),
                            "overflow must place Invalidated before the replacement"
                        );
                        assert!(q.items.len() <= 2, "capacity must not be resized");
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("source worker never queued the final replacement");
        assert!(host
            .current_snapshot()
            .map_or(true, |s| s.entries.len() < 10));
        assert!(host.diagnostic_status().replacement_count > 0);

        host.diagnostic_release();
        task.await.unwrap().unwrap();
        let applied = await_snapshot_where(&host, |s| s.entries.len() == 10).await;
        assert_eq!(applied.entries.len(), 10);
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_bypasses_active_hold() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let _ = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;

        let holder = Arc::clone(&host);
        let _task = tokio::spawn(async move {
            holder
                .diagnostic_hold(
                    DiagnosticPlane::SourceIntake,
                    std::time::Duration::from_millis(5000),
                )
                .await
        });
        await_hold_slot(&host, std::time::Duration::from_millis(50), |p| p.is_some()).await;

        // Ordinary (non-diagnostic) close must not wait out the hold.
        let started = tokio::time::Instant::now();
        let outcome = Arc::clone(&host).close_and_await().await;
        assert!(outcome.source_joined && outcome.applier_joined);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(5000),
            "cancellation must bypass the hold, waited {:?}",
            started.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn diagnostic_close_timeout_stays_fail_closed() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let _ = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;

        // Stall the real applier mid-delivery so the shared completion
        // boundary cannot be reached.
        host.inner
            .applier_test_gate
            .hold
            .store(true, Ordering::SeqCst);
        let entered = host.inner.applier_test_gate.entered.notified();
        tokio::pin!(entered);
        reg.register(desc("b"), fake("b")).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), &mut entered)
            .await
            .expect("real default applier did not enter the barrier");

        let started = tokio::time::Instant::now();
        let err = host.diagnostic_close().await.unwrap_err();
        assert_eq!(err, DiagnosticError::CloseTimeout { elapsed_ms: 5000 });
        assert!(started.elapsed() <= std::time::Duration::from_millis(5001));
        assert!(
            host.is_closing(),
            "timeout must leave the host close-requested"
        );
        assert_eq!(
            host.diagnostic_status().lifecycle,
            DiagnosticLifecycle::Closing
        );
        assert!(
            host.inner
                .completion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .outcome
                .is_none(),
            "timeout must not fabricate completion"
        );
        // Shared authority is not closed by the projection close.
        reg.register(desc("c"), fake("c")).unwrap();

        // The retained joins still complete through the same boundary.
        host.inner
            .applier_test_gate
            .hold
            .store(false, Ordering::SeqCst);
        host.inner.applier_test_gate.release.notify_waiters();
        let outcome = Arc::clone(&host).close_and_await().await;
        assert!(outcome.source_joined && outcome.applier_joined);
        assert_eq!(
            host.diagnostic_status().lifecycle,
            DiagnosticLifecycle::Closed
        );
    }

    #[tokio::test]
    async fn disabled_mount_has_no_diagnostic_hold_path() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount_with_diagnostics(reg.clone(), Arc::clone(&tree), false);

        let err = host
            .diagnostic_hold(
                DiagnosticPlane::Delivery,
                std::time::Duration::from_millis(10),
            )
            .await
            .unwrap_err();
        assert_eq!(err, DiagnosticError::Closed);

        reg.register(desc("ordinary"), fake("ordinary")).unwrap();
        let snapshot =
            await_snapshot_where(&host, |snapshot| snapshot.entries.contains_key("ordinary")).await;
        assert!(snapshot.entries.contains_key("ordinary"));
        let _ = Arc::clone(&host).close_and_await().await;
    }

    #[tokio::test]
    async fn hold_on_closed_host_is_rejected() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        let _ = Arc::clone(&host).close_and_await().await;
        let err = host
            .diagnostic_hold(
                DiagnosticPlane::Delivery,
                std::time::Duration::from_millis(10),
            )
            .await
            .unwrap_err();
        assert_eq!(err, DiagnosticError::Closed);
        assert_eq!(active_hold_plane(&host), None);
    }

    /// Deterministically await until `diagnostic_status()` satisfies `pred`,
    /// registering the private readiness `Notified` (re-checking the predicate
    /// after registration) -- no sleep and no yield-based ordering guess.
    async fn await_status_where(
        host: &ProjectionHost,
        pred: impl Fn(&DiagnosticStatus) -> bool,
    ) -> DiagnosticStatus {
        loop {
            let s = host.diagnostic_status();
            if pred(&s) {
                return s;
            }
            let ready = host.inner.readiness.notified();
            tokio::pin!(ready);
            let s = host.diagnostic_status();
            if pred(&s) {
                return s;
            }
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut ready)
                .await
                .expect("applier made no progress toward the awaited status");
        }
    }

    /// RED: a plain initial-mount Snapshot is NOT a replacement (no preceding
    /// `Invalidated` cleared the slot). After mount, the applier applies the
    /// initial Snapshot with `awaiting_replacement == false`, so the receipt
    /// counters MUST stay at zero and `last_replacement_*` MUST stay empty.
    #[tokio::test]
    async fn applied_replacement_count_starts_zero_after_mount() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let _ = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;

        let status = host.diagnostic_status();
        assert_eq!(status.applied_replacement_count, 0);
        assert_eq!(status.applied_invalidation_count, 0);
        assert_eq!(status.last_replacement_registry_cursor, None);
        assert!(status.last_replacement_tool_ids.is_empty());
    }

    /// RED: a consumed `Invalidated` event with NO follow-up replacement
    /// MUST bump `applied_invalidation_count` but MUST NOT touch the
    /// replacement receipts (count, cursor, tool ids). The applied snapshot
    /// is cleared (fail-closed contract).
    #[tokio::test]
    async fn bare_invalidated_bumps_invalidation_count_only() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let _ = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;
        assert!(host.current_snapshot().is_some());

        // Push JUST an `Invalidated` (no replacement Snapshot) onto the
        // default consumer's queue and wake the applier. We bypass
        // `invalidate_all` deliberately: that helper enqueues the mandated
        // Invalidated + replacement Snapshot PAIR. This test isolates the
        // bare-Invalidated branch of the applier.
        {
            let mut q = host
                .inner
                .default_consumer
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            q.items.push_back(ProjectionEvent::Invalidated);
        }
        host.inner.default_consumer.wake.notify_one();

        let status = await_status_where(&host, |s| s.applied_invalidation_count > 0).await;
        assert_eq!(status.applied_invalidation_count, 1);
        assert_eq!(status.applied_replacement_count, 0);
        assert_eq!(status.last_replacement_registry_cursor, None);
        assert!(status.last_replacement_tool_ids.is_empty());
        assert!(
            host.current_snapshot().is_none(),
            "bare Invalidated must clear applied snapshot (fail-closed)"
        );
    }

    /// RED: an `Invalidated` + replacement `Snapshot` pair (the production
    /// `invalidate_all` contract) MUST bump BOTH counters and record the
    /// replacement snapshot's cursor + tool ids in the same lock cut. No
    /// sleep: deterministic via the readiness notifier pattern. The cursor
    /// is the one carried by the replacement snapshot; `invalidate_all`
    /// does NOT advance the registry cursor (only a registry change does),
    /// so the assertion checks the cursor is recorded verbatim, not that it
    /// advanced.
    #[tokio::test]
    async fn invalidated_pair_bumps_both_counts_and_records_replacement() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let initial = await_snapshot_where(&host, |s| s.entries.contains_key("a")).await;
        let initial_cursor = initial.registry_cursor;

        // Production invalidation contract: appends Invalidated + a fresh
        // replacement Snapshot (synchronously, before the applier drains).
        invalidate_all(&host.inner);

        let status = await_status_where(&host, |s| s.applied_replacement_count > 0).await;
        assert_eq!(status.applied_invalidation_count, 1);
        assert_eq!(status.applied_replacement_count, 1);
        let cursor = status
            .last_replacement_registry_cursor
            .expect("replacement cursor must be set after pair");
        assert_eq!(
            cursor, initial_cursor,
            "invalidate_all does NOT advance the registry cursor; the              recorded cursor is the one carried by the replacement snapshot"
        );
        assert!(
            status.last_replacement_tool_ids.iter().any(|n| n == "a"),
            "replacement must carry the registered tool name"
        );
    }
}
