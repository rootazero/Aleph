//! A Desktop IPC connection owns at most one active or sealed capture.

use std::collections::HashSet;
use std::time::Duration;

use aleph_protocol::panel_voice::{
    VoiceErrorKind, VOICE_TURN_CAPTURE_CANCEL_METHOD, VOICE_TURN_CAPTURE_FINISH_METHOD,
    VOICE_TURN_CAPTURE_METHOD, VOICE_TURN_CAPTURE_READY_METHOD, WAV_HEADER_BYTES,
};
use shared_ui_logic::voice::{CaptureDecision, CaptureVad};
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
use tracing::instrument::WithSubscriber;
use uuid::Uuid;

use super::{validate_voice_wav, voice_error, VoiceIngress, VoiceRequestContext, PCM_SAMPLE_BYTES};
use super::{Arc, CancellationToken, DesktopPlatform, HandlerRegistry, JsonRpcResponse};
use super::{Deserialize, Mutex, PoisonError};
use serde_json::json;

const READY_WAIT: Duration = Duration::from_secs(5);
const CAPTURE_DEADLINE: Duration = Duration::from_secs(45);
const SEALED_LIFETIME: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(40);
// A fresh connection can get more leases, but an old UUID can never be reused
// on the same connection to consume a different clip.
const MAX_CAPTURE_IDS: usize = 2048;

type CaptureResult<T> = Result<T, VoiceErrorKind>;

#[derive(Clone, Copy, Default)]
struct Update {
    ready: bool,
    terminal: Option<CaptureResult<bool>>,
}

struct Attempt {
    id: Uuid,
    cancel: CancellationToken,
    finish: CancellationToken,
    released: CancellationToken,
    update: watch::Sender<Update>,
}

impl Attempt {
    fn new(id: Uuid, connection: &CancellationToken) -> Arc<Self> {
        let (update, _) = watch::channel(Update::default());
        Arc::new(Self {
            id,
            cancel: connection.child_token(),
            finish: CancellationToken::new(),
            released: CancellationToken::new(),
            update,
        })
    }

    async fn terminal(&self) -> CaptureResult<bool> {
        let mut update = self.update.subscribe();
        loop {
            if let Some(outcome) = update.borrow_and_update().terminal {
                return outcome;
            }
            let _ = update.changed().await;
        }
    }

    async fn ready(&self) -> CaptureResult<()> {
        let mut update = self.update.subscribe();
        loop {
            let state = *update.borrow_and_update();
            if let Some(Err(error)) = state.terminal {
                return Err(error);
            }
            if self.cancel.is_cancelled() {
                return Err(VoiceErrorKind::Cancelled);
            }
            if state.ready {
                return Ok(());
            }
            tokio::select! {
                () = self.cancel.cancelled() => return Err(VoiceErrorKind::Cancelled),
                _ = update.changed() => {},
            }
        }
    }
}

struct CaptureSlot {
    attempt: Arc<Attempt>,
    clip: Option<SealedClip>,
}

struct SealedClip {
    wav: Vec<u8>,
    expires_at: tokio::time::Instant,
}

#[derive(Default)]
struct CaptureTable {
    slot: Option<CaptureSlot>,
    used: HashSet<Uuid>,
}

pub(super) struct ConnectionCapture {
    table: Mutex<CaptureTable>,
    registration: watch::Sender<()>,
}

impl Default for ConnectionCapture {
    fn default() -> Self {
        Self {
            table: Mutex::default(),
            registration: watch::channel(()).0,
        }
    }
}

