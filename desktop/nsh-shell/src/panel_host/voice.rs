use std::sync::Arc;

use aleph_client::{AlephClient, CliError};
use aleph_protocol::{
    panel::PANEL_HOST_UNAVAILABLE,
    panel_voice::{
        is_capture_control, is_native_capture_method, VOICE_TURN_CANCEL_METHOD,
        VOICE_TURN_CAPTURE_CANCEL_METHOD, VOICE_TURN_CAPTURE_METHOD, VOICE_TURN_TRANSCRIBE_METHOD,
    },
};
use serde_json::{json, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use super::{error_reply, rpc_reply, Link, Shared, CALL_TIMEOUT, CAPTURE_TIMEOUT};

pub(super) struct VoiceSlots {
    serial: Arc<Semaphore>,
    capture: Arc<Semaphore>,
    capture_controls: Arc<Semaphore>,
    provider_cancel: Arc<Semaphore>,
    owned_chat_abort: Arc<Semaphore>,
}

impl Default for VoiceSlots {
    fn default() -> Self {
        Self {
            serial: Arc::new(Semaphore::new(1)),
            capture: Arc::new(Semaphore::new(1)),
            // The third capture-control slot is reserved for native cleanup.
            capture_controls: Arc::new(Semaphore::new(2)),
            provider_cancel: Arc::new(Semaphore::new(1)),
            owned_chat_abort: Arc::new(Semaphore::new(1)),
        }
    }
}

pub(super) struct AdmittedVoice {
    binding: Uuid,
    visibility_epoch: u64,
    capture_id: Option<Uuid>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CapturePhase {
    Capturing,
    Sealed,
    Consuming,
    Cancelling,
    Cleanup { confirmed: bool },
}

#[derive(Clone, Copy)]
pub(super) struct CaptureOwner {
    id: Uuid,
    serial: u64,
    phase: CapturePhase,
}

pub(super) struct CaptureCleanup {
    owner: CaptureOwner,
    binding: Uuid,
    client: Arc<AlephClient>,
}

impl Link {
    fn owns_voice_client(&self, binding: Uuid, client: &Arc<AlephClient>) -> bool {
        !self.closed
            && self.voice_binding == Some(binding)
            && self
                .client
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(client, current))
    }

    pub(super) fn start_capture_cleanup(&mut self, owner: CaptureOwner) -> Option<CaptureCleanup> {
        let binding = self.voice_binding?;
        let client = self.client.clone()?;
        let capture = self
            .capture
            .as_mut()
            .filter(|capture| capture.serial == owner.serial)?;
        if matches!(capture.phase, CapturePhase::Cleanup { .. }) {
            return None;
        }
        capture.phase = CapturePhase::Cleanup { confirmed: false };
        Some(CaptureCleanup {
            owner,
            binding,
            client,
        })
    }
}

fn reject(reason: &'static str) -> Value {
    error_reply(PANEL_HOST_UNAVAILABLE, reason)
}

fn dispatch_error(reason: &'static str) -> CliError {
    CliError::Other(reason.into())
}

impl Shared {
    pub(super) fn admit_voice_rpc(
        &self,
        method: &str,
        params: Option<&Value>,
        binding: Option<&Value>,
    ) -> Result<AdmittedVoice, Value> {
        let binding = binding
            .and_then(Value::as_str)
            .and_then(|binding| Uuid::parse_str(binding).ok())
            .ok_or_else(|| reject("a live voice binding is required"))?;
        let visibility_epoch = {
            let link = self.link();
            if link.closed || link.voice_binding != Some(binding) || link.client.is_none() {
                return Err(reject("the voice binding is no longer current"));
            }
            let (visible, epoch) = self.playback_admission.visibility();
            if matches!(
                method,
                VOICE_TURN_CAPTURE_METHOD | VOICE_TURN_TRANSCRIBE_METHOD
            ) && !visible
            {
                return Err(reject("the voice surface is not visible"));
            }
            epoch
        };
        let slots = &self.voice_slots;
        let slot = if method == VOICE_TURN_CAPTURE_METHOD {
            &slots.capture
        } else if is_capture_control(method) {
            &slots.capture_controls
        } else if method == VOICE_TURN_CANCEL_METHOD {
            &slots.provider_cancel
        } else if method == "chat.abort" {
            &slots.owned_chat_abort
        } else {
            &slots.serial
        };
        let permit = slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| reject("the voice RPC lane is busy"))?;
        if !AlephClient::panel_voice_request_fits(method, params) {
            return Err(reject("voice RPC exceeds its wire budget"));
        }
        if method == "chat.abort"
            && !params.and_then(Value::as_object).is_some_and(|params| {
                params.len() == 2
                    && params.get("run_only").and_then(Value::as_bool) == Some(true)
                    && params
                        .get("run_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
            })
        {
            return Err(error_reply(
                -32602,
                "bound chat.abort requires run_id and run_only: true",
            ));
        }
        let id = params
            .and_then(|value| value.get("capture_id"))
            .filter(|id| !id.is_null());
        let capture_id = if is_native_capture_method(method)
            || (method == VOICE_TURN_TRANSCRIBE_METHOD && id.is_some())
        {
            Some(
                id.and_then(Value::as_str)
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .ok_or_else(|| error_reply(-32602, "voice RPC requires a UUID capture_id"))?,
            )
        } else {
            None
        };
        Ok(AdmittedVoice {
            binding,
            visibility_epoch,
            capture_id,
            _permit: permit,
        })
    }

    pub(super) async fn voice_rpc(
        &self,
        method: &str,
        params: Option<&Value>,
        voice: AdmittedVoice,
    ) -> Value {
        let _turn = if is_native_capture_method(method)
            || matches!(method, VOICE_TURN_CANCEL_METHOD | "chat.abort")
        {
            None
        } else {
            Some(self.turn.lock().await)
        };
        let client = {
            let link = self.link();
            if link.closed || link.voice_binding != Some(voice.binding) {
                return reject("the voice binding is no longer current");
            }
            link.client.clone()
        };
        let Some(client) = client else {
            return reject("the Gateway is not connected");
        };
        let mut owner = None;
        let timeout = if method == VOICE_TURN_CAPTURE_METHOD {
            CAPTURE_TIMEOUT
        } else {
            CALL_TIMEOUT
        };
        let result = client
            .call_panel_voice(method, params, timeout, || {
                let mut link = self.link();
                if !link.owns_voice_client(voice.binding, &client) {
                    return Err(dispatch_error("the voice binding is no longer current"));
                }
                if matches!(
                    method,
                    VOICE_TURN_CAPTURE_METHOD | VOICE_TURN_TRANSCRIBE_METHOD
                ) {
                    let (visible, epoch) = self.playback_admission.visibility();
                    if !visible || epoch != voice.visibility_epoch {
                        return Err(dispatch_error(
                            "the voice surface is no longer visible in this epoch",
                        ));
                    }
                }
                if method == VOICE_TURN_CAPTURE_METHOD {
                    if link.capture.is_some() {
                        return Err(dispatch_error("the surface already owns a capture"));
                    }
                    link.last_capture += 1;
                    let capture = CaptureOwner {
                        id: voice.capture_id.expect("admitted capture ID"),
                        serial: link.last_capture,
                        phase: CapturePhase::Capturing,
                    };
                    link.capture = Some(capture);
                    owner = Some(capture);
                } else if method == VOICE_TURN_TRANSCRIBE_METHOD {
                    if let Some(id) = voice.capture_id {
                        let capture = link
                            .capture
                            .as_mut()
                            .filter(|capture| {
                                capture.id == id && capture.phase == CapturePhase::Sealed
                            })
                            .ok_or_else(|| {
                                dispatch_error("the surface no longer owns this sealed capture")
                            })?;
                        capture.phase = CapturePhase::Consuming;
                        owner = Some(*capture);
                    }
                } else if method == VOICE_TURN_CAPTURE_CANCEL_METHOD {
                    if let Some(capture) = link
                        .capture
                        .as_mut()
                        .filter(|capture| Some(capture.id) == voice.capture_id)
                    {
                        if !matches!(capture.phase, CapturePhase::Cleanup { .. }) {
                            capture.phase = CapturePhase::Cancelling;
                        }
                        owner = Some(*capture);
                    }
                }
                Ok(())
            })
            .await;
        if !client.is_connected() {
            self.lost(&client);
        }
        let mut sealed = false;
        let cleanup = {
            let mut link = self.link();
            if !link.owns_voice_client(voice.binding, &client) {
                return reject("the voice binding is no longer current");
            }
            if let Some(capture) = link
                .capture
                .as_mut()
                .filter(|capture| owner.is_some_and(|owner| owner.serial == capture.serial))
            {
                if result.is_ok() {
                    match (method, &mut capture.phase) {
                        (VOICE_TURN_CAPTURE_METHOD, phase @ CapturePhase::Capturing) => {
                            *phase = CapturePhase::Sealed;
                            sealed = true;
                        }
                        (
                            VOICE_TURN_CAPTURE_CANCEL_METHOD | VOICE_TURN_TRANSCRIBE_METHOD,
                            CapturePhase::Cleanup { confirmed },
                        ) => *confirmed = true,
                        (VOICE_TURN_CAPTURE_CANCEL_METHOD | VOICE_TURN_TRANSCRIBE_METHOD, _) => {
                            link.capture = None
                        }
                        _ => {}
                    }
                    None
                } else {
                    // A typed or transport error need not prove consumption or
                    // cancellation. Use the reserved exact cleanup path.
                    link.start_capture_cleanup(owner.expect("matching capture"))
                }
            } else {
                None
            }
        };
        if let Some(cleanup) = cleanup {
            self.clean_capture(cleanup).await;
        }
        if method == VOICE_TURN_CAPTURE_METHOD && result.is_ok() && !sealed {
            return reject("the surface no longer owns this capture");
        }
        rpc_reply(result)
    }

    pub(super) async fn clean_capture(&self, cleanup: CaptureCleanup) {
        let CaptureCleanup {
            owner,
            binding,
            client,
        } = cleanup;
        let params = json!({"capture_id": owner.id});
        let result = client
            .call_panel_voice(
                VOICE_TURN_CAPTURE_CANCEL_METHOD,
                Some(&params),
                CALL_TIMEOUT,
                || Ok(()),
            )
            .await;
        let mut link = self.link();
        if link.owns_voice_client(binding, &client) {
            let Some(capture) = link
                .capture
                .as_ref()
                .filter(|capture| capture.serial == owner.serial)
            else {
                return;
            };
            let confirmed = capture.phase == (CapturePhase::Cleanup { confirmed: true });
            if result.is_ok() || confirmed {
                link.capture = None;
                return;
            }
            // Retire before releasing Link, otherwise a new writer can enter
            // the same uncertain owner between cleanup failure and retirement.
            client.retire_panel_connection();
            drop(link);
            self.lost(&client);
        } else if result.is_err() {
            client.retire_panel_connection();
        }
    }
}
