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
        if value.trim().is_empty() {
            return Err(ProviderIdError::Empty);
        }
        Ok(Self(value))
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
    #[error("invalid provider transition for {provider_id}: {from:?} -> {to:?}")]
    InvalidTransition {
        provider_id: ProviderId,
        from: ProviderState,
        to: ProviderState,
    },
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
            self.generation = self.generation.saturating_add(1);
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
        (ProviderState::Discovered, ProviderState::Disabled)
            | (ProviderState::Discovered, ProviderState::Starting)
            | (ProviderState::Disabled, ProviderState::Starting)
            | (ProviderState::Starting, ProviderState::Ready)
            | (ProviderState::Starting, ProviderState::Failed)
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
        let lifecycle = self
            .lifecycle
            .lock()
            .expect("provider lifecycle mutex poisoned");
        (lifecycle.state(), lifecycle.generation())
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

    /// Commit a state transition and notify live observers after the state is
    /// visible. The broadcast is a notification tap, not a recovery source.
    pub fn transition(
        &self,
        next: ProviderState,
    ) -> Result<ProviderTransition, ProviderLifecycleError> {
        let transition = self
            .lifecycle
            .lock()
            .expect("provider lifecycle mutex poisoned")
            .transition(next)?;
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
    fn handle_shares_state_and_publishes_committed_transitions() {
        let handle = ProviderHandle::new(provider());
        let clone = handle.clone();
        let mut changes = handle.subscribe_changes();
        clone.transition(ProviderState::Starting).unwrap();
        assert_eq!(handle.state(), ProviderState::Starting);
        assert_eq!(changes.try_recv().unwrap().generation, 1);
    }
}