impl ConnectionCapture {
    fn lock(&self) -> std::sync::MutexGuard<'_, CaptureTable> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start(&self, id: Uuid, connection: &CancellationToken) -> CaptureResult<Arc<Attempt>> {
        let mut table = self.lock();
        if connection.is_cancelled() || table.used.contains(&id) {
            return Err(VoiceErrorKind::Cancelled);
        }
        if table.slot.is_some() || table.used.len() == MAX_CAPTURE_IDS {
            return Err(VoiceErrorKind::CaptureUnavailable);
        }
        let attempt = Attempt::new(id, connection);
        table.used.insert(id);
        table.slot = Some(CaptureSlot {
            attempt: attempt.clone(),
            clip: None,
        });
        self.registration.send_replace(());
        Ok(attempt)
    }

    fn lookup(&self, id: Uuid) -> CaptureResult<Option<Arc<Attempt>>> {
        let table = self.lock();
        match &table.slot {
            Some(slot) if slot.attempt.id == id => Ok(Some(slot.attempt.clone())),
            _ if table.used.contains(&id) => Err(VoiceErrorKind::Cancelled),
            _ => Ok(None),
        }
    }

    async fn ready(&self, id: Uuid) -> CaptureResult<()> {
        let mut registration = self.registration.subscribe();
        loop {
            if let Some(attempt) = self.lookup(id)? {
                return attempt.ready().await;
            }
            let _ = registration.changed().await;
        }
    }

    fn cancel(&self, id: Uuid) -> Option<Arc<Attempt>> {
        let mut table = self.lock();
        let slot = table.slot.as_ref().filter(|slot| slot.attempt.id == id);
        if let Some(slot) = slot {
            let attempt = slot.attempt.clone();
            attempt.cancel.cancel();
            if slot.clip.is_some() {
                table.slot = None;
                attempt.released.cancel();
            }
            Some(attempt)
        } else {
            // At capacity start is already closed; do not evict an older ID.
            if table.used.len() < MAX_CAPTURE_IDS && table.used.insert(id) {
                self.registration.send_replace(());
            }
            None
        }
    }

    pub(super) fn consume(&self, id: Uuid) -> CaptureResult<Vec<u8>> {
        let mut table = self.lock();
        let Some(slot) = table
            .slot
            .as_mut()
            .filter(|slot| slot.attempt.id == id && !slot.attempt.cancel.is_cancelled())
        else {
            return Err(VoiceErrorKind::InvalidAudio);
        };
        let clip = slot.clip.take().ok_or(VoiceErrorKind::InvalidAudio)?;
        slot.attempt.released.cancel();
        table.slot = None;
        if tokio::time::Instant::now() >= clip.expires_at {
            return Err(VoiceErrorKind::InvalidAudio);
        }
        Ok(clip.wav)
    }

    fn complete(
        &self,
        attempt: &Attempt,
        result: CaptureResult<Sealed>,
    ) -> Option<tokio::time::Instant> {
        let mut table = self.lock();
        let Some(slot) = table
            .slot
            .as_mut()
            .filter(|slot| slot.attempt.id == attempt.id)
        else {
            attempt
                .update
                .send_modify(|update| update.terminal = Some(Err(VoiceErrorKind::Cancelled)));
            return None;
        };
        let result = if attempt.cancel.is_cancelled() {
            Err(VoiceErrorKind::Cancelled)
        } else {
            result
        };
        let (outcome, expiry) = match result {
            Ok(Sealed { wav, limit_reached }) => {
                let expires_at = tokio::time::Instant::now() + SEALED_LIFETIME;
                slot.clip = Some(SealedClip { wav, expires_at });
                (Ok(limit_reached), Some(expires_at))
            }
            Err(error) => {
                table.slot = None;
                (Err(error), None)
            }
        };
        attempt
            .update
            .send_modify(|update| update.terminal = Some(outcome));
        expiry
    }

    fn expire(&self, id: Uuid) {
        let mut table = self.lock();
        if table
            .slot
            .as_ref()
            .is_some_and(|slot| slot.attempt.id == id)
        {
            table.slot = None;
        }
    }
}

#[derive(Deserialize)]
struct Params {
    capture_id: Uuid,
}

#[derive(Clone, Copy)]
enum Control {
    Ready,
    Finish,
    Cancel,
}

