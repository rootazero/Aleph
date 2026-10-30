//! Provider lifecycle contracts shared by in-process and Bridge providers.
//!
//! This module owns state and generation semantics only. It does not start
//! threads, open transports, load dynamic libraries, or perform IPC. Runtime
//! owners such as `ExtensionManager` and a future Bridge supervisor perform
//! those effects and drive this state machine around them.

use crate::sync_primitives::{Arc, Mutex};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Stable identity of a provider instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderIdError {
    #[error("provider id must not be empty")]
    Empty,
}

impl ProviderId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderIdError> {
        let value = value.into();
        let value = value.trim();
        if value.is_empty() {
            return Err(ProviderIdError::Empty);
        }
        Ok(Self(value.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Runtime boundary used by a provider implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderTransport {
    InProcess,
    Bridge,
}

/// Lifecycle state of one provider instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderState {
    Discovered,
    /// Admitted declaration, no runtime effects and no executable lease.
    Cold,
    Disabled,
    Starting,
    Ready,
    Stopping,
    Stopped,
    Failed,
}

/// One committed state transition, including the provider generation that
/// leases must capture and validate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTransition {
    pub provider_id: ProviderId,
    pub transport: ProviderTransport,
    pub from: ProviderState,
    pub to: ProviderState,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderLifecycleError {
    #[error("provider generation exhausted for {provider_id}")]
    GenerationExhausted { provider_id: ProviderId },
    #[error("invalid provider transition for {provider_id}: {from:?} -> {to:?}")]
    InvalidTransition {
        provider_id: ProviderId,
        from: ProviderState,
        to: ProviderState,
    },
}

/// A call was not admitted to the expected live provider instance.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("provider {provider_id} rejects call: {state:?}, expected generation {expected_generation}, current generation {actual_generation}")]
pub struct ProviderCallError {
    pub provider_id: ProviderId,
    pub state: ProviderState,
    pub expected_generation: u64,
    pub actual_generation: u64,
}

/// Pure provider lifecycle state machine.
///
/// A generation is allocated when a new start begins. A lease from an older
/// generation is therefore invalid during restart and remains invalid after a
/// replacement reaches `Ready`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLifecycle {
    provider_id: ProviderId,
    transport: ProviderTransport,
    state: ProviderState,
    generation: u64,
}

impl ProviderLifecycle {
    #[must_use]
    pub fn new(provider_id: ProviderId, transport: ProviderTransport) -> Self {
        Self {
            provider_id,
            transport,
            state: ProviderState::Discovered,
            generation: 0,
        }
    }

    /// Construct an already-installed built-in provider. This is used for the
    /// existing canonical Tool registry; it does not run any initialization.
    #[must_use]
    pub fn ready(provider_id: ProviderId, transport: ProviderTransport) -> Self {
        Self {
            provider_id,
            transport,
            state: ProviderState::Ready,
            generation: 1,
        }
    }

    #[must_use]
    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    #[must_use]
    pub fn transport(&self) -> ProviderTransport {
        self.transport
    }

    #[must_use]
    pub fn state(&self) -> ProviderState {
        self.state
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state == ProviderState::Ready
    }

    /// Check admission against both state and generation from the same instance.
    pub fn acquire_call(&self, expected_generation: u64) -> Result<(), ProviderCallError> {
        if !self.is_ready() || self.generation != expected_generation {
            return Err(ProviderCallError {
                provider_id: self.provider_id.clone(),
                state: self.state,
                expected_generation,
                actual_generation: self.generation,
            });
        }
        Ok(())
    }

