//! Lifecycle management for ACP harness registrations and native sessions.

mod harness_admin;
mod lifecycle;
mod persistence;
pub(crate) mod session_key;

#[cfg(test)]
mod tests;

pub use persistence::{
    load_persisted_sessions, save_persisted_sessions, wire_persistence, AcpPersistence,
};
pub use session_key::SessionKey;

use std::collections::HashMap;
use std::sync::MutexGuard;
use std::time::Duration;

use tokio::sync::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
use tokio_util::sync::CancellationToken;

use crate::acp::adapter::AcpAdapter;
use crate::acp::protocol::{AcpErrorCode, AcpOperationError};
use crate::acp::session::{AcpSession, CancelHandle};
use crate::acp::{AcpSessionEvent, GatewayChangeHook, PersistenceHook};
use crate::config::types::acp::AcpAdapterEntry;
use crate::error::{AlephError, Result};
use crate::sync_primitives::{Arc, Mutex};

struct SessionEntry {
    session: AsyncMutex<AcpSession>,
    cancel: CancelHandle,
    timeout: Duration,
    closed: CancellationToken,
    abort: CancellationToken,
}

impl SessionEntry {
    fn new(session: AcpSession, timeout: Duration) -> Self {
        Self {
            cancel: session.cancel_handle(),
            session: AsyncMutex::new(session),
            timeout,
            closed: CancellationToken::new(),
            abort: CancellationToken::new(),
        }
    }

    fn healthy(&self) -> bool {
        !self.closed.is_cancelled()
            && self.session.try_lock().map_or(true, |mut session| {
                session.is_alive() && !session.is_errored()
            })
    }

    async fn lock_for_operation(&self) -> Result<AsyncMutexGuard<'_, AcpSession>> {
        tokio::select! {
            biased;
            () = self.closed.cancelled() => Err(session_closed()),
            mut session = self.session.lock() => {
                if self.closed.is_cancelled() || !session.is_alive() || session.is_errored() {
                    return Err(session_closed());
                }
                Ok(session)
            }
        }
    }
}

fn session_closed() -> AlephError {
    AcpOperationError::new(
        AcpErrorCode::SessionDead,
        "ACP session is unavailable or closing",
    )
    .into()
}

#[derive(Default)]
struct SessionSlot {
    transition: Arc<AsyncMutex<()>>,
    epoch: Arc<CancellationToken>,
    live: Option<Arc<SessionEntry>>,
    saved_id: Option<String>,
    load_failure: Option<String>,
}

impl SessionSlot {
    fn withdraw(&self) {
        self.epoch.cancel();
        if let Some(entry) = &self.live {
            entry.closed.cancel();
        }
    }
}

struct HarnessRegistration {
    config: AcpAdapterEntry,
    adapter: Option<Arc<dyn AcpAdapter>>,
}

#[derive(Default)]
struct ManagerState {
    registrations: HashMap<String, HarnessRegistration>,
    slots: HashMap<SessionKey, SessionSlot>,
    stopped: bool,
    persistence_hook: Option<PersistenceHook>,
    gateway_change_hook: Option<GatewayChangeHook>,
}

impl ManagerState {
    // These hooks only enqueue a persistence event and invalidate a projection.
    // Their order is the order of the associated in-memory transitions.
    fn publish(&self, event: Option<AcpSessionEvent>) {
        if let (Some(event), Some(hook)) = (event, &self.persistence_hook) {
            hook(event);
        }
        if let Some(hook) = &self.gateway_change_hook {
            hook();
        }
    }
}

/// Holds a slot identity across transitions. Vacant slots are reclaimed after
/// the last reservation so a queued operation cannot acquire a replacement lock.
struct SlotReservation {
    manager: AcpAdapterManager,
    key: SessionKey,
    transition: Arc<AsyncMutex<()>>,
    epoch: Arc<CancellationToken>,
}

impl Drop for SlotReservation {
    fn drop(&mut self) {
        let mut state = self.manager.state();
        let unused = state.slots.get(&self.key).is_some_and(|slot| {
            Arc::strong_count(&self.transition) == 2
                && slot.live.is_none()
                && slot.saved_id.is_none()
        });
        if unused {
            state.slots.remove(&self.key);
        }
    }
}

/// Owns the registered ACP harnesses and the native session slots.
/// The state mutex only guards admission, identity and publication. Process I/O
/// is serialized by the individual slot or live session mutex.
#[derive(Clone)]
pub struct AcpAdapterManager {
    state: Arc<Mutex<ManagerState>>,
}

impl AcpAdapterManager {
    fn state(&self) -> MutexGuard<'_, ManagerState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn reservation(&self, key: &SessionKey, slot: &SessionSlot) -> SlotReservation {
        SlotReservation {
            manager: self.clone(),
            key: key.clone(),
            transition: Arc::clone(&slot.transition),
            epoch: Arc::clone(&slot.epoch),
        }
    }
}

impl Default for AcpAdapterManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Lightweight view of an ACP association for Gateway and Panel.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSnapshot {
    pub harness_id: String,
    pub cwd: String,
    /// `None` for the default unnamed session.
    pub session_name: Option<String>,
    pub acp_session_id: Option<String>,
    pub alive: bool,
    pub state: crate::acp::protocol::AcpSessionState,
    /// Most recent failed attempt to open the saved association.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
}
