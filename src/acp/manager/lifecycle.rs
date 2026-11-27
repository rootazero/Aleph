//! Native session admission, operations and retirement.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::{session_closed, AcpAdapterManager, SessionEntry, SessionKey, SlotReservation};
use crate::acp::adapter::{AcpAdapter, AdapterMode};
use crate::acp::protocol::{AcpErrorCode, AcpOperationError};
use crate::acp::session::{AcpSession, CancelDispatch};
use crate::acp::{AcpChunkCallback, AcpSessionEvent};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;

const CANCEL_GRACE: Duration = Duration::from_millis(100);
const REAP_DEADLINE: Duration = Duration::from_secs(2);

impl AcpAdapterManager {
    /// Start the native connection. An existing remote association is loaded
    /// here. A first prompt normally establishes a new remote session ID.
    pub async fn ensure_session(&self, harness_id: &str, cwd: &str) -> Result<()> {
        self.ensure_session_named(harness_id, cwd, None).await
    }

    pub async fn ensure_session_named(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<()> {
        let adapter = self.admitted_adapter(harness_id).await?;
        require_mode(&*adapter, harness_id, AdapterMode::NativeAcp)?;
        let key = SessionKey::with_name(harness_id, cwd, session_name);
        self.acquire_owner(key, adapter, false, false, None).await?;
        Ok(())
    }

    pub(super) async fn acquire_owner(
        &self,
        key: SessionKey,
        adapter: Arc<dyn AcpAdapter>,
        establish_id: bool,
        replace: bool,
        restore_epoch: Option<&Arc<CancellationToken>>,
    ) -> Result<Arc<SessionEntry>> {
        let reservation = {
            let mut state = self.state();
            Self::check_registration(&state, &key.harness_id, &adapter)?;
            if restore_epoch.is_some_and(|expected| {
                !state
                    .slots
                    .get(&key)
                    .is_some_and(|slot| Arc::ptr_eq(&slot.epoch, expected))
            }) {
                return Err(session_closed());
            }
            let slot = state.slots.entry(key.clone()).or_default();
            if slot.epoch.is_cancelled() {
                return Err(session_closed());
            }
            if !replace {
                let reusable = slot.live.as_ref().filter(|entry| {
                    entry.healthy() && (!establish_id || entry.cancel.has_session_id())
                });
                if let Some(entry) = reusable {
                    return Ok(Arc::clone(entry));
                }
            }
            let event = if replace {
                slot.withdraw();
                forget_association(slot, &key)
            } else {
                None
            };
            let reservation = self.reservation(&key, slot);
            if let Some(event) = event {
                state.publish(Some(event));
            }
            reservation
        };

        // The task owns the transition even if the request is dropped. The
        // reservation keeps the same gate visible to close, update and stop.
        let manager = self.clone();
        tokio::spawn(async move {
            manager
                .acquire_reserved(reservation, adapter, establish_id, replace)
                .await
        })
        .await
        .map_err(transition_task_error)?
    }

    async fn acquire_reserved(
        &self,
        reservation: SlotReservation,
        adapter: Arc<dyn AcpAdapter>,
        establish_id: bool,
        replace: bool,
    ) -> Result<Arc<SessionEntry>> {
        let key = &reservation.key;
        let _guard = reservation.transition.lock().await;
        let epoch = if replace {
            self.retire_reserved(&reservation)
                .await?
                .ok_or_else(session_closed)?
        } else {
            Arc::clone(&reservation.epoch)
        };
        let (existing, saved_id) = {
            let state = self.state();
            Self::check_registration(&state, &key.harness_id, &adapter)?;
            if epoch.is_cancelled() {
                return Err(session_closed());
            }
            let slot = &state.slots[key];
            (slot.live.clone(), slot.saved_id.clone())
        };
        let cwd = key.cwd.to_string_lossy();
        let config = adapter.build_native_config(Some(&cwd));
        let timeout = config.timeout;

        if let Some(entry) = existing {
            if entry.healthy() {
                if establish_id && !entry.cancel.has_session_id() {
                    let mut session = entry.lock_for_operation().await?;
                    let result = tokio::select! {
                        biased;
                        () = epoch.cancelled() => return Err(session_closed()),
                        result = session.create_acp_session(&cwd, timeout) => result,
                    };
                    match result {
                        Ok(id) => self.publish_owner(key, &adapter, &epoch, &entry, Some(id))?,
                        Err(error) => {
                            entry.closed.cancel();
                            return Err(error);
                        }
                    }
                }
                return Ok(entry);
            }
            entry.closed.cancel();
            retire_owner(&entry).await?;
            let mut state = self.state();
            state.slots.get_mut(key).expect("reserved slot").live = None;
            state.publish(None);
        }

        if epoch.is_cancelled() {
            return Err(session_closed());
        }
        let mut session = AcpSession::spawn(adapter.id(), &config).await?;
        let result = tokio::select! {
            biased;
            () = epoch.cancelled() => Err(session_closed()),
            result = async {
                session.initialize(timeout).await?;
                match &saved_id {
                    Some(id) => session.load_acp_session(id, &cwd, timeout).await.map(Some),
                    None if establish_id => session.create_acp_session(&cwd, timeout).await.map(Some),
                    None => Ok(None),
                }
            } => result,
        };
        let entry = Arc::new(SessionEntry::new(session, timeout));
        let error = match result {
            Ok(id) => match self.publish_owner(key, &adapter, &epoch, &entry, id) {
                Ok(()) => return Ok(entry),
                Err(error) => error,
            },
            Err(error) => error,
        };

        entry.closed.cancel();
        let reap = retire_owner(&entry).await;
        let mut state = self.state();
        let slot = state.slots.get_mut(key).expect("reserved slot");
        if let Err(reap_error) = reap {
            // Keep ownership until an explicit or subsequent retirement can
            // join this child. A replacement cannot start while it remains.
            warn!(error = %reap_error, harness = %key.harness_id, "ACP startup cleanup failed");
            slot.live = Some(entry);
        }
        if !epoch.is_cancelled() && saved_id.is_some() {
            slot.load_failure = Some(error.to_string());
            state.publish(None);
        }
        Err(error)
    }

    fn publish_owner(
        &self,
        key: &SessionKey,
        adapter: &Arc<dyn AcpAdapter>,
        epoch: &Arc<CancellationToken>,
        entry: &Arc<SessionEntry>,
        id: Option<String>,
    ) -> Result<()> {
        let mut state = self.state();
        Self::check_registration(&state, &key.harness_id, adapter)?;
        let slot = state.slots.get_mut(key).expect("reserved slot");
        if epoch.is_cancelled() {
            return Err(session_closed());
        }
        let event = id.and_then(|id| {
            if slot.saved_id.as_ref() == Some(&id) {
                return None;
            }
            slot.saved_id = Some(id.clone());
            Some(AcpSessionEvent::Created {
                harness_id: key.harness_id.clone(),
                acp_session_id: id,
                cwd: key.cwd.to_string_lossy().into_owned(),
                session_name: key.session_name_opt().map(str::to_owned),
            })
        });
        slot.live = Some(Arc::clone(entry));
        slot.load_failure = None;
        state.publish(event);
        Ok(())
    }

    pub async fn prompt(
        &self,
        harness_id: &str,
        prompt_text: &str,
        cwd: &str,
        mode: Option<AdapterMode>,
        reuse_session: bool,
        on_chunk: Option<AcpChunkCallback>,
    ) -> Result<String> {
        self.prompt_named(
            harness_id,
            prompt_text,
            cwd,
            None,
            mode,
            reuse_session,
            on_chunk,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prompt_named(
        &self,
        harness_id: &str,
        prompt_text: &str,
        cwd: &str,
        session_name: Option<&str>,
        mode: Option<AdapterMode>,
        reuse_session: bool,
        on_chunk: Option<AcpChunkCallback>,
    ) -> Result<String> {
        let adapter = self.admitted_adapter(harness_id).await?;
        let mode = mode.unwrap_or_else(|| adapter.mode());
        require_mode(&*adapter, harness_id, mode)?;
        match mode {
            AdapterMode::NativeAcp => {
                let key = SessionKey::with_name(harness_id, cwd, session_name);
                let entry = self
                    .acquire_owner(key, adapter, true, !reuse_session, None)
                    .await?;
                let mut session = entry.lock_for_operation().await?;
                tokio::select! {
                    biased;
                    () = entry.abort.cancelled() => Err(session_closed()),
                    result = session.prompt(prompt_text, cwd, entry.timeout, on_chunk.as_ref()) =>
                        result.map(|(text, _)| text),
                }
            }
            AdapterMode::Oneshot => adapter.execute_oneshot(prompt_text, cwd).await,
        }
    }

    /// Cancel the captured peer without queuing behind its active prompt.
    pub async fn cancel(&self, harness_id: &str, cwd: &str) -> Result<()> {
        self.cancel_named(harness_id, cwd, None).await.map(|_| ())
    }

    pub async fn cancel_named(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<CancelDispatch> {
        let entry = self.current_owner(harness_id, cwd, session_name)?;
        tokio::select! {
            biased;
            () = entry.closed.cancelled() => Err(session_closed()),
            result = entry.cancel.send_cancel() => result,
        }
    }

    pub async fn set_mode(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        mode_id: &str,
    ) -> Result<()> {
        let entry = self.current_owner(harness_id, cwd, session_name)?;
        let mut session = entry.lock_for_operation().await?;
        tokio::select! {
            biased;
            () = entry.abort.cancelled() => Err(session_closed()),
            result = session.set_mode(mode_id, entry.timeout) => result,
        }
    }

    pub async fn set_model(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        model_id: &str,
    ) -> Result<()> {
        let entry = self.current_owner(harness_id, cwd, session_name)?;
        let mut session = entry.lock_for_operation().await?;
        tokio::select! {
            biased;
            () = entry.abort.cancelled() => Err(session_closed()),
            result = session.set_model(model_id, entry.timeout) => result,
        }
    }

    pub async fn set_config_option(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        key: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        let entry = self.current_owner(harness_id, cwd, session_name)?;
        let mut session = entry.lock_for_operation().await?;
        tokio::select! {
            biased;
            () = entry.abort.cancelled() => Err(session_closed()),
            result = session.set_config_option(key, value, entry.timeout) => result,
        }
    }

    pub async fn authenticate(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        method_id: &str,
        credential: &str,
    ) -> Result<()> {
        let entry = self.current_owner(harness_id, cwd, session_name)?;
        let mut session = entry.lock_for_operation().await?;
        tokio::select! {
            biased;
            () = entry.abort.cancelled() => Err(session_closed()),
            result = session.authenticate(method_id, credential, entry.timeout) => result,
        }
    }

    fn current_owner(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<Arc<SessionEntry>> {
        let key = SessionKey::with_name(harness_id, cwd, session_name);
        self.state()
            .slots
            .get(&key)
            .and_then(|slot| slot.live.clone())
            .ok_or_else(session_closed)
    }

    /// Withdraw and close the owner and saved association for this key.
    pub async fn shutdown_named(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<()> {
        let key = SessionKey::with_name(harness_id, cwd, session_name);
        let reservation = {
            let mut state = self.state();
            let Some(slot) = state.slots.get_mut(&key) else {
                return Ok(());
            };
            slot.withdraw();
            let event = forget_association(slot, &key);
            let reservation = self.reservation(&key, slot);
            if let Some(event) = event {
                state.publish(Some(event));
            }
            reservation
        };
        self.finish_retirements(vec![reservation]).await
    }

    /// Stop admission permanently and reap live children while preserving IDs.
    pub async fn shutdown_all(&self) {
        let reservations = {
            let mut state = self.state();
            state.stopped = true;
            self.withdraw_slots(&mut state, None, false)
        };
        if let Err(error) = self.finish_retirements(reservations).await {
            warn!(%error, "ACP server shutdown could not reap every child");
        }
    }

    pub(super) fn withdraw_slots(
        &self,
        state: &mut super::ManagerState,
        harness: Option<&str>,
        forget: bool,
    ) -> Vec<SlotReservation> {
        let mut reservations = Vec::new();
        let mut events = Vec::new();
        for (key, slot) in &mut state.slots {
            if harness.is_none_or(|harness| key.harness_id == harness) {
                slot.withdraw();
                reservations.push(self.reservation(key, slot));
                if forget {
                    events.extend(forget_association(slot, key));
                }
            }
        }
        for event in events {
            state.publish(Some(event));
        }
        reservations
    }

    pub(super) async fn finish_retirements(
        &self,
        reservations: Vec<SlotReservation>,
    ) -> Result<()> {
        if reservations.is_empty() {
            return Ok(());
        }
        let manager = self.clone();
        tokio::spawn(async move {
            let results = futures::future::join_all(reservations.into_iter().map(|reservation| {
                let manager = &manager;
                async move {
                    let _guard = reservation.transition.lock().await;
                    manager.retire_reserved(&reservation).await
                }
            }))
            .await;
            results
                .into_iter()
                .try_for_each(|result| result.map(|_| ()))
        })
        .await
        .map_err(transition_task_error)?
    }

    // The slot's transition guard is held for the whole retirement. Keep
    // the retiring entry registered until process exit has been observed.
    async fn retire_reserved(
        &self,
        target: &SlotReservation,
    ) -> Result<Option<Arc<CancellationToken>>> {
        let key = &target.key;
        let live = {
            let state = self.state();
            let slot = &state.slots[key];
            if !Arc::ptr_eq(&slot.epoch, &target.epoch) {
                return Ok(None);
            }
            slot.live.clone()
        };
        if let Some(entry) = &live {
            retire_owner(entry).await?;
        }
        let mut state = self.state();
        let slot = state.slots.get_mut(key).expect("reserved slot");
        slot.live = None;
        slot.epoch = Arc::default();
        let epoch = Arc::clone(&slot.epoch);
        if live.is_some() {
            state.publish(None);
        }
        Ok(Some(epoch))
    }
}

fn forget_association(slot: &mut super::SessionSlot, key: &SessionKey) -> Option<AcpSessionEvent> {
    slot.load_failure = None;
    slot.saved_id.take()?;
    Some(AcpSessionEvent::Removed {
        harness_id: key.harness_id.clone(),
        cwd: key.cwd.to_string_lossy().into_owned(),
        session_name: key.session_name_opt().map(str::to_owned),
    })
}

pub(super) fn require_mode(
    adapter: &dyn AcpAdapter,
    harness_id: &str,
    mode: AdapterMode,
) -> Result<()> {
    if !adapter.supported_modes().contains(&mode) {
        return Err(AcpOperationError::new(
            AcpErrorCode::ModeUnsupported,
            format!("Harness '{harness_id}' does not support {mode:?} mode"),
        )
        .into());
    }
    Ok(())
}

async fn retire_owner(entry: &SessionEntry) -> Result<()> {
    let cooperative = async {
        match entry.cancel.send_cancel().await? {
            CancelDispatch::Sent => drop(entry.session.lock().await),
            CancelDispatch::NoSession => {}
        }
        Ok::<(), AlephError>(())
    };
    if let Ok(Err(error)) = tokio::time::timeout(CANCEL_GRACE, cooperative).await {
        debug!(%error, "ACP cancel during retirement could not be sent");
    }
    entry.abort.cancel();
    tokio::time::timeout(REAP_DEADLINE, async {
        entry.session.lock().await.reap().await
    })
    .await
    .map_err(|_| AcpOperationError::new(AcpErrorCode::Timeout, "Timed out reaping ACP session"))?
}

fn transition_task_error(error: tokio::task::JoinError) -> AlephError {
    AcpOperationError::new(
        AcpErrorCode::SessionDead,
        format!("ACP session transition task failed: {error}"),
    )
    .into()
}