pub fn register_capture(
    handlers: &mut HandlerRegistry,
    platform: Option<Arc<dyn DesktopPlatform>>,
) {
    let device = Arc::new(Semaphore::new(1));
    handlers.register_with_voice_context(VOICE_TURN_CAPTURE_METHOD, move |request, context| {
        let platform = platform.clone();
        let device = device.clone();
        async move {
            let outcome = async {
                check_desktop(&context)?;
                let id = capture_id(request.params.as_ref())?;
                let platform = platform.ok_or(VoiceErrorKind::CaptureUnavailable)?;
                if !platform
                    .media()
                    .is_some_and(|media| media.supports_voice_capture())
                {
                    return Err(VoiceErrorKind::CaptureUnavailable);
                }
                let lease = device
                    .try_acquire_owned()
                    .map_err(|_| VoiceErrorKind::CaptureUnavailable)?;
                let attempt = context.turns.capture.start(id, &context.cancellation)?;
                context.turns.offline.spawn(
                    supervise(platform, context.clone(), attempt.clone(), lease)
                        .with_subscriber(tracing::Dispatch::none()),
                );
                let limit_reached = attempt.terminal().await?;
                Ok(json!({ "capture_id": id, "limit_reached": limit_reached }))
            }
            .await;
            match outcome {
                Ok(result) => JsonRpcResponse::success(request.id, result),
                Err(error) => voice_error(request.id, error, &context),
            }
        }
    });
    for (method, control) in [
        (VOICE_TURN_CAPTURE_READY_METHOD, Control::Ready),
        (VOICE_TURN_CAPTURE_FINISH_METHOD, Control::Finish),
        (VOICE_TURN_CAPTURE_CANCEL_METHOD, Control::Cancel),
    ] {
        handlers.register_with_voice_context(method, move |request, context| async move {
            let outcome = async {
                check_desktop(&context)?;
                let id = capture_id(request.params.as_ref())?;
                let capture = &context.turns.capture;
                match control {
                    Control::Ready => {
                        super::until_cancelled(
                            &context.cancellation,
                            tokio::time::timeout(READY_WAIT, capture.ready(id)),
                        )
                        .await?
                        .map_err(|_| VoiceErrorKind::CaptureUnavailable)??;
                        Ok(json!({ "ready": true }))
                    }
                    Control::Finish => {
                        let attempt = capture
                            .lookup(id)?
                            .ok_or(VoiceErrorKind::CaptureUnavailable)?;
                        attempt.finish.cancel();
                        attempt.terminal().await?;
                        Ok(json!({}))
                    }
                    Control::Cancel => {
                        if let Some(attempt) = capture.cancel(id) {
                            let _ = attempt.terminal().await;
                        }
                        Ok(json!({}))
                    }
                }
            }
            .await;
            match outcome {
                Ok(result) => JsonRpcResponse::success(request.id, result),
                Err(error) => voice_error(request.id, error, &context),
            }
        });
    }
}

fn check_desktop(context: &VoiceRequestContext) -> CaptureResult<()> {
    context.check_admission()?;
    if context.ingress != VoiceIngress::DesktopIpc {
        return Err(VoiceErrorKind::PermissionDenied);
    }
    Ok(())
}

fn capture_id(value: Option<&serde_json::Value>) -> CaptureResult<Uuid> {
    value
        .and_then(|value| Params::deserialize(value).ok())
        .map(|params| params.capture_id)
        .ok_or(VoiceErrorKind::InvalidAudio)
}

struct Sealed {
    wav: Vec<u8>,
    limit_reached: bool,
}

async fn supervise(
    platform: Arc<dyn DesktopPlatform>,
    mut context: VoiceRequestContext,
    attempt: Arc<Attempt>,
    device: OwnedSemaphorePermit,
) {
    let recording = record(platform.as_ref(), &attempt);
    tokio::pin!(recording);
    let result = tokio::select! {
        result = &mut recording => result,
        () = tokio::time::sleep(CAPTURE_DEADLINE) => {
            attempt.cancel.cancel();
            recording.await
        }
    };
    // The media adapter has stopped or reaped the exact hardware owner before
    // returning. No normal cancellation path drops an in-flight adapter call.
    drop(device);
    drop(context.query_permit.take());
    let capture = &context.turns.capture;
    if let Some(expires_at) = capture.complete(&attempt, result) {
        tokio::select! {
            () = context.cancellation.cancelled() => {},
            () = attempt.released.cancelled() => {},
            () = tokio::time::sleep_until(expires_at) => {},
        }
        capture.expire(attempt.id);
    }
}