    /// Apply one lifecycle transition and return its committed evidence.
    pub fn transition(
        &mut self,
        next: ProviderState,
    ) -> Result<ProviderTransition, ProviderLifecycleError> {
        let from = self.state;
        if !is_allowed(from, next) {
            return Err(ProviderLifecycleError::InvalidTransition {
                provider_id: self.provider_id.clone(),
                from,
                to: next,
            });
        }

        if next == ProviderState::Starting {
            self.generation = self.generation.checked_add(1).ok_or_else(|| {
                ProviderLifecycleError::GenerationExhausted {
                    provider_id: self.provider_id.clone(),
                }
            })?;
        }
        self.state = next;
        Ok(ProviderTransition {
            provider_id: self.provider_id.clone(),
            transport: self.transport,
            from,
            to: next,
            generation: self.generation,
        })
    }
}

fn is_allowed(from: ProviderState, to: ProviderState) -> bool {
    matches!(
        (from, to),
        (ProviderState::Discovered, ProviderState::Cold)
            | (ProviderState::Disabled, ProviderState::Cold)
            | (ProviderState::Stopped, ProviderState::Cold)
            | (ProviderState::Cold, ProviderState::Disabled)
            | (ProviderState::Cold, ProviderState::Starting)
            | (ProviderState::Discovered, ProviderState::Disabled)
            | (ProviderState::Discovered, ProviderState::Starting)
            | (ProviderState::Disabled, ProviderState::Starting)
            | (ProviderState::Starting, ProviderState::Ready)
            | (ProviderState::Starting, ProviderState::Failed)
            | (ProviderState::Starting, ProviderState::Stopping)
            | (ProviderState::Ready, ProviderState::Stopping)
            | (ProviderState::Ready, ProviderState::Failed)
            | (ProviderState::Stopping, ProviderState::Stopped)
            | (ProviderState::Stopping, ProviderState::Failed)
            | (ProviderState::Stopped, ProviderState::Starting)
            | (ProviderState::Stopped, ProviderState::Disabled)
            | (ProviderState::Failed, ProviderState::Starting)
            | (ProviderState::Failed, ProviderState::Stopped)
            | (ProviderState::Failed, ProviderState::Disabled)
    )
}

/// Thread-safe control handle used by a registry or a future lifecycle
/// supervisor. The side effect is only state publication; process startup and
/// shutdown remain outside this type.
#[derive(Clone)]
pub struct ProviderHandle {
    lifecycle: Arc<Mutex<ProviderLifecycle>>,
    changes: broadcast::Sender<ProviderTransition>,
}

impl ProviderHandle {
    #[must_use]
    pub fn new(lifecycle: ProviderLifecycle) -> Self {
        let (changes, _) = broadcast::channel(16);
        Self {
            lifecycle: Arc::new(Mutex::new(lifecycle)),
            changes,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> (ProviderState, u64) {
        self.with_snapshot(|lifecycle| (lifecycle.state(), lifecycle.generation()))
    }

    /// Opaque instance identity; a matching textual provider id is not authority.
    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.lifecycle, &other.lifecycle)
    }

    /// Observe lifecycle state while transitions are excluded. Canonical plugin
    /// publication holds this guard through its registry transaction: lock order
    /// is provider -> registry mutation, never the reverse. The closure must not
    /// re-enter this handle or invoke external/handler code.
    pub(crate) fn with_snapshot<R>(&self, read: impl FnOnce(&ProviderLifecycle) -> R) -> R {
        let lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read(&lifecycle)
    }

    /// Atomically commit Starting -> Ready with a canonical registry batch.
    /// Only local state is prepared first. On callback error neither state nor
    /// notifications change. The callback must not re-enter this provider or
    /// call external code; lock order is lifecycle -> registry transaction.
    /// A panic may poison the mutex; later operations recover its inner state.
    pub(crate) fn with_ready_commit<R, E: From<ProviderCallError>>(
        &self,
        expected_generation: u64,
        commit: impl FnOnce(&ProviderLifecycle) -> Result<R, E>,
    ) -> Result<R, E> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lifecycle.state() != ProviderState::Starting
            || lifecycle.generation() != expected_generation
        {
            return Err(ProviderCallError {
                provider_id: lifecycle.provider_id().clone(),
                state: lifecycle.state(),
                expected_generation,
                actual_generation: lifecycle.generation(),
            }
            .into());
        }
        let mut prepared = lifecycle.clone();
        let transition = prepared
            .transition(ProviderState::Ready)
            .expect("Starting -> Ready cannot fail or allocate a generation");
        let result = commit(&prepared)?;
        // Infallible local commit: readers of a frozen registry proof must
        // acquire this same lifecycle mutex before accepting it as executable.
        *lifecycle = prepared;
        drop(lifecycle);
        let _ = self.changes.send(transition);
        Ok(result)
    }

    #[must_use]
    pub fn state(&self) -> ProviderState {
        self.snapshot().0
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.snapshot().1
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state() == ProviderState::Ready
    }

    /// Admit a call atomically with respect to lifecycle transitions.
    ///
    /// Invoke this at the handler boundary, not just when resolving it. A
    /// successful check is the admission linearization point: calls admitted
    /// before `Stopping` may finish. This does not drain or cancel in-flight
    /// work and is not permission to unload its code or transport.
    pub fn acquire_call(&self, expected_generation: u64) -> Result<(), ProviderCallError> {
        self.lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .acquire_call(expected_generation)
    }

