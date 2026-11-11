//! Focused, acknowledged UI capability transport owned by the Core.

use crate::capability::policy_runtime::CapabilityCallContext;
use crate::capability::special::{
    CapabilityDispatchError, CapabilityHandler, CapabilityHandlerCall,
};
use crate::capability::ui::UiError;
use crate::sync_primitives::{Arc, AtomicU64, Mutex, Ordering};
use aleph_protocol::capability::{
    UiAttachment, UiDisplayMessage, UiInputEvent, UiOperation, UiRenderAck, UiSubmitInput,
    MAX_UI_QUEUE, MAX_UI_TEXT_BYTES, UI_ACK_TIMEOUT_MS,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::VecDeque;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

struct PendingDisplay {
    request_id: String,
    sent: bool,
    start: Option<oneshot::Sender<()>>,
    resolution: Option<oneshot::Sender<Result<(), UiError>>>,
}

#[derive(Default)]
struct DisplayWorker {
    handle: Option<JoinHandle<()>>,
    failed: bool,
}

struct SurfaceState {
    closed: bool,
    pending: Option<PendingDisplay>,
    input: VecDeque<UiInputEvent>,
    policy_source: Option<(
        std::sync::Weak<super::policy::PolicyEngine>,
        super::policy_runtime::PolicyFactSourceKey,
    )>,
}

/// A server-owned, one-render-at-a-time UI surface.
///
/// The send callback is the transport boundary. It only emits a display
/// message; completion is controlled by the matching render acknowledgement.
pub struct UiSurface {
    attachment: UiAttachment,
    owner_connection: String,
    owner: CapabilityCallContext,
    state: Arc<Mutex<SurfaceState>>,
    worker: tokio::sync::Mutex<DisplayWorker>,
    send: Arc<dyn Fn(UiDisplayMessage) -> Result<(), UiError> + Send + Sync>,
}

impl UiSurface {
    #[must_use]
    pub fn new(
        attachment: UiAttachment,
        owner: CapabilityCallContext,
        send: Arc<dyn Fn(UiDisplayMessage) -> Result<(), UiError> + Send + Sync>,
    ) -> Self {
        Self {
            attachment,
            owner_connection: owner.connection_id().unwrap_or_default().to_owned(),
            owner,
            state: Arc::new(Mutex::new(SurfaceState {
                closed: false,
                pending: None,
                input: VecDeque::new(),
                policy_source: None,
            })),
            worker: tokio::sync::Mutex::new(DisplayWorker::default()),
            send,
        }
    }

    #[must_use]
    pub fn owner_connection(&self) -> &str {
        &self.owner_connection
    }

    pub(crate) fn owner_context(&self) -> &CapabilityCallContext {
        &self.owner
    }

    pub(crate) fn bind_policy_source(
        &self,
        policy: &Arc<super::policy::PolicyEngine>,
        key: super::policy_runtime::PolicyFactSourceKey,
    ) -> Result<(), UiError> {
        let mut state = self.state.lock().map_err(|_| UiError::Poisoned)?;
        if state.closed {
            return Err(UiError::Closed);
        }
        let weak = Arc::downgrade(policy);
        match &state.policy_source {
            Some((existing, existing_key)) if existing.ptr_eq(&weak) && existing_key == &key => {
                Ok(())
            }
            Some(_) => Err(UiError::Unavailable),
            None => {
                state.policy_source = Some((weak, key));
                Ok(())
            }
        }
    }

    pub async fn display(
        &self,
        text: String,
        admission: CapabilityHandlerCall,
        cancel: CancellationToken,
    ) -> Result<(), UiError> {
        if text.len() > MAX_UI_TEXT_BYTES {
            return Err(UiError::InvalidInput(
                "display text exceeds maximum UTF-8 byte length".into(),
            ));
        }
        if admission.context().connection_id() != Some(self.owner_connection()) {
            return Err(UiError::StaleAttachment);
        }
        if cancel.is_cancelled() {
            return Err(UiError::Cancelled);
        }

        let mut worker = self.worker.lock().await;
        {
            let state = self.state.lock().map_err(|_| UiError::Poisoned)?;
            if state.closed {
                return Err(UiError::Closed);
            }
            if state.pending.is_some() {
                return Err(UiError::Busy);
            }
        }
        // The previous completed render may still be releasing its admission.
        // Poll the retained handle in place; cancellation never detaches it.
        if let Some(handle) = worker.handle.as_mut() {
            let failed = handle.await.is_err();
            worker.failed |= failed;
            worker.handle = None;
        }
        let (request_id, result_rx) = {
            let mut surface_state = self.state.lock().map_err(|_| UiError::Poisoned)?;
            if surface_state.closed {
                return Err(UiError::Closed);
            }
            if surface_state.pending.is_some() {
                return Err(UiError::Busy);
            }
            let request_id = format!(
                "ui-request-{:016x}",
                NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
            );
            let (start_tx, start_rx) = oneshot::channel();
            let (resolution_tx, resolution_rx) = oneshot::channel();
            let (result_tx, result_rx) = oneshot::channel();
            let state = Arc::clone(&self.state);
            let task_request_id = request_id.clone();
            let observation = admission.observation();
            let task = tokio::spawn(async move {
                let _ = start_rx.await;
                let result = tokio::select! {
                    resolution = resolution_rx => resolution.unwrap_or(Err(UiError::Unavailable)),
                    _ = cancel.cancelled() => Err(UiError::Cancelled),
                    _ = tokio::time::sleep(std::time::Duration::from_millis(UI_ACK_TIMEOUT_MS)) => {
                        Err(UiError::Timeout)
                    }
                };
                clear_pending(&state, &task_request_id);
                if let Some(observation) = observation {
                    observation.complete(if result.is_ok() {
                        crate::capability::events::HandlerResult::Success
                    } else {
                        crate::capability::events::HandlerResult::ExecutionFailed
                    });
                }
                let _ = result_tx.send(result);
                drop(admission);
            });

            surface_state.pending = Some(PendingDisplay {
                request_id: request_id.clone(),
                sent: false,
                start: Some(start_tx),
                resolution: Some(resolution_tx),
            });
            worker.handle = Some(task);
            (request_id, result_rx)
        };
        drop(worker);

        let message = UiDisplayMessage {
            surface_id: self.attachment.surface_id.clone(),
            mount_generation: self.attachment.mount_generation,
            request_id: request_id.clone(),
            text,
        };
        if let Err(error) = (self.send)(message) {
            return self.finish_pending(request_id, error, result_rx).await;
        }

        let start = {
            let mut state = self.state.lock().map_err(|_| UiError::Poisoned)?;
            let pending = state
                .pending
                .as_mut()
                .filter(|pending| pending.request_id == request_id)
                .ok_or(UiError::Closed)?;
            pending.sent = true;
            pending.start.take()
        };
        if let Some(start) = start {
            let _ = start.send(());
        }
        result_rx.await.unwrap_or(Err(UiError::Unavailable))
    }

    pub fn input(&self) -> Result<Option<UiInputEvent>, UiError> {
        let mut state = self.state.lock().map_err(|_| UiError::Poisoned)?;
        if state.closed {
            return Err(UiError::Closed);
        }
        Ok(state.input.pop_front())
    }

    pub fn ack(&self, caller_connection: &str, ack: UiRenderAck) -> Result<(), UiError> {
        self.validate_attachment(caller_connection, &ack.surface_id, ack.mount_generation)?;
        if !ack.rendered {
            return Err(UiError::RenderFailed);
        }
        let (resolution, start) = {
            let mut state = self.state.lock().map_err(|_| UiError::Poisoned)?;
            if state.closed {
                return Err(UiError::Closed);
            }
            let pending = state
                .pending
                .as_mut()
                .filter(|pending| pending.request_id == ack.request_id && pending.sent)
                .ok_or(UiError::StaleAttachment)?;
            (pending.resolution.take(), pending.start.take())
        };
        let resolution = resolution.ok_or(UiError::StaleAttachment)?;
        let _ = resolution.send(Ok(()));
        if let Some(start) = start {
            let _ = start.send(());
        }
        Ok(())
    }

    pub fn submit_input(
        &self,
        caller_connection: &str,
        input: UiSubmitInput,
    ) -> Result<(), UiError> {
        self.validate_attachment(caller_connection, &input.surface_id, input.mount_generation)?;
        input
            .event
            .validate()
            .map_err(|error| UiError::InvalidInput(error.into()))?;
        let mut state = self.state.lock().map_err(|_| UiError::Poisoned)?;
        if state.closed {
            return Err(UiError::Closed);
        }
        if state.input.len() >= MAX_UI_QUEUE {
            return Err(UiError::QueueFull);
        }
        state.input.push_back(input.event);
        Ok(())
    }

    pub async fn close(&self) -> Result<(), UiError> {
        // Release acknowledgement waits BEFORE joining any retained worker.
        // A cancelled close leaves both this owner and its handle intact.
        let (resolution, start, policy_source) = {
            let mut state = self.state.lock().map_err(|_| UiError::Poisoned)?;
            state.closed = true;
            state.input.clear();
            let (resolution, start) = state
                .pending
                .as_mut()
                .map(|pending| (pending.resolution.take(), pending.start.take()))
                .unwrap_or_default();
            (resolution, start, state.policy_source.take())
        };
        if let Some((policy, key)) = policy_source {
            if let Some(policy) = policy.upgrade() {
                policy.withdraw_fact_source(&key);
            }
        }
        if let Some(resolution) = resolution {
            let _ = resolution.send(Err(UiError::Closed));
        }
        if let Some(start) = start {
            let _ = start.send(());
        }
        let mut worker = self.worker.lock().await;
        if let Some(handle) = worker.handle.as_mut() {
            let failed = handle.await.is_err();
            worker.failed |= failed;
            worker.handle = None;
        }
        if worker.failed {
            Err(UiError::Unavailable)
        } else {
            Ok(())
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.state.lock().map_or(true, |state| state.closed)
    }

    pub(crate) fn validate_attachment(
        &self,
        caller_connection: &str,
        surface_id: &str,
        mount_generation: u64,
    ) -> Result<(), UiError> {
        if caller_connection != self.owner_connection
            || surface_id != self.attachment.surface_id
            || mount_generation != self.attachment.mount_generation
        {
            return Err(UiError::StaleAttachment);
        }
        if self.is_closed() {
            return Err(UiError::Closed);
        }
        Ok(())
    }

    async fn finish_pending(
        &self,
        request_id: String,
        error: UiError,
        resolution_rx: oneshot::Receiver<Result<(), UiError>>,
    ) -> Result<(), UiError> {
        let (resolution, start) = {
            let mut state = self.state.lock().map_err(|_| UiError::Poisoned)?;
            state
                .pending
                .as_mut()
                .filter(|pending| pending.request_id == request_id)
                .map(|pending| (pending.resolution.take(), pending.start.take()))
                .unwrap_or_default()
        };
        if let Some(resolution) = resolution {
            let _ = resolution.send(Err(error));
        }
        if let Some(start) = start {
            let _ = start.send(());
        }
        resolution_rx.await.unwrap_or(Err(UiError::Unavailable))
    }
}

fn clear_pending(state: &Arc<Mutex<SurfaceState>>, request_id: &str) {
    let Ok(mut state) = state.lock() else {
        return;
    };
    if state
        .pending
        .as_ref()
        .is_some_and(|pending| pending.request_id == request_id)
    {
        state.pending.take();
    }
}

#[async_trait]
impl CapabilityHandler for UiSurface {
    async fn invoke(
        &self,
        input: Value,
        call: CapabilityHandlerCall,
        cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError> {
        let operation: UiOperation = serde_json::from_value(input)
            .map_err(|error| CapabilityDispatchError::InvalidInput(error.to_string()))?;
        match operation {
            UiOperation::Display { text } => self
                .display(text, call, cancel)
                .await
                .map(|_| Value::Null)
                .map_err(map_error),
            UiOperation::Input {} => self
                .input()
                .map(|event| serde_json::to_value(event).unwrap_or(Value::Null))
                .map_err(map_error),
        }
    }
}

fn map_error(error: UiError) -> CapabilityDispatchError {
    match error {
        UiError::Unavailable => CapabilityDispatchError::Unavailable,
        UiError::Cancelled => CapabilityDispatchError::Cancelled,
        UiError::InvalidInput(message) => CapabilityDispatchError::InvalidInput(message),
        other => CapabilityDispatchError::Execution(other.to_string()),
    }
}