async fn record(platform: &dyn DesktopPlatform, attempt: &Attempt) -> CaptureResult<Sealed> {
    if attempt.cancel.is_cancelled() {
        return Err(VoiceErrorKind::Cancelled);
    }
    let media = platform.media().ok_or(VoiceErrorKind::CaptureUnavailable)?;
    let mut input = media
        .voice_capture_begin(attempt.cancel.clone())
        .await
        .map_err(|_| VoiceErrorKind::CaptureUnavailable)?;
    let mut vad = CaptureVad::default();
    let result = loop {
        if attempt.cancel.is_cancelled() {
            break Err(VoiceErrorKind::Cancelled);
        }
        if attempt.finish.is_cancelled() {
            break Ok(vad.finish());
        }
        let Ok(observations) = input
            .poll(vad.observed_samples() as u32, &attempt.cancel)
            .await
        else {
            break Err(VoiceErrorKind::CaptureUnavailable);
        };
        let mut decision = CaptureDecision::Continue;
        for observation in observations {
            decision = vad.observe(observation.end_sample as usize, observation.rms);
            if decision == CaptureDecision::InvalidInput {
                break;
            }
            attempt.update.send_if_modified(|update| {
                let became_ready = !update.ready;
                update.ready = true;
                became_ready
            });
            if decision != CaptureDecision::Continue {
                break;
            }
        }
        if decision != CaptureDecision::Continue {
            break Ok(decision);
        }
        tokio::select! {
            () = attempt.cancel.cancelled() => {},
            () = attempt.finish.cancelled() => {},
            () = tokio::time::sleep(POLL_INTERVAL) => {},
        }
    };
    match result {
        Ok(CaptureDecision::Seal {
            first_sample,
            end_sample,
            limit_reached,
        }) if !attempt.cancel.is_cancelled() => {
            let wav = input
                .seal(first_sample as u32, end_sample as u32, &attempt.cancel)
                .await
                .map_err(|_| VoiceErrorKind::CaptureUnavailable)?;
            validate_voice_wav(&wav)?;
            if wav.len()
                != WAV_HEADER_BYTES + (end_sample - first_sample) * PCM_SAMPLE_BYTES as usize
            {
                return Err(VoiceErrorKind::InvalidAudio);
            }
            Ok(Sealed { wav, limit_reached })
        }
        result => {
            input
                .cancel()
                .await
                .map_err(|_| VoiceErrorKind::CaptureUnavailable)?;
            Err(match result {
                Ok(CaptureDecision::NoSpeech) => VoiceErrorKind::NoSpeech,
                Err(error) => error,
                _ => VoiceErrorKind::CaptureUnavailable,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn voice_capture_early_cancel_never_starts_or_cancels_another_id() {
        let capture = ConnectionCapture::default();
        let owner = CancellationToken::new();
        let live = capture.start(Uuid::new_v4(), &owner).unwrap();
        let late = Uuid::new_v4();

        assert!(capture.cancel(late).is_none());
        assert!(!live.cancel.is_cancelled());
        assert_eq!(capture.ready(late).await, Err(VoiceErrorKind::Cancelled));
        assert!(matches!(
            capture.start(late, &owner),
            Err(VoiceErrorKind::Cancelled)
        ));
    }

    #[tokio::test]
    async fn voice_capture_ready_requires_a_sample_and_obeys_owner_cancellation() {
        let capture = ConnectionCapture::default();
        let owner = CancellationToken::new();
        let id = Uuid::new_v4();
        let attempt = capture.start(id, &owner).unwrap();
        let readiness = capture.ready(id);
        tokio::pin!(readiness);

        assert!(futures::poll!(&mut readiness).is_pending());
        attempt.update.send_modify(|update| update.ready = true);
        assert_eq!(readiness.await, Ok(()));
        owner.cancel();
        assert_eq!(capture.ready(id).await, Err(VoiceErrorKind::Cancelled));
    }

    #[tokio::test]
    async fn voice_capture_clip_is_owner_bound_single_use_and_never_consumed_after_expiry() {
        let capture = ConnectionCapture::default();
        let other = ConnectionCapture::default();
        let owner = CancellationToken::new();
        let id = Uuid::new_v4();
        let attempt = capture.start(id, &owner).unwrap();
        assert_eq!(capture.consume(id), Err(VoiceErrorKind::InvalidAudio));
        capture.complete(
            &attempt,
            Ok(Sealed {
                wav: vec![1, 2],
                limit_reached: false,
            }),
        );
        assert_eq!(other.consume(id), Err(VoiceErrorKind::InvalidAudio));
        assert_eq!(capture.consume(id), Ok(vec![1, 2]));
        assert_eq!(capture.consume(id), Err(VoiceErrorKind::InvalidAudio));

        let id = Uuid::new_v4();
        let attempt = capture.start(id, &owner).unwrap();
        capture.complete(
            &attempt,
            Ok(Sealed {
                wav: vec![3],
                limit_reached: true,
            }),
        );
        capture
            .lock()
            .slot
            .as_mut()
            .unwrap()
            .clip
            .as_mut()
            .unwrap()
            .expires_at = tokio::time::Instant::now();
        assert_eq!(capture.consume(id), Err(VoiceErrorKind::InvalidAudio));
        assert!(attempt.released.is_cancelled());
    }
}