    /// Hold Ready/current-generation validation through a source-local commit.
    ///
    /// The callback must only mutate local bookkeeping: no await, I/O, external
    /// handler code, or provider re-entry. Source and ownership gates must be
    /// acquired before this guard; entry bookkeeping follows it.
    pub(crate) fn with_call_admission<R>(
        &self,
        expected_generation: u64,
        commit: impl FnOnce() -> R,
    ) -> Result<R, ProviderCallError> {
        let lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lifecycle.acquire_call(expected_generation)?;
        Ok(commit())
    }

    /// Hold provider validation through completion of already-admitted work.
    ///
    /// This checks only the provider domain, not permission to finish work in
    /// Stopping: the source must also validate a live, pre-stop, non-clone work
    /// ticket. The callback and lock-order constraints of `with_call_admission`
    /// apply here too; no drain or transport work runs under this guard.
    #[allow(dead_code)] // Reserved for a future source completion caller.
    pub(crate) fn with_admitted_completion<R>(
        &self,
        expected_generation: u64,
        commit: impl FnOnce() -> R,
    ) -> Result<R, ProviderCallError> {
        let lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(
            lifecycle.state(),
            ProviderState::Ready | ProviderState::Stopping
        ) || lifecycle.generation() != expected_generation
        {
            return Err(ProviderCallError {
                provider_id: lifecycle.provider_id().clone(),
                state: lifecycle.state(),
                expected_generation,
                actual_generation: lifecycle.generation(),
            });
        }
        Ok(commit())
    }

