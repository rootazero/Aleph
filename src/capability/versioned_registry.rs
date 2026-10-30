//! Core-only immutable publication primitive for versioned state.

use std::cell::RefCell;

use crate::sync_primitives::Arc;
use arc_swap::ArcSwap;
use tokio::sync::{broadcast, Notify};

thread_local! {
    /// Registries whose `transact` / `close` is currently running on this
    /// thread, identified by the address of the registry itself.
    ///
    /// The mutation lock is NOT reentrant: a nested call on the same registry
    /// would wait for a lock its own thread already holds, and that wait can
    /// never be cancelled. Tracking the active registries lets the nested
    /// call fail fast instead of hanging the thread.
    static ACTIVE_REGISTRIES: RefCell<Vec<usize>> = RefCell::new(Vec::new());
}

/// Fail-fast guard against re-entering one registry on one thread.
///
/// Entering the guard aborts the call when this thread is already inside the
/// same registry. The aborted call publishes nothing and leaves the outer
/// transaction's state untouched; the panic unwinds through the outer
/// closure, and the poisoned mutation lock is recovered by the same
/// `PoisonError::into_inner` path every other panic uses, so the registry
/// stays usable afterwards.
struct ReentrancyGuard {
    registry: usize,
}

impl ReentrancyGuard {
    fn enter(registry: usize) -> Self {
        ACTIVE_REGISTRIES.with(|active| {
            let already_active = active.borrow().contains(&registry);
            assert!(
                !already_active,
                "versioned registry re-entrancy: a transaction closure must not call transact() \
                 or close() on the same registry (the mutation lock is not reentrant)"
            );
            active.borrow_mut().push(registry);
        });
        Self { registry }
    }
}

impl Drop for ReentrancyGuard {
    fn drop(&mut self) {
        ACTIVE_REGISTRIES.with(|active| {
            let mut active = active.borrow_mut();
            if let Some(position) = active.iter().rposition(|entry| *entry == self.registry) {
                active.remove(position);
            }
        });
    }
}

/// One immutable state generation.
#[derive(Clone)]
pub(crate) struct VersionedSnapshot<T> {
    pub(crate) value: T,
    pub(crate) revision: u64,
    pub(crate) closed: bool,
}

/// Atomic snapshots plus ordered, notification-only publication.
///
/// # Re-entrancy
///
/// `transact` and `close` serialise every mutation through ONE non-reentrant
/// mutex, and both run caller code while that mutex is held. A transaction
/// closure MUST NOT call `transact` or `close` on the SAME registry: the
/// nested call would wait for a lock its own thread already holds, and that
/// wait can never be cancelled. The registry detects the re-entry and aborts
/// the nested call immediately instead of deadlocking; the aborted call
/// publishes nothing and the registry stays usable afterwards. Transacting a
/// DIFFERENT registry from inside a closure is allowed (each registry owns
/// its own lock), but the caller owns the lock ordering between the two.
pub(crate) struct VersionedRegistry<T, E = (), L = ()>
where
    E: Clone,
    L: Clone,
{
    snapshot: ArcSwap<VersionedSnapshot<T>>,
    events: broadcast::Sender<E>,
    projected_events: broadcast::Sender<L>,
    project: fn(&E) -> Vec<L>,
    mutation_lock: std::sync::Mutex<()>,
    close_signal: Arc<Notify>,
}

impl<T, E> VersionedRegistry<T, E, ()>
where
    E: Clone,
{
    pub(crate) fn new(value: T) -> Self {
        Self::from_snapshot(VersionedSnapshot {
            value,
            revision: 0,
            closed: false,
        })
    }

    /// Explicit initial generation for frozen fixtures; never re-wrap their values.
    pub(crate) fn from_snapshot(snapshot: VersionedSnapshot<T>) -> Self {
        Self::from_snapshot_with_projection(snapshot, |_| Vec::new())
    }
}

impl<T, E, L> VersionedRegistry<T, E, L>
where
    E: Clone,
    L: Clone,
{
    pub(crate) fn new_with_projection(value: T, project: fn(&E) -> Vec<L>) -> Self {
        Self::from_snapshot_with_projection(
            VersionedSnapshot {
                value,
                revision: 0,
                closed: false,
            },
            project,
        )
    }

    /// Explicit initial generation for frozen fixtures; never re-wrap their values.
    pub(crate) fn from_snapshot_with_projection(
        snapshot: VersionedSnapshot<T>,
        project: fn(&E) -> Vec<L>,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        let (projected_events, _) = broadcast::channel(256);
        Self {
            snapshot: ArcSwap::from_pointee(snapshot),
            events,
            projected_events,
            project,
            mutation_lock: std::sync::Mutex::new(()),
            close_signal: Arc::new(Notify::new()),
        }
    }

    #[must_use]
    pub(crate) fn snapshot(&self) -> Arc<VersionedSnapshot<T>> {
        self.snapshot.load_full()
    }

    #[must_use]
    pub(crate) fn revision(&self) -> u64 {
        self.snapshot.load().revision
    }

    #[must_use]
    pub(crate) fn is_closed(&self) -> bool {
        self.snapshot.load().closed
    }

    #[must_use]
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<E> {
        self.events.subscribe()
    }

    #[must_use]
    pub(crate) fn subscribe_projected(&self) -> broadcast::Receiver<L> {
        self.projected_events.subscribe()
    }

    #[must_use]
    pub(crate) fn close_signal(&self) -> Arc<Notify> {
        Arc::clone(&self.close_signal)
    }

    /// Load once and invoke once under the publication mutex. A no-op or error
    /// preserves the current Arc and emits neither stream. The synchronous sends
    /// stay under the same lock so event order equals revision order. Poison
    /// recovery is safe because the mutex guards only a unit token, never mutable
    /// state.
    ///
    /// # Re-entrancy
    ///
    /// `f` runs while the (non-reentrant) mutation lock is held, so `f` MUST
    /// NOT call `transact` or `close` on THIS registry: the nested call would
    /// wait for a lock its own thread already holds and hang the thread
    /// forever. Such a call is refused immediately (it aborts with a clear
    /// message instead of deadlocking), publishes nothing, and leaves the
    /// outer transaction's state untouched.
    ///
    /// # Revision exhaustion
    ///
    /// The revision counter is checked: when it cannot advance, the call
    /// aborts instead of silently reusing an already-published revision.
    pub(crate) fn transact<R, Err>(
        &self,
        f: impl FnOnce(&VersionedSnapshot<T>, u64) -> Result<Option<(T, E, R)>, Err>,
    ) -> Result<Option<R>, Err> {
        let _reentrancy = ReentrancyGuard::enter(std::ptr::from_ref(self) as usize);
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.snapshot.load_full();
        let Some(next_revision) = current.revision.checked_add(1) else {
            panic!(
                "versioned registry revision counter exhausted: the sequence cannot continue past \
                 u64::MAX without reusing an already-published revision"
            );
        };
        let Some((value, event, result)) = f(&current, next_revision)? else {
            return Ok(None);
        };
        let projected = (self.project)(&event);
        self.snapshot.store(Arc::new(VersionedSnapshot {
            value,
            revision: next_revision,
            closed: current.closed,
        }));
        let _ = self.events.send(event);
        for projected_event in projected {
            let _ = self.projected_events.send(projected_event);
        }
        Ok(Some(result))
    }

    /// Publish the closed bit at the same revision before waking all observers.
    /// Observers create `notified()` before checking `is_closed()` to avoid a
    /// missed wake. Close carries no event on either stream and does not reject
    /// transactions: each shell retains its own teardown and admission rules.
    ///
    /// # Re-entrancy
    ///
    /// `close` takes the same non-reentrant mutation lock as `transact`, so it
    /// MUST NOT be called from inside a `transact` closure on this registry:
    /// the nested call would wait for a lock its own thread already holds. The
    /// re-entry is refused immediately instead of deadlocking.
    ///
    /// The closed bit lives INSIDE the published snapshot, so publishing it
    /// re-publishes the value (`T: Clone`): closing a registry that holds a
    /// large catalog copies that catalog once. A consumer that captured
    /// `snapshot()` before the close keeps observing that older snapshot
    /// (`closed == false`); `is_closed()` is the registry-level answer.
    pub(crate) fn close(&self)
    where
        T: Clone,
    {
        let _reentrancy = ReentrancyGuard::enter(std::ptr::from_ref(self) as usize);
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.snapshot.load_full();
        if !current.closed {
            self.snapshot.store(Arc::new(VersionedSnapshot {
                value: current.value.clone(),
                revision: current.revision,
                closed: true,
            }));
        }
        self.close_signal.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct ProjectionEvent {
        revision: u64,
        values: Vec<u64>,
    }

    fn project_values(event: &ProjectionEvent) -> Vec<(u64, u64)> {
        event
            .values
            .iter()
            .copied()
            .map(|value| (event.revision, value))
            .collect()
    }

    static PROJECTOR_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn counting_projector(revision: &u64) -> Vec<u64> {
        PROJECTOR_CALLS.fetch_add(1, Ordering::SeqCst);
        vec![*revision]
    }

    static PANIC_PROJECTOR_ONCE: AtomicBool = AtomicBool::new(false);

    fn panic_once_projector(revision: &u64) -> Vec<u64> {
        if PANIC_PROJECTOR_ONCE.swap(false, Ordering::SeqCst) {
            panic!("intentional projector panic");
        }
        vec![*revision]
    }

    fn project_group(revision: &u64) -> Vec<(u64, u64)> {
        vec![(*revision, 0), (*revision, 1), (*revision, 2)]
    }

    fn project_single_revision(revision: &u64) -> Vec<u64> {
        vec![*revision]
    }

    #[test]
    fn projected_success_publishes_primary_and_ordered_values() {
        let registry =
            VersionedRegistry::<Vec<u64>, ProjectionEvent, (u64, u64)>::new_with_projection(
                Vec::new(),
                project_values,
            );
        let mut primary = registry.subscribe();
        let mut projected = registry.subscribe_projected();

        registry
            .transact(|current, revision| {
                let mut value = current.value.clone();
                value.extend([10, 20, 30]);
                Ok::<_, ()>(Some((
                    value,
                    ProjectionEvent {
                        revision,
                        values: vec![10, 20, 30],
                    },
                    (),
                )))
            })
            .unwrap();
        assert_eq!(
            primary.try_recv().unwrap(),
            ProjectionEvent {
                revision: 1,
                values: vec![10, 20, 30],
            }
        );
        assert_eq!(projected.try_recv().unwrap(), (1, 10));
        assert_eq!(projected.try_recv().unwrap(), (1, 20));
        assert_eq!(projected.try_recv().unwrap(), (1, 30));

        registry
            .transact(|current, revision| {
                Ok::<_, ()>(Some((
                    current.value.clone(),
                    ProjectionEvent {
                        revision,
                        values: Vec::new(),
                    },
                    (),
                )))
            })
            .unwrap();
        assert_eq!(
            primary.try_recv().unwrap(),
            ProjectionEvent {
                revision: 2,
                values: Vec::new(),
            }
        );
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn projected_no_op_and_error_do_not_call_or_publish() {
        PROJECTOR_CALLS.store(0, Ordering::SeqCst);
        let registry =
            VersionedRegistry::<u64, u64, u64>::new_with_projection(7, counting_projector);
        let before = registry.snapshot();
        let mut primary = registry.subscribe();
        let mut projected = registry.subscribe_projected();

        assert_eq!(registry.transact::<(), ()>(|_, _| Ok(None)), Ok(None));
        assert_eq!(
            registry.transact::<(), _>(|_, _| Err("rejected")),
            Err("rejected")
        );
        assert_eq!(PROJECTOR_CALLS.load(Ordering::SeqCst), 0);
        assert!(Arc::ptr_eq(&before, &registry.snapshot()));
        assert_eq!(registry.revision(), 0);
        assert!(matches!(
            primary.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn projector_panic_preserves_publication_and_poison_recovery() {
        PANIC_PROJECTOR_ONCE.store(true, Ordering::SeqCst);
        let registry = Arc::new(VersionedRegistry::<u64, u64, u64>::new_with_projection(
            0,
            panic_once_projector,
        ));
        let before = registry.snapshot();
        let mut primary = registry.subscribe();
        let mut projected = registry.subscribe_projected();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
            let registry = Arc::clone(&registry);
            move || {
                let _ = registry.transact::<(), ()>(|current, revision| {
                    Ok(Some((current.value + 1, revision, ())))
                });
            }
        }));
        assert!(result.is_err());
        assert!(registry.mutation_lock.is_poisoned());
        assert!(Arc::ptr_eq(&before, &registry.snapshot()));
        assert_eq!(registry.revision(), 0);
        assert!(matches!(
            primary.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        assert_eq!(
            registry.transact(|current, revision| {
                Ok::<_, ()>(Some((current.value + 1, revision, revision)))
            }),
            Ok(Some(1))
        );
        assert_eq!(primary.try_recv().unwrap(), 1);
        assert_eq!(projected.try_recv().unwrap(), 1);
    }

    #[test]
    fn concurrent_projected_groups_follow_primary_commit_order() {
        let registry = Arc::new(
            VersionedRegistry::<u64, u64, (u64, u64)>::new_with_projection(0, project_group),
        );
        let mut primary = registry.subscribe();
        let mut projected = registry.subscribe_projected();
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let registry = Arc::clone(&registry);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    registry
                        .transact(|current, revision| {
                            Ok::<_, ()>(Some((current.value + 1, revision, revision)))
                        })
                        .unwrap()
                        .unwrap()
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }

        let primary_revisions: Vec<_> = (0..16).map(|_| primary.try_recv().unwrap()).collect();
        assert_eq!(primary_revisions, (1..=16).collect::<Vec<u64>>());
        for revision in primary_revisions {
            assert_eq!(projected.try_recv().unwrap(), (revision, 0));
            assert_eq!(projected.try_recv().unwrap(), (revision, 1));
            assert_eq!(projected.try_recv().unwrap(), (revision, 2));
        }
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn each_stream_publishes_when_the_other_has_no_receiver() {
        let registry =
            VersionedRegistry::<u64, u64, u64>::new_with_projection(0, project_single_revision);
        let mut projected = registry.subscribe_projected();
        registry
            .transact(|_, revision| Ok::<_, ()>(Some((revision, revision, ()))))
            .unwrap();
        assert_eq!(projected.try_recv().unwrap(), 1);

        let registry =
            VersionedRegistry::<u64, u64, u64>::new_with_projection(0, project_single_revision);
        let mut primary = registry.subscribe();
        registry
            .transact(|_, revision| Ok::<_, ()>(Some((revision, revision, ()))))
            .unwrap();
        assert_eq!(primary.try_recv().unwrap(), 1);
    }

    #[test]
    fn projected_close_preserves_same_revision_and_teardown_transactions() {
        let registry = VersionedRegistry::<Vec<u32>, u64, u64>::new_with_projection(
            vec![7],
            project_single_revision,
        );
        let before = registry.snapshot();
        let mut primary = registry.subscribe();
        let mut projected = registry.subscribe_projected();
        registry.close();
        let closed = registry.snapshot();
        assert!(closed.closed);
        assert_eq!(closed.value, vec![7]);
        assert_eq!(closed.revision, 0);
        assert!(!before.closed);
        assert!(matches!(
            primary.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let repeated = registry.snapshot();
        registry.close();
        assert!(Arc::ptr_eq(&repeated, &registry.snapshot()));
        assert!(matches!(
            primary.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        assert_eq!(
            registry.transact(|current, revision| {
                assert!(current.closed);
                Ok::<_, ()>(Some((Vec::new(), revision, current.value.clone())))
            }),
            Ok(Some(vec![7]))
        );
        assert_eq!(registry.revision(), 1);
        assert_eq!(primary.try_recv().unwrap(), 1);
        assert_eq!(projected.try_recv().unwrap(), 1);
    }

    #[test]
    fn no_op_and_error_preserve_snapshot_revision_and_events() {
        let registry = VersionedRegistry::<Vec<u32>, u64>::new(vec![7]);
        let before = registry.snapshot();
        let mut events = registry.subscribe();
        let calls = AtomicUsize::new(0);
        let result = registry.transact::<(), ()>(|current, next| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(current.value, vec![7]);
            assert_eq!(next, 1);
            Ok(None)
        });
        assert_eq!(result, Ok(None));
        let result = registry.transact::<(), _>(|_, next| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(next, 1);
            Err("rejected")
        });
        assert_eq!(result, Err("rejected"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(Arc::ptr_eq(&before, &registry.snapshot()));
        assert_eq!(registry.revision(), 0);
        assert!(!registry.is_closed());
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn commit_calls_fn_once_and_keeps_captured_snapshot_stable() {
        let registry = VersionedRegistry::<Vec<u32>, u64>::new(vec![7]);
        let before = registry.snapshot();
        let mut events = registry.subscribe();
        let calls = AtomicUsize::new(0);
        let owned = Box::new(9);
        let result = registry.transact(|current, revision| {
            calls.fetch_add(1, Ordering::SeqCst);
            let mut value = current.value.clone();
            value.push(*owned);
            drop(owned); // Requires FnOnce: a retry could not invoke this again.
            Ok::<_, ()>(Some((value, revision, "committed")))
        });
        assert_eq!(result, Ok(Some("committed")));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(registry.snapshot().value, vec![7, 9]);
        assert_eq!(registry.revision(), 1);
        assert_eq!(events.try_recv().unwrap(), 1);
        assert_eq!(before.value, vec![7]);
        assert_eq!(before.revision, 0);
        assert!(!before.closed);
    }

    #[test]
    fn concurrent_transactions_keep_all_values_and_order_events() {
        let registry = Arc::new(VersionedRegistry::<Vec<u64>, u64>::new(Vec::new()));
        let mut events = registry.subscribe();
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let calls = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let registry = Arc::clone(&registry);
                let barrier = Arc::clone(&barrier);
                let calls = Arc::clone(&calls);
                thread::spawn(move || {
                    barrier.wait();
                    registry
                        .transact(|current, revision| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            assert_eq!(revision, current.revision + 1);
                            let mut value = current.value.clone();
                            value.push(revision);
                            Ok::<_, ()>(Some((value, revision, revision)))
                        })
                        .unwrap()
                        .unwrap()
                })
            })
            .collect();
        let mut returned: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        returned.sort_unstable();
        let expected: Vec<u64> = (1..=16).collect();
        assert_eq!(returned, expected);
        assert_eq!(calls.load(Ordering::SeqCst), 16);
        assert_eq!(registry.snapshot().value, expected);
        assert_eq!(registry.revision(), 16);
        let published: Vec<_> = (0..16).map(|_| events.try_recv().unwrap()).collect();
        assert_eq!(published, expected);
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn close_wakes_all_registered_waiters_without_revision_or_event() {
        let registry = VersionedRegistry::<Vec<u32>, u64>::new(vec![7]);
        let before = registry.snapshot();
        let mut events = registry.subscribe();
        let signal = registry.close_signal();
        let first = signal.notified();
        let second = signal.notified();
        assert!(!registry.is_closed());
        registry.close();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(first, second);
        })
        .await
        .expect("all registered observers wake after close");
        let closed = registry.snapshot();
        assert!(closed.closed);
        assert_eq!(closed.value, vec![7]);
        assert_eq!(closed.revision, 0);
        assert!(!before.closed);
        let repeated = signal.notified();
        registry.close();
        tokio::time::timeout(std::time::Duration::from_secs(5), repeated)
            .await
            .expect("repeated close may notify again");
        assert!(Arc::ptr_eq(&closed, &registry.snapshot()));
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        // Closing must not disable shell-controlled disposal transactions.
        assert_eq!(
            registry.transact(|current, revision| {
                assert!(current.closed);
                Ok::<_, ()>(Some((Vec::new(), revision, current.value.clone())))
            }),
            Ok(Some(vec![7]))
        );
        assert!(registry.is_closed());
        assert_eq!(registry.revision(), 1);
        assert!(registry.snapshot().value.is_empty());
        assert_eq!(events.try_recv().unwrap(), 1);
    }

    #[test]
    fn panic_does_not_publish_and_poison_recovery_allows_commit_and_close() {
        let registry = Arc::new(VersionedRegistry::<u32, u64>::new(0));
        let before = registry.snapshot();
        let mut events = registry.subscribe();
        let clone = Arc::clone(&registry);
        let panic = thread::spawn(move || {
            let _ = clone.transact::<(), ()>(|_, _| panic!("intentional transaction panic"));
        })
        .join();
        assert!(panic.is_err());
        assert!(registry.mutation_lock.is_poisoned());
        assert!(Arc::ptr_eq(&before, &registry.snapshot()));
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert_eq!(
            registry.transact(|current, revision| {
                Ok::<_, ()>(Some((current.value + 1, revision, revision)))
            }),
            Ok(Some(1))
        );
        assert_eq!(registry.snapshot().value, 1);
        assert_eq!(registry.revision(), 1);
        assert_eq!(events.try_recv().unwrap(), 1);
        registry.close();
        assert!(registry.is_closed());
        assert_eq!(registry.revision(), 1);
    }

    #[test]
    fn live_events_lag_at_256_and_do_not_replay_to_late_subscribers() {
        let registry = VersionedRegistry::<u64, u64>::new(0);
        let mut events = registry.subscribe();
        for _ in 0..257 {
            registry
                .transact(|_, revision| Ok::<_, ()>(Some((revision, revision, ()))))
                .unwrap();
        }
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(1))
        ));
        let retained: Vec<_> = (0..256).map(|_| events.try_recv().unwrap()).collect();
        assert_eq!(retained, (2..=257).collect::<Vec<u64>>());
        let mut late = registry.subscribe();
        assert!(matches!(
            late.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert_eq!(registry.snapshot().value, 257);
        assert_eq!(registry.revision(), 257);
    }

    #[test]
    fn reentrant_transact_and_close_abort_instead_of_deadlocking() {
        let registry = Arc::new(VersionedRegistry::<u32, u64>::new(0));

        // A nested `transact` on the same registry must abort rather than wait
        // for the mutation lock this thread already holds.
        let clone = Arc::clone(&registry);
        let nested_transact = thread::spawn(move || {
            let _ = clone.transact::<(), ()>(|_, _| {
                let _ = clone.transact::<(), ()>(|_, _| Ok::<_, ()>(None));
                Ok::<_, ()>(None)
            });
        })
        .join();
        assert!(nested_transact.is_err());

        // A nested `close` on the same registry must abort the same way.
        let clone = Arc::clone(&registry);
        let nested_close = thread::spawn(move || {
            let _ = clone.transact::<(), ()>(|_, _| {
                clone.close();
                Ok::<_, ()>(None)
            });
        })
        .join();
        assert!(nested_close.is_err());

        // Neither aborted call published anything, and the registry stays
        // usable because the mutation lock recovers from poisoning.
        assert_eq!(registry.snapshot().value, 0);
        assert_eq!(registry.revision(), 0);
        assert!(!registry.is_closed());
        assert_eq!(
            registry.transact(|current, revision| {
                Ok::<_, ()>(Some((current.value + 1, revision, revision)))
            }),
            Ok(Some(1))
        );
        assert_eq!(registry.snapshot().value, 1);
        registry.close();
        assert!(registry.is_closed());
    }

    #[test]
    fn revision_exhaustion_aborts_instead_of_reusing_a_published_revision() {
        let registry = Arc::new(VersionedRegistry::<u32, u64>::from_snapshot(
            VersionedSnapshot {
                value: 7,
                revision: u64::MAX,
                closed: false,
            },
        ));

        let clone = Arc::clone(&registry);
        let exhausted = thread::spawn(move || {
            let _ = clone.transact::<(), ()>(|_, _| Ok::<_, ()>(Some((1, 1, ()))));
        })
        .join();
        let payload = exhausted.expect_err("an exhausted revision counter must abort");
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_string())
            })
            .unwrap_or_default();
        assert!(
            message.contains("revision counter exhausted"),
            "unexpected abort message: {message}"
        );

        // The aborted transaction published nothing: the last published
        // revision and value are untouched.
        assert_eq!(registry.revision(), u64::MAX);
        assert_eq!(registry.snapshot().value, 7);
    }
}