    /// Commit a state transition and notify live observers after the state is
    /// visible. The broadcast is a notification tap, not a recovery source.
    pub fn transition(
        &self,
        next: ProviderState,
    ) -> Result<ProviderTransition, ProviderLifecycleError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transition = lifecycle.transition(next)?;
        drop(lifecycle);
        let _ = self.changes.send(transition.clone());
        Ok(transition)
    }

    #[must_use]
    pub fn subscribe_changes(&self) -> broadcast::Receiver<ProviderTransition> {
        self.changes.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> ProviderLifecycle {
        ProviderLifecycle::new(
            ProviderId::new("test/provider").expect("valid provider id"),
            ProviderTransport::Bridge,
        )
    }

    #[test]
    fn explicit_start_is_the_only_transition_that_allocates_fixture_resources() {
        let provider = ProviderHandle::new(provider());
        provider.transition(ProviderState::Cold).unwrap();
        let resources = std::sync::atomic::AtomicUsize::new(0);

        assert_eq!(provider.snapshot(), (ProviderState::Cold, 0));
        assert_eq!(resources.load(std::sync::atomic::Ordering::SeqCst), 0);

        provider.transition(ProviderState::Starting).unwrap();
        resources.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        provider.transition(ProviderState::Ready).unwrap();
        assert_eq!(resources.load(std::sync::atomic::Ordering::SeqCst), 1);

        provider.transition(ProviderState::Stopping).unwrap();
        provider.transition(ProviderState::Stopped).unwrap();
        provider.transition(ProviderState::Starting).unwrap();
        resources.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        provider.transition(ProviderState::Ready).unwrap();
        assert_eq!(resources.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn lifecycle_allocates_a_new_generation_on_each_start() {
        let mut lifecycle = provider();
        assert_eq!(lifecycle.generation(), 0);
        assert_eq!(
            lifecycle
                .transition(ProviderState::Starting)
                .unwrap()
                .generation,
            1
        );
        lifecycle.transition(ProviderState::Ready).unwrap();
        lifecycle.transition(ProviderState::Stopping).unwrap();
        lifecycle.transition(ProviderState::Stopped).unwrap();
        assert_eq!(
            lifecycle
                .transition(ProviderState::Starting)
                .unwrap()
                .generation,
            2
        );
    }

    #[test]
    fn invalid_transition_is_fail_closed() {
        let mut lifecycle = provider();
        let error = lifecycle.transition(ProviderState::Ready).unwrap_err();
        assert!(matches!(
            error,
            ProviderLifecycleError::InvalidTransition {
                from: ProviderState::Discovered,
                to: ProviderState::Ready,
                ..
            }
        ));
        assert_eq!(lifecycle.state(), ProviderState::Discovered);
        assert_eq!(lifecycle.generation(), 0);
    }

    #[test]
    fn call_admission_requires_ready_and_the_current_generation() {
        let handle = ProviderHandle::new(provider());
        assert!(handle.acquire_call(0).is_err());
        handle.transition(ProviderState::Starting).unwrap();
        assert!(handle.acquire_call(1).is_err());
        handle.transition(ProviderState::Ready).unwrap();
        assert_eq!(handle.acquire_call(1), Ok(()));
        handle.transition(ProviderState::Stopping).unwrap();
        assert!(handle.acquire_call(1).is_err());
        handle.transition(ProviderState::Stopped).unwrap();
        assert!(handle.acquire_call(1).is_err());
        handle.transition(ProviderState::Starting).unwrap();
        handle.transition(ProviderState::Ready).unwrap();
        assert!(handle.acquire_call(1).is_err());
        assert_eq!(handle.acquire_call(2), Ok(()));
        handle.transition(ProviderState::Failed).unwrap();
        assert!(handle.acquire_call(2).is_err());
        handle.transition(ProviderState::Disabled).unwrap();
        assert!(handle.acquire_call(2).is_err());
    }

    #[test]
    fn admission_commit_runs_only_for_ready_current_generation() {
        let handle = ProviderHandle::new(provider());
        let commits = std::cell::Cell::new(0);
        for state in [
            ProviderState::Discovered,
            ProviderState::Cold,
            ProviderState::Disabled,
            ProviderState::Starting,
            ProviderState::Ready,
            ProviderState::Stopping,
            ProviderState::Stopped,
            ProviderState::Starting,
            ProviderState::Failed,
        ] {
            if state != ProviderState::Discovered {
                handle.transition(state).unwrap();
            }
            let generation = handle.generation();
            let before = commits.get();
            let result = handle.with_call_admission(generation, || {
                commits.set(commits.get() + 1);
                7
            });
            if state == ProviderState::Ready {
                assert_eq!(result, Ok(7));
                assert_eq!(commits.get(), before + 1);
                assert!(handle
                    .with_call_admission(generation - 1, || panic!("stale generation"))
                    .is_err());
            } else {
                assert_eq!(
                    result,
                    Err(ProviderCallError {
                        provider_id: ProviderId::new("test/provider").unwrap(),
                        state,
                        expected_generation: generation,
                        actual_generation: generation,
                    })
                );
                assert_eq!(commits.get(), before);
            }
            assert_eq!(
                handle.with_call_admission(generation + 1, || {
                    commits.set(commits.get() + 1);
                }),
                Err(ProviderCallError {
                    provider_id: ProviderId::new("test/provider").unwrap(),
                    state,
                    expected_generation: generation + 1,
                    actual_generation: generation,
                })
            );
        }
        assert_eq!(commits.get(), 1);
    }

    #[test]
    fn completion_guard_accepts_same_generation_ready_or_stopping() {
        let handle = ProviderHandle::new(provider());
        let commits = std::cell::Cell::new(0);
        for state in [
            ProviderState::Discovered,
            ProviderState::Cold,
            ProviderState::Disabled,
            ProviderState::Starting,
            ProviderState::Ready,
            ProviderState::Stopping,
            ProviderState::Stopped,
            ProviderState::Starting,
            ProviderState::Failed,
        ] {
            if state != ProviderState::Discovered {
                handle.transition(state).unwrap();
            }
            let generation = handle.generation();
            let before = commits.get();
            let result = handle.with_admitted_completion(generation, || {
                commits.set(commits.get() + 1);
                7
            });
            if matches!(state, ProviderState::Ready | ProviderState::Stopping) {
                assert_eq!(result, Ok(7));
                assert_eq!(commits.get(), before + 1);
                assert!(handle
                    .with_admitted_completion(generation - 1, || panic!("stale generation"))
                    .is_err());
            } else {
                assert_eq!(
                    result,
                    Err(ProviderCallError {
                        provider_id: ProviderId::new("test/provider").unwrap(),
                        state,
                        expected_generation: generation,
                        actual_generation: generation,
                    })
                );
                assert_eq!(commits.get(), before);
            }
            assert_eq!(
                handle.with_admitted_completion(generation + 1, || {
                    commits.set(commits.get() + 1);
                }),
                Err(ProviderCallError {
                    provider_id: ProviderId::new("test/provider").unwrap(),
                    state,
                    expected_generation: generation + 1,
                    actual_generation: generation,
                })
            );
        }
        assert_eq!(commits.get(), 2);
    }

    #[test]
    fn provider_guard_serializes_stop_with_commit() {
        // Exercise both guards: neither may release the lifecycle lock before
        // the local commit has completed. No callback re-enters the provider.
        for completion in [false, true] {
            let handle = ProviderHandle::new(provider());
            handle.transition(ProviderState::Starting).unwrap();
            handle.transition(ProviderState::Ready).unwrap();
            let generation = handle.generation();
            let entered = std::sync::Barrier::new(2);
            let release = std::sync::Barrier::new(2);
            let committed = std::sync::atomic::AtomicBool::new(false);
            let (attempt_tx, attempt_rx) = std::sync::mpsc::channel();
            let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();

            std::thread::scope(|scope| {
                let admission = scope.spawn(|| {
                    let commit = || {
                        entered.wait();
                        release.wait();
                        committed.store(true, std::sync::atomic::Ordering::SeqCst);
                        7
                    };
                    if completion {
                        handle.with_admitted_completion(generation, commit)
                    } else {
                        handle.with_call_admission(generation, commit)
                    }
                });
                entered.wait();
                let stop = scope.spawn(|| {
                    // Observe the real mutex, not an arbitrary scheduling delay.
                    attempt_tx
                        .send(handle.lifecycle.try_lock().is_err())
                        .unwrap();
                    let result = handle.transition(ProviderState::Stopping);
                    stopped_tx
                        .send(committed.load(std::sync::atomic::Ordering::SeqCst))
                        .unwrap();
                    result
                });
                let locked = attempt_rx.recv_timeout(std::time::Duration::from_secs(5));
                let pending = stopped_rx.try_recv();
                // Always release the callback before asserting, even on failure.
                release.wait();
                assert_eq!(admission.join().unwrap(), Ok(7));
                assert_eq!(stop.join().unwrap().unwrap().to, ProviderState::Stopping);
                assert!(locked.unwrap());
                assert_eq!(pending, Err(std::sync::mpsc::TryRecvError::Empty));
                assert!(stopped_rx.recv().unwrap());
            });
            assert_eq!(handle.snapshot(), (ProviderState::Stopping, generation));
            assert!(handle
                .with_call_admission(generation, || panic!("stopped"))
                .is_err());
        }
    }

    #[test]
    fn generation_exhaustion_does_not_run_commit() {
        let mut lifecycle = provider();
        lifecycle.state = ProviderState::Stopped;
        lifecycle.generation = u64::MAX;
        let handle = ProviderHandle::new(lifecycle);
        let mut changes = handle.subscribe_changes();
        assert_eq!(
            handle.transition(ProviderState::Starting),
            Err(ProviderLifecycleError::GenerationExhausted {
                provider_id: ProviderId::new("test/provider").unwrap(),
            })
        );
        assert_eq!(handle.snapshot(), (ProviderState::Stopped, u64::MAX));
        assert_eq!(
            changes.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        );
        let commits = std::cell::Cell::new(0);
        assert_eq!(
            handle.with_call_admission(u64::MAX, || commits.set(commits.get() + 1)),
            Err(ProviderCallError {
                provider_id: ProviderId::new("test/provider").unwrap(),
                state: ProviderState::Stopped,
                expected_generation: u64::MAX,
                actual_generation: u64::MAX,
            })
        );
        assert_eq!(
            handle.with_admitted_completion(u64::MAX, || commits.set(commits.get() + 1)),
            Err(ProviderCallError {
                provider_id: ProviderId::new("test/provider").unwrap(),
                state: ProviderState::Stopped,
                expected_generation: u64::MAX,
                actual_generation: u64::MAX,
            })
        );
        assert_eq!(commits.get(), 0);
    }

    #[test]
    fn exhausted_generation_cannot_restart_or_readmit_old_calls() {
        let mut lifecycle = provider();
        lifecycle.state = ProviderState::Stopped;
        lifecycle.generation = u64::MAX;
        assert!(matches!(
            lifecycle.transition(ProviderState::Starting),
            Err(ProviderLifecycleError::GenerationExhausted { .. })
        ));
        assert_eq!(lifecycle.state(), ProviderState::Stopped);
        assert!(lifecycle.acquire_call(u64::MAX).is_err());
    }

    #[test]
    fn handle_shares_state_and_publishes_committed_transitions() {
        let handle = ProviderHandle::new(provider());
        let clone = handle.clone();
        let mut changes = handle.subscribe_changes();
        clone.transition(ProviderState::Starting).unwrap();
        assert_eq!(handle.state(), ProviderState::Starting);
        assert_eq!(changes.try_recv().unwrap().generation, 1);
    }
}
