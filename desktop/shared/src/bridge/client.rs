//! Long-lived Swift helper RPC client.
//!
//! Spawns `aleph-bridge` once and keeps stdin / stdout / stderr open. Requests
//! are line-delimited JSON; responses are routed through the originating helper
//! generation's reply slots. Stderr is forwarded to tracing for diagnostics.
//!
//! On stdout EOF the reader retires only its own helper generation. Retirement
//! isolates its inflight requests and starts a separate kill/wait task; EOF
//! is not proof of process death. The [`SpawnGate`] still paces respawn.
//!
//! Respawn pacing: every (re)spawn goes through `ensure_running`, which asks the
//! `SpawnGate` whether enough time has elapsed since the last failure. Within
//! the backoff window it returns `DesktopError::BridgeBackoff` *without*
//! spawning, so a crash/spawn-failure loop spaces out (1s→2s→…→30s) instead of
//! hammering — the old code computed this delay and then discarded it, leaving
//! the ladder inert. The state lock is held across the check-and-spawn so two
//! concurrent first-callers cannot both spawn a helper.

use std::collections::{hash_map::Entry, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::runtime::Handle;
use tokio::sync::{oneshot, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use aleph_protocol::desktop_bridge::envelope::{Message, Request, RpcError};
use aleph_protocol::desktop_bridge::errors::{
    ERR_AX_REFERENCE, ERR_BRIDGE_DISABLED, ERR_HELPER_CRASHED, ERR_NOT_IMPLEMENTED,
    ERR_PERMISSION_DENIED, ERR_PLATFORM, ERR_TIMEOUT, ERR_UNSUPPORTED_LANGUAGE,
};
use aleph_protocol::desktop_bridge::methods::ax::{
    AxReferenceError, AxReferenceErrorData, METHOD_ACT_ON_REFERENCE, METHOD_SNAPSHOT_REFERENCES,
};
use aleph_protocol::desktop_bridge::methods::media::{
    METHOD_AUDIO_RECORD, METHOD_AUDIO_RECORD_START, METHOD_CAMERA_CLIP, METHOD_VOICE_CAPTURE_START,
};
use aleph_protocol::desktop_bridge::methods::perm::PermissionGuide;
use aleph_protocol::desktop_bridge::methods::suggested_timeout_ms;

use super::codec::{decode_line, encode};
use super::supervisor::{SpawnDecision, SpawnGate};
use crate::error::{DesktopError, Result};

/// Convert a bridge `RpcError` into a `DesktopError`.
///
/// The server-defined codes (`errors.rs`) carry semantics the caller needs to
/// distinguish, so each maps to its matching typed variant — most importantly
/// `-32002 NotImplemented` (the method exists, the capability is deliberately
/// absent, e.g. `pim.mail.*` on macOS) vs `-32601 MethodNotFound` (a real
/// wiring gap). `-32001 PermissionDenied` additionally carries a
/// `PermissionGuide` in `data` so the LLM can surface the deep link, steps, and
/// rationale. Codes with no typed home (parse / invalid-request / internal /
/// helper-crashed / anything unknown) fall through to `BridgeFailed`.
const OFFLINE_SPEECH_ROOT_ENV: &str = "ALEPH_OFFLINE_STT_ROOT";

/// Convert a Bridge RPC error after the caller has resolved resource ownership.
pub fn map_bridge_error(e: RpcError) -> DesktopError {
    match e.code {
        ERR_PERMISSION_DENIED => {
            if let Some(data) = e.data {
                if let Ok(guide) = serde_json::from_value::<PermissionGuide>(data) {
                    return DesktopError::PermissionDenied {
                        kind: guide.kind,
                        guide: Box::new(guide),
                    };
                }
            }
            DesktopError::BridgeFailed(format!("bridge error {}: {}", e.code, e.message))
        }
        ERR_AX_REFERENCE => match e
            .data
            .and_then(|data| serde_json::from_value::<AxReferenceErrorData>(data).ok())
        {
            Some(data) => DesktopError::AxReference {
                code: data.code,
                message: e.message,
            },
            None => DesktopError::BridgeFailed("invalid AX reference error data".into()),
        },
        ERR_NOT_IMPLEMENTED => DesktopError::NotImplemented(e.message),
        ERR_UNSUPPORTED_LANGUAGE => DesktopError::UnsupportedLanguage(e.message),
        ERR_PLATFORM => DesktopError::PlatformError(e.message),
        ERR_TIMEOUT => DesktopError::BridgeTimeout(e.message),
        ERR_BRIDGE_DISABLED => DesktopError::BridgeDisabled(e.message),
        // `-32005 HelperCrashed` has no dedicated variant; it means the helper
        // died mid-request, which is exactly `BridgeFailed`'s remit.
        ERR_HELPER_CRASHED => DesktopError::BridgeFailed(format!("helper crashed: {}", e.message)),
        _ => DesktopError::BridgeFailed(format!("bridge error {}: {}", e.code, e.message)),
    }
}

/// Map an older helper's missing reference RPC to an explicit capability refusal.
pub fn map_bridge_method_error(method: &str, error: RpcError) -> DesktopError {
    if error.code == -32601
        && matches!(method, METHOD_SNAPSHOT_REFERENCES | METHOD_ACT_ON_REFERENCE)
    {
        return DesktopError::AxReference {
            code: AxReferenceError::ReferenceUnsupported,
            message: "The installed desktop helper does not support native AX references".into(),
        };
    }
    map_bridge_error(error)
}

/// Fallback per-call RPC deadline, for a method the protocol has no opinion on.
///
/// Bounded so a helper that accepts a request and then hangs (without closing
/// stdout, so crash recovery never fires) cannot wedge an agent turn forever.
///
/// Almost nothing should reach this any more: [`SwiftBridge::call`] asks
/// [`suggested_timeout_ms`] first, and that answers for every namespace the
/// protocol defines. This is what is left for a method outside all of them.
///
/// Long-running capture operations (`camera.clip`, `audio.record`,
/// `speech.transcribe_file`) run for as long as the caller asked and so pass an
/// explicitly computed deadline to [`SwiftBridge::call_with_timeout`].
pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_mins(1);

/// The deadline [`SwiftBridge::call`] will use for `method`.
///
/// Exposed so a caller can reason about (or assert on) the budget a call will
/// get without having to reproduce the lookup.
#[must_use]
pub fn rpc_timeout_for(method: &str) -> Duration {
    suggested_timeout_ms(method).map_or(DEFAULT_RPC_TIMEOUT, Duration::from_millis)
}

/// JSON-RPC envelope version Aleph speaks to the Swift helper. Sent in the
/// `bridge.handshake` request and required to match the helper's reply.
pub const BRIDGE_PROTOCOL_VERSION: u32 = 2;

/// A decoded response from the helper, including JSON-RPC error replies.
/// Local transport failures are outside this result.
pub type BridgeRemoteReply = std::result::Result<serde_json::Value, RpcError>;

type InflightReply = Result<BridgeRemoteReply>;

// Decode the original line, not a Value or the untagged Message: both can
// hide duplicate fields or the presence of a second response branch.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundReplyEnvelope {
    jsonrpc: String,
    id: u64,
    #[serde(default, deserialize_with = "present_bound_value")]
    result: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_bound_error")]
    error: Option<RpcError>,
}

fn present_bound_error<'de, D>(deserializer: D) -> std::result::Result<Option<RpcError>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{value::MapAccessDeserializer, MapAccess, Visitor};
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct BoundError {
        code: i32,
        message: String,
        #[serde(default, deserialize_with = "present_bound_value")]
        data: Option<serde_json::Value>,
    }

    struct BoundErrorVisitor;

    impl<'de> Visitor<'de> for BoundErrorVisitor {
        type Value = RpcError;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON-RPC error object")
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> std::result::Result<RpcError, A::Error> {
            let BoundError {
                code,
                message,
                data,
            } = BoundError::deserialize(MapAccessDeserializer::new(map))?;
            Ok(RpcError {
                code,
                message,
                data: data.filter(|value| !value.is_null()),
            })
        }
    }

    deserializer.deserialize_map(BoundErrorVisitor).map(Some)
}

// Preserve result presence and reject duplicate direct keys in result/error
// data before Value normalizes them. Nested values are owned by the method.
fn present_bound_value<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{value::SeqAccessDeserializer, Error, MapAccess, SeqAccess, Visitor};
    use serde::Deserialize;
    use serde_json::Value;

    struct BoundResultVisitor;

    impl<'de> Visitor<'de> for BoundResultVisitor {
        type Value = Value;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON result without duplicate direct object keys")
        }

        fn visit_unit<E: Error>(self) -> std::result::Result<Value, E> {
            Ok(Value::Null)
        }

        fn visit_bool<E: Error>(self, value: bool) -> std::result::Result<Value, E> {
            Ok(value.into())
        }

        fn visit_i64<E: Error>(self, value: i64) -> std::result::Result<Value, E> {
            Ok(value.into())
        }

        fn visit_u64<E: Error>(self, value: u64) -> std::result::Result<Value, E> {
            Ok(value.into())
        }

        fn visit_f64<E: Error>(self, value: f64) -> std::result::Result<Value, E> {
            Ok(value.into())
        }

        fn visit_str<E: Error>(self, value: &str) -> std::result::Result<Value, E> {
            Ok(value.into())
        }

        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> std::result::Result<Value, A::Error> {
            Value::deserialize(SeqAccessDeserializer::new(seq))
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
            let mut object = serde_json::Map::new();
            while let Some((key, value)) = map.next_entry::<String, Value>()? {
                if object.insert(key, value).is_some() {
                    return Err(A::Error::custom("duplicate bound result key"));
                }
            }
            Ok(Value::Object(object))
        }
    }

    deserializer.deserialize_any(BoundResultVisitor).map(Some)
}

impl BoundReplyEnvelope {
    fn decode(line: &str, id: u64) -> InflightReply {
        let malformed = || DesktopError::BridgeFailed("malformed bound response envelope".into());
        // A derived struct also accepts positional sequences. Only objects
        // are JSON-RPC responses.
        if !line.trim_start().starts_with('{') {
            return Err(malformed());
        }
        let envelope: Self = decode_line(line).map_err(|_| malformed())?;
        if envelope.jsonrpc != "2.0" || envelope.id != id {
            return Err(malformed());
        }
        match (envelope.result, envelope.error) {
            (Some(result), None) => Ok(Ok(result)),
            (None, Some(error)) => Ok(Err(error)),
            _ => Err(malformed()),
        }
    }
}

enum ReplyKind {
    Ordinary,
    Bound,
}

struct ReplySlot {
    kind: ReplyKind,
    tx: oneshot::Sender<InflightReply>,
}

#[derive(Default)]
struct GenerationReplies {
    slots: Mutex<HashMap<u64, ReplySlot>>,
}

impl GenerationReplies {
    fn register(&self, id: u64, slot: ReplySlot) -> Result<()> {
        let mut slots = self.slots.lock().expect("bridge reply slots lock");
        match slots.entry(id) {
            Entry::Vacant(entry) => {
                entry.insert(slot);
                Ok(())
            }
            Entry::Occupied(_) => Err(DesktopError::BridgeFailed("duplicate request id".into())),
        }
    }

    fn remove(&self, id: u64) -> Option<ReplySlot> {
        self.slots
            .lock()
            .expect("bridge reply slots lock")
            .remove(&id)
    }

    fn route_line(&self, line: &str) {
        match decode_line::<Message>(line) {
            Ok(Message::Response(r)) => self.deliver(r.id, line, Ok(r.result)),
            Ok(Message::Error(e)) => {
                if let Some(id) = e.id {
                    self.deliver(id, line, Err(e.error));
                } else {
                    tracing::warn!(target: "bridge", "parse-error from helper: {}", e.error.message);
                }
            }
            Ok(Message::Notification(_)) => {}
            Err(err) => {
                // Raw lines can carry OCR, window titles or PIM data.
                tracing::warn!(target: "bridge", "decode failed: {err}");
                tracing::trace!(target: "bridge", "undecodable bridge line: {line:?}");
            }
        }
    }

    fn deliver(&self, id: u64, line: &str, ordinary: BridgeRemoteReply) {
        if let Some(slot) = self.remove(id) {
            let reply = match slot.kind {
                ReplyKind::Ordinary => Ok(ordinary),
                ReplyKind::Bound => BoundReplyEnvelope::decode(line, id),
            };
            let _ = slot.tx.send(reply);
        }
    }

    fn fail_unresolved(&self, reason: &str) {
        for (_, slot) in self.slots.lock().expect("bridge reply slots lock").drain() {
            let _ = slot.tx.send(Err(DesktopError::BridgeFailed(reason.into())));
        }
    }
}

/// Long-lived RPC client for the `aleph-bridge` Swift helper.
pub struct SwiftBridge {
    binary_path: PathBuf,
    offline_speech_root: Option<PathBuf>,
    #[cfg(all(test, unix))]
    stdout_frame_limit: usize,
    state: Arc<Mutex<BridgeState>>,
    id_seq: AtomicU64,
}

/// Owns an active helper resource. Calls never attach to a replacement;
/// dropping the binding schedules exact retirement, not a release proof.
#[must_use]
pub struct BoundBridgeGeneration {
    bridge: Arc<SwiftBridge>,
    generation: Arc<BridgeGeneration>,
    released: bool,
}

/// What a same-generation reply proves about an attempted acquisition.
pub enum BoundStartReply {
    Active,
    NotAcquired(DesktopError),
    Unproven(DesktopError),
}

struct BridgeState {
    process: Option<BridgeProcess>,
    retired_mic: Option<Arc<BridgeGeneration>>,
    gate: SpawnGate,
    disabled: bool,
}

struct BridgeProcess {
    child: Child,
    generation: Arc<BridgeGeneration>,
    reaped: watch::Sender<bool>,
    mic_claims: usize,
    open_recording_dispatched: bool,
}

struct BridgeGeneration {
    // Taking the handle for a write makes an interrupted or partial write
    // close this lane instead of letting a sibling append to partial JSON.
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    replies: GenerationReplies,
    retired: CancellationToken,
    drain_ready_replies: AtomicBool,
    reaped: watch::Receiver<bool>,
    runtime: Handle,
    #[cfg(all(test, unix))]
    pause_reaper: Mutex<Option<oneshot::Receiver<()>>>,
}

#[derive(Clone, Copy)]
enum Retirement {
    StdoutClosed,
    StdoutFrameRejected(&'static str),
    WriteLaneClosed { terminal_io_failure: bool },
    BoundAbandoned,
    BridgeDropped,
}

// Stdout multiplexes replies, so a frame cannot be trusted to identify its
// method before it is read. Leave room for inline 8K RGBA PNG screenshots
// (127 MiB raw), base64 expansion and Foundation escaping every slash.
const MAX_STDOUT_FRAME_BYTES: usize = 384 * 1024 * 1024;
const STDOUT_FRAME_TOO_LARGE: &str = "helper stdout frame reached byte limit without newline";

// Keep the partial frame outside the future: retirement can cancel a read
// before the old reader makes its final nonblocking drain of ready replies.
async fn read_stdout_frame<'a>(
    reader: &mut (impl AsyncBufRead + Unpin),
    frame: &'a mut Vec<u8>,
    limit: usize,
) -> std::result::Result<Option<&'a str>, &'static str> {
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|_| "helper stdout read failed")?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err("helper stdout frame ended without newline")
            };
        }
        let remaining = limit - frame.len();
        let readable = &available[..available.len().min(remaining)];
        let newline = readable.iter().position(|byte| *byte == b'\n');
        let length = newline.unwrap_or(readable.len());
        if length == remaining {
            return Err(STDOUT_FRAME_TOO_LARGE);
        }
        let needed = frame.len() + length;
        if needed > frame.capacity() {
            let capacity = needed.max(frame.capacity().saturating_mul(2)).min(limit);
            frame
                .try_reserve_exact(capacity - frame.len())
                .map_err(|_| "could not allocate helper stdout frame")?;
        }
        frame.extend_from_slice(&readable[..length]);
        reader.consume(length + usize::from(newline.is_some()));
        if newline.is_some() {
            return std::str::from_utf8(frame)
                .map(Some)
                .map_err(|_| "helper stdout frame is not UTF-8");
        }
    }
}

impl BridgeProcess {
    fn is_generation(&self, expected: &BridgeGeneration) -> bool {
        std::ptr::eq(self.generation.as_ref(), expected)
    }
}

impl BridgeState {
    fn mic_process(&mut self, expected: &BridgeGeneration) -> Option<&mut BridgeProcess> {
        if self
            .retired_mic
            .as_ref()
            .is_some_and(|old| !*old.reaped.borrow())
        {
            return None;
        }
        self.process.as_mut().filter(|p| p.is_generation(expected))
    }

    fn retire(&mut self, expected: &BridgeGeneration, reason: Retirement) {
        if !self
            .process
            .as_ref()
            .is_some_and(|p| p.is_generation(expected))
        {
            return;
        }
        let BridgeProcess {
            mut child,
            generation,
            reaped,
            mic_claims,
            open_recording_dispatched,
        } = self.process.take().expect("matched generation");
        if mic_claims != 0 || open_recording_dispatched {
            // The microphone entry gate forbids arming another generation
            // before this exact child is reaped, so there is at most one debt.
            self.retired_mic = Some(Arc::clone(&generation));
        }
        let (failure, charge_gate) = match reason {
            Retirement::StdoutClosed => (Some("helper stdout closed"), true),
            Retirement::StdoutFrameRejected(detail) => (Some(detail), true),
            Retirement::WriteLaneClosed {
                terminal_io_failure,
            } => {
                // The old reader owns its final ready-read pass. No new slot
                // can register once this generation has been detached.
                generation
                    .drain_ready_replies
                    .store(true, Ordering::Release);
                (None, terminal_io_failure)
            }
            Retirement::BoundAbandoned => (Some("helper retired without a release reply"), false),
            Retirement::BridgeDropped => (Some("bridge dropped"), false),
        };
        if charge_gate && self.gate.record_failure() {
            self.disabled = true;
            tracing::error!(target: "bridge", "bridge disabled: too many helper failures within the restart window");
        }
        generation.retired.cancel();
        if let Some(failure) = failure {
            generation.replies.fail_unresolved(failure);
        }
        // Detachment and reaper handoff have no suspension point. Dropping the
        // waiter, EOF racing retirement, or installing a replacement cannot
        // cancel this task or publish proof for the wrong child.
        #[cfg(all(test, unix))]
        let pause_reaper = generation.pause_reaper.lock().unwrap().take();
        generation.runtime.spawn(async move {
            #[cfg(all(test, unix))]
            if let Some(pause) = pause_reaper {
                let _ = pause.await;
            }
            if let Err(err) = child.start_kill() {
                tracing::warn!(target: "bridge", "helper kill request failed: {err}");
            }
            match child.wait().await {
                Ok(_) => {
                    reaped.send_replace(true);
                }
                Err(err) => {
                    tracing::error!(target: "bridge", "could not prove helper was reaped: {err}")
                }
            }
        });
    }
}

#[derive(Clone, Copy)]
enum MicClaim {
    UntilProvenReleased,
    UntilReaped,
}

impl MicClaim {
    fn for_legacy_request(request: &Request) -> Option<Self> {
        match request.method.as_str() {
            METHOD_AUDIO_RECORD => Some(Self::UntilProvenReleased),
            METHOD_AUDIO_RECORD_START => Some(Self::UntilReaped),
            METHOD_CAMERA_CLIP
                if request.params.as_ref().and_then(|params| {
                    params
                        .get("with_audio")
                        .and_then(serde_json::Value::as_bool)
                }) == Some(true) =>
            {
                Some(Self::UntilProvenReleased)
            }
            _ => None,
        }
    }
}

enum WriteAttempt<'a> {
    Ordinary,
    OrdinaryRetry,
    LegacyMic(MicClaim),
    Bound(&'a mut bool),
    VoiceCaptureStart(&'a mut bool),
}

#[derive(Clone, Copy)]
enum BoundTarget<'a> {
    Current,
    VoiceCaptureStart,
    Pinned(&'a Arc<BridgeGeneration>),
}

struct StdinWriteOwner<'a> {
    bridge: &'a SwiftBridge,
    generation: &'a BridgeGeneration,
    attempt: WriteAttempt<'a>,
    slot: tokio::sync::MutexGuard<'a, Option<ChildStdin>>,
    stdin: Option<ChildStdin>,
}

impl StdinWriteOwner<'_> {
    fn finish(mut self, result: std::io::Result<()>) -> std::io::Result<()> {
        let stdin = self.stdin.take();
        if result.is_ok() {
            *self.slot = stdin;
        } else {
            // Retire under the write lock before another writer can
            // mistake this broken lane for its own failure.
            self.bridge.retire(
                self.generation,
                Retirement::WriteLaneClosed {
                    terminal_io_failure: !matches!(self.attempt, WriteAttempt::Ordinary),
                },
            );
        }
        result
    }
}

impl Drop for StdinWriteOwner<'_> {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            // Classify cancellation before closing stdin can trigger stdout EOF.
            let reason = match self.attempt {
                WriteAttempt::Bound(_) | WriteAttempt::VoiceCaptureStart(_) => {
                    Retirement::BoundAbandoned
                }
                WriteAttempt::Ordinary
                | WriteAttempt::OrdinaryRetry
                | WriteAttempt::LegacyMic(_) => Retirement::WriteLaneClosed {
                    terminal_io_failure: false,
                },
            };
            self.bridge.retire(self.generation, reason);
        }
        drop(self.stdin.take());
    }
}

impl BridgeGeneration {
    async fn write(
        &self,
        bridge: &SwiftBridge,
        line: &str,
        attempt: WriteAttempt<'_>,
    ) -> std::io::Result<()> {
        tokio::select! {
            biased;
            () = self.retired.cancelled() => Err(std::io::ErrorKind::BrokenPipe.into()),
            result = async {
                let mut slot = self.stdin.lock().await;
                if self.retired.is_cancelled() {
                    return Err(std::io::ErrorKind::BrokenPipe.into());
                }
                let Some(stdin) = slot.take() else {
                    // A cancelled write closed the lane, not a new I/O error.
                    bridge.retire(self, Retirement::WriteLaneClosed { terminal_io_failure: false });
                    return Err(std::io::ErrorKind::BrokenPipe.into());
                };
                let mut attempt = attempt;
                // Mark the resource before beginning I/O, under the same
                // lock that detaches the child and publishes its reap proof.
                let may_write = match &mut attempt {
                    WriteAttempt::VoiceCaptureStart(armed) => {
                        if bridge.begin_mic_claim(self, MicClaim::UntilProvenReleased) {
                            **armed = true;
                            true
                        } else {
                            false
                        }
                    }
                    WriteAttempt::LegacyMic(kind) => {
                        bridge.begin_mic_claim(self, *kind)
                    }
                    WriteAttempt::Bound(armed) => {
                        **armed = true;
                        true
                    }
                    _ => true,
                };
                if !may_write {
                    *slot = Some(stdin);
                    return Err(std::io::ErrorKind::BrokenPipe.into());
                }
                let mut owner = StdinWriteOwner {
                    bridge,
                    generation: self,
                    attempt,
                    slot,
                    stdin: Some(stdin),
                };
                let stdin = owner.stdin.as_mut().expect("write owner holds stdin");
                let result = async {
                    stdin.write_all(line.as_bytes()).await?;
                    stdin.flush().await
                }.await;
                owner.finish(result)
            } => result,
        }
    }

    async fn wait_reaped(&self) {
        let mut proof = self.reaped.clone();
        if proof.wait_for(|reaped| *reaped).await.is_err() {
            // Failed wait, dropped reaper or runtime shutdown are not proof.
            std::future::pending::<()>().await;
        }
    }
}

struct PendingReply<'a> {
    generation: &'a BridgeGeneration,
    id: u64,
    rx: oneshot::Receiver<InflightReply>,
}

impl PendingReply<'_> {
    async fn receive(&mut self) -> InflightReply {
        (&mut self.rx)
            .await
            .unwrap_or_else(|_| Err(DesktopError::BridgeFailed("inflight dropped".into())))
    }
}

impl Drop for PendingReply<'_> {
    fn drop(&mut self) {
        self.generation.replies.remove(self.id);
    }
}

struct BoundDispatch<'a> {
    bridge: &'a SwiftBridge,
    generation: &'a BridgeGeneration,
    armed: bool,
}

impl Drop for BoundDispatch<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.bridge
                .retire(self.generation, Retirement::BoundAbandoned);
        }
    }
}

impl BoundBridgeGeneration {
    /// Call the helper that acquired the resource, retaining the binding on success.
    pub async fn call<P: serde::Serialize, T>(
        &mut self,
        method: &str,
        params: P,
        cancel: &CancellationToken,
        validate: impl FnOnce(BridgeRemoteReply) -> Result<T>,
    ) -> Result<T> {
        let (result, _) = self
            .bridge
            .dispatch_bound(
                BoundTarget::Pinned(&self.generation),
                method,
                params,
                rpc_timeout_for(method),
                cancel,
                |reply| {
                    let result = validate(reply);
                    (result.is_ok(), result)
                },
            )
            .await?;
        result
    }

    /// Consume the binding. Even a call abandoned before writing cannot leave
    /// the already-acquired resource live: retire and reap on unproven release.
    pub async fn finish<P: serde::Serialize, T>(
        mut self,
        method: &str,
        params: P,
        cancel: &CancellationToken,
        proves_release: impl FnOnce(BridgeRemoteReply) -> Result<T>,
    ) -> Result<T> {
        let result = self.call(method, params, cancel, proves_release).await;
        if result.is_err() {
            self.bridge
                .retire(&self.generation, Retirement::BoundAbandoned);
            self.generation.wait_reaped().await;
        } else {
            self.bridge.clear_mic_claim(&self.generation);
        }
        self.released = true;
        result
    }
}

impl Drop for BoundBridgeGeneration {
    fn drop(&mut self) {
        if !self.released {
            self.bridge
                .retire(&self.generation, Retirement::BoundAbandoned);
        }
    }
}

impl Drop for SwiftBridge {
    fn drop(&mut self) {
        let mut state = self.state.lock().expect("bridge state lock");
        if let Some(generation) = state.process.as_ref().map(|p| Arc::clone(&p.generation)) {
            state.retire(&generation, Retirement::BridgeDropped);
        }
    }
}

impl SwiftBridge {
    #[must_use]
    pub fn new(binary_path: PathBuf) -> Self {
        Self::new_with_offline_speech_root(binary_path, None)
    }

    /// Construct the lazy client with Core's trusted offline staging root.
    #[must_use]
    pub fn new_with_offline_speech_root(
        binary_path: PathBuf,
        offline_speech_root: Option<PathBuf>,
    ) -> Self {
        Self {
            binary_path,
            offline_speech_root,
            #[cfg(all(test, unix))]
            stdout_frame_limit: MAX_STDOUT_FRAME_BYTES,
            state: Arc::new(Mutex::new(BridgeState {
                process: None,
                retired_mic: None,
                gate: SpawnGate::new(5, Duration::from_mins(10)),
                disabled: false,
            })),
            id_seq: AtomicU64::new(1),
        }
    }

    /// Return the exact root this client binds to each helper generation.
    #[must_use]
    pub fn offline_speech_root(&self) -> Option<&Path> {
        self.offline_speech_root.as_deref()
    }

    /// Returns `true` if the helper subprocess is currently spawned (its
    /// reader loop is live).
    ///
    /// Cheap, non-blocking diagnostic: it `try_lock`s the state slot and never
    /// awaits. Used to verify the bridge stays idle (unspawned) until the first
    /// real `desktop.*` call. A momentary lock contention conservatively
    /// reports `false`.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state.try_lock().is_ok_and(|g| g.process.is_some())
    }

    /// Spawn the helper subprocess and wire up reader + stderr tasks.
    ///
    /// This is the inner spawn — it does NOT touch the `SpawnGate` and does NOT
    /// install the process into `self.state`; it returns the live
    /// [`BridgeProcess`] so the caller (`ensure_running`) can store it while
    /// still holding the state lock, closing the double-spawn race.
    fn spawn_process(&self) -> Result<BridgeProcess> {
        let mut cmd = Command::new(&self.binary_path);
        if let Some(root) = &self.offline_speech_root {
            cmd.env(OFFLINE_SPEECH_ROOT_ENV, root);
        } else {
            cmd.env_remove(OFFLINE_SPEECH_ROOT_ENV);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Kill the helper when its `Child` handle is dropped (state-slot
            // reset, bridge teardown) — otherwise a superseded helper outlives
            // the bridge as an orphan. Same idiom as
            // `script_exec::output_capped`.
            .kill_on_drop(true);

        // Place the helper in its own process group so it inherits the
        // parent's controlling terminal signals (SIGTERM/SIGKILL on
        // aleph-server shutdown propagate cleanly). The Swift helper also
        // polls getppid() and exits if it gets reparented to init (ppid=1).
        #[cfg(unix)]
        // SAFETY: the pre_exec closure only calls async-signal-safe syscalls
        // (setpgid) and returns an error via io::Error, not panic.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|e| {
            DesktopError::BridgeFailed(format!("spawn {}: {e}", self.binary_path.display()))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| DesktopError::BridgeFailed("missing stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DesktopError::BridgeFailed("missing stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| DesktopError::BridgeFailed("missing stderr".into()))?;

        let (reaped, proof) = watch::channel(false);
        let generation = Arc::new(BridgeGeneration {
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            replies: GenerationReplies::default(),
            retired: CancellationToken::new(),
            drain_ready_replies: AtomicBool::new(false),
            reaped: proof,
            runtime: Handle::current(),
            #[cfg(all(test, unix))]
            pause_reaper: Mutex::new(None),
        });
        let reader_generation = Arc::clone(&generation);
        let state = Arc::downgrade(&self.state);
        #[cfg(all(test, unix))]
        let frame_limit = self.stdout_frame_limit;
        #[cfg(not(all(test, unix)))]
        let frame_limit = MAX_STDOUT_FRAME_BYTES;
        tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            let mut frame = Vec::new();
            let frame_failure = loop {
                let result = tokio::select! {
                    biased;
                    () = reader_generation.retired.cancelled() => break None,
                    result = read_stdout_frame(&mut stdout, &mut frame, frame_limit) => result,
                };
                match result {
                    Ok(Some(line)) => {
                        reader_generation.replies.route_line(line);
                        frame.clear();
                        // Do not retain a screenshot-sized allocation for the
                        // remainder of this helper's lifetime.
                        if frame.capacity() > 1024 * 1024 {
                            frame = Vec::new();
                        }
                    }
                    status => {
                        let failure = status.err();
                        let reason = failure
                            .map_or(Retirement::StdoutClosed, Retirement::StdoutFrameRejected);
                        tracing::warn!(target: "bridge", detail = failure.unwrap_or("helper stdout closed"), "reader loop exited");
                        if let Some(state) = state.upgrade() {
                            state
                                .lock()
                                .expect("bridge state lock")
                                .retire(&reader_generation, reason);
                        }
                        break failure;
                    }
                }
            };

            if reader_generation
                .drain_ready_replies
                .load(Ordering::Acquire)
            {
                // Keep complete replies the old pipe already has available,
                // but never wait for more bytes or for the reaper.
                let failure = if let Some(detail) = frame_failure {
                    detail
                } else {
                    loop {
                        let result = tokio::select! {
                            biased;
                            result = read_stdout_frame(&mut stdout, &mut frame, frame_limit) => result,
                            () = std::future::ready(()) => break "write stdin failed",
                        };
                        match result {
                            Ok(Some(line)) => reader_generation.replies.route_line(line),
                            Ok(None) => break "write stdin failed",
                            Err(detail) => break detail,
                        }
                        frame.clear();
                    }
                };
                reader_generation.replies.fail_unresolved(failure);
            }
        });

        // Stderr forwarder → tracing.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(target: "bridge_stderr", "{line}");
            }
        });

        Ok(BridgeProcess {
            child,
            generation,
            reaped,
            mic_claims: 0,
            open_recording_dispatched: false,
        })
    }

    /// Ensure that a helper is installed, respecting disabled mode and backoff.
    pub async fn ensure_running(&self) -> Result<()> {
        self.ensure_generation().map(|_| ())
    }

    fn ensure_generation(&self) -> Result<Arc<BridgeGeneration>> {
        self.ensure_generation_locked(&mut self.state.lock().expect("bridge state lock"))
    }

    async fn mic_generation(&self) -> Result<Arc<BridgeGeneration>> {
        loop {
            let old = {
                let mut state = self.state.lock().expect("bridge state lock");
                match state.retired_mic.as_ref() {
                    Some(old) if !*old.reaped.borrow() => Arc::clone(old),
                    _ => {
                        state.retired_mic = None;
                        return self.ensure_generation_locked(&mut state);
                    }
                }
            };
            old.wait_reaped().await;
        }
    }

    fn begin_mic_claim(&self, expected: &BridgeGeneration, claim: MicClaim) -> bool {
        let mut state = self.state.lock().expect("bridge state lock");
        let Some(process) = state.mic_process(expected) else {
            return false;
        };
        if matches!(claim, MicClaim::UntilReaped) {
            // Stop replies are not tied to a particular start, and another
            // dispatched start can acquire across a stop. Only reaping proves
            // all open-ended requests on this generation are finished.
            process.open_recording_dispatched = true;
        } else {
            process.mic_claims += 1;
        }
        true
    }

    fn clear_mic_claim(&self, expected: &BridgeGeneration) {
        let mut state = self.state.lock().expect("bridge state lock");
        if let Some(process) = state.process.as_mut().filter(|p| p.is_generation(expected)) {
            process.mic_claims -= 1;
        }
    }

    fn ensure_generation_locked(&self, state: &mut BridgeState) -> Result<Arc<BridgeGeneration>> {
        if state.disabled {
            return Err(DesktopError::BridgeDisabled(
                "restart threshold exceeded".into(),
            ));
        }
        if let Some(process) = &state.process {
            return Ok(Arc::clone(&process.generation));
        }
        match state.gate.poll() {
            SpawnDecision::Go => {}
            SpawnDecision::Backoff { remaining } => {
                return Err(DesktopError::BridgeBackoff(format!(
                    "helper recovering; retry in {}s",
                    remaining.as_secs().max(1)
                )));
            }
        }
        match self.spawn_process() {
            Ok(process) => {
                let generation = Arc::clone(&process.generation);
                state.process = Some(process);
                state.gate.record_spawn();
                Ok(generation)
            }
            Err(e) => {
                if state.gate.record_failure() {
                    state.disabled = true;
                    tracing::error!(target: "bridge", "bridge disabled: too many spawn failures");
                    return Err(DesktopError::BridgeDisabled(
                        "restart threshold exceeded".into(),
                    ));
                }
                Err(e)
            }
        }
    }

    fn retire(&self, generation: &BridgeGeneration, reason: Retirement) {
        self.state
            .lock()
            .expect("bridge state lock")
            .retire(generation, reason);
    }

    fn register<'a>(
        &self,
        generation: &'a BridgeGeneration,
        id: u64,
        kind: ReplyKind,
    ) -> Result<PendingReply<'a>> {
        let state = self.state.lock().expect("bridge state lock");
        if !state
            .process
            .as_ref()
            .is_some_and(|p| p.is_generation(generation))
        {
            return Err(DesktopError::BridgeFailed("bridge not running".into()));
        }
        let (tx, rx) = oneshot::channel();
        generation.replies.register(id, ReplySlot { kind, tx })?;
        Ok(PendingReply { generation, id, rx })
    }

    fn request(&self, method: &str, params: impl serde::Serialize) -> Result<Request> {
        Ok(Request {
            jsonrpc: "2.0".into(),
            id: self.id_seq.fetch_add(1, Ordering::SeqCst),
            method: method.into(),
            params: Some(
                serde_json::to_value(params)
                    .map_err(|e| DesktopError::BridgeFailed(format!("serialize params: {e}")))?,
            ),
        })
    }

    /// Send a JSON-RPC request and await the typed response, bounded by the
    /// deadline the protocol declares for `method` (see [`rpc_timeout_for`]).
    ///
    /// Most callers want this — the per-method budget is the protocol's to
    /// state, not each limb's to remember. Only an operation whose length is a
    /// function of its *arguments* (camera/audio capture, whose duration the
    /// caller chose) needs [`SwiftBridge::call_with_timeout`].
    pub async fn call<P, R>(&self, method: &str, params: P) -> Result<R>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.call_with_timeout(method, params, rpc_timeout_for(method))
            .await
    }

    /// Send a JSON-RPC request and await the typed response, bounded by an
    /// explicit per-call reply `timeout`. Ordinary writes get one gated retry.
    /// Microphone acquisitions wait for old capture reaping and never retry.
    pub async fn call_with_timeout<P, R>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<R>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        if method == METHOD_VOICE_CAPTURE_START {
            return Err(DesktopError::BridgeFailed(
                "native capture requires bound start".into(),
            ));
        }
        let mut request = self.request(method, params)?;
        if let Some(kind) = MicClaim::for_legacy_request(&request) {
            let generation = tokio::time::timeout(timeout, self.mic_generation())
                .await
                .map_err(|_| {
                    DesktopError::BridgeTimeout(format!(
                        "'{method}' waiting for prior microphone release"
                    ))
                })??;
            let line = encode(&request)?;
            let pending = self
                .send(
                    &generation,
                    request.id,
                    &line,
                    WriteAttempt::LegacyMic(kind),
                )
                .await?;
            let result = Self::decode_reply(pending, timeout, method).await;
            // The timed recorder finishes and the camera stops its session
            // before the helper returns success for this particular request.
            if result.is_ok() && matches!(kind, MicClaim::UntilProvenReleased) {
                self.clear_mic_claim(&generation);
            }
            return result;
        }
        let mut generation = self.ensure_generation()?;
        for retry in [false, true] {
            let line = encode(&request)?;
            let attempt = if retry {
                WriteAttempt::OrdinaryRetry
            } else {
                WriteAttempt::Ordinary
            };
            match self.send(&generation, request.id, &line, attempt).await {
                Ok(pending) => return Self::decode_reply(pending, timeout, method).await,
                Err(err) => {
                    if retry {
                        return Err(err);
                    }
                    tracing::warn!(target: "bridge", "stdin write failed, will retry: {err}");
                }
            }
            generation = self.ensure_generation()?;
            request.id = self.id_seq.fetch_add(1, Ordering::SeqCst);
        }
        unreachable!("the retry always returns")
    }

    async fn send<'a>(
        &self,
        generation: &'a BridgeGeneration,
        id: u64,
        line: &str,
        attempt: WriteAttempt<'_>,
    ) -> Result<PendingReply<'a>> {
        let kind = if matches!(
            attempt,
            WriteAttempt::LegacyMic(MicClaim::UntilProvenReleased)
        ) {
            ReplyKind::Bound
        } else {
            ReplyKind::Ordinary
        };
        let reply = self.register(generation, id, kind)?;
        let retry = matches!(attempt, WriteAttempt::OrdinaryRetry);
        generation.write(self, line, attempt).await.map_err(|e| {
            let operation = if retry {
                "write stdin retry"
            } else {
                "write stdin"
            };
            DesktopError::BridgeFailed(format!("{operation}: {e}"))
        })?;
        Ok(reply)
    }

    async fn decode_reply<R: serde::de::DeserializeOwned>(
        mut pending: PendingReply<'_>,
        timeout: Duration,
        method: &str,
    ) -> Result<R> {
        let remote = tokio::time::timeout(timeout, pending.receive())
            .await
            .map_err(|_| {
                DesktopError::BridgeTimeout(format!(
                    "no reply for '{method}' within {}s",
                    timeout.as_secs()
                ))
            })??;
        serde_json::from_value(remote.map_err(|err| map_bridge_method_error(method, err))?)
            .map_err(|e| DesktopError::BridgeFailed(format!("decode result: {e}")))
    }

    /// Dispatch at most once to the helper installed under the state lock.
    /// The method-specific predicate may accept only a definitive same-helper
    /// reply proving the operation never acquired or has released its resource.
    /// Every other dispatched outcome first retires that helper and waits for
    /// successful reaping (which can outlast `timeout` and abort sibling RPCs).
    /// Dropping this future requests retirement but provides no release proof;
    /// keep resources owned by a task that survives outer caller cancellation.
    pub async fn call_bound<P: serde::Serialize>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
        cancel: &CancellationToken,
        proves_release: impl FnOnce(&BridgeRemoteReply) -> bool,
    ) -> Result<BridgeRemoteReply> {
        let (reply, _) = self
            .dispatch_bound(
                BoundTarget::Current,
                method,
                params,
                timeout,
                cancel,
                |reply| (proves_release(&reply), reply),
            )
            .await?;
        Ok(reply)
    }

    /// Dispatch microphone capture activation once. Only an accepted
    /// activation transfers ownership of the dispatched generation.
    pub async fn call_bound_start<P: serde::Serialize>(
        self: &Arc<Self>,
        method: &str,
        params: P,
        cancel: &CancellationToken,
        classify_activation: impl FnOnce(BridgeRemoteReply) -> BoundStartReply,
    ) -> Result<BoundBridgeGeneration> {
        let (result, generation) = self
            .dispatch_bound(
                BoundTarget::VoiceCaptureStart,
                method,
                params,
                rpc_timeout_for(method),
                cancel,
                |reply| match classify_activation(reply) {
                    BoundStartReply::Active => (true, Ok(())),
                    BoundStartReply::NotAcquired(err) => (true, Err(err)),
                    BoundStartReply::Unproven(err) => (false, Err(err)),
                },
            )
            .await?;
        if let Err(err) = result {
            if let Some(generation) = generation {
                self.clear_mic_claim(&generation);
            }
            return Err(err);
        }
        Ok(BoundBridgeGeneration {
            bridge: Arc::clone(self),
            generation: generation.expect("accepted activation was dispatched"),
            released: false,
        })
    }

    async fn dispatch_bound<P: serde::Serialize, T>(
        &self,
        target: BoundTarget<'_>,
        method: &str,
        params: P,
        timeout: Duration,
        cancel: &CancellationToken,
        classify_reply: impl FnOnce(BridgeRemoteReply) -> (bool, T),
    ) -> Result<(T, Option<Arc<BridgeGeneration>>)> {
        let cancelled = || DesktopError::BridgeFailed(format!("'{method}' cancelled"));
        let timed_out = || DesktopError::BridgeTimeout(format!("'{method}' deadline exceeded"));
        let deadline = Instant::now() + timeout;
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let request = self.request(method, params)?;
        if MicClaim::for_legacy_request(&request).is_some()
            || (method == METHOD_VOICE_CAPTURE_START
                && !matches!(target, BoundTarget::VoiceCaptureStart))
        {
            return Err(DesktopError::BridgeFailed(
                "microphone acquisition requires its own dispatch path".into(),
            ));
        }
        let line = encode(&request)?;
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        let generation = match target {
            BoundTarget::Pinned(generation) => Arc::clone(generation),
            BoundTarget::Current => self.ensure_generation()?,
            BoundTarget::VoiceCaptureStart => tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(cancelled()),
                () = tokio::time::sleep_until(deadline) => return Err(timed_out()),
                generation = self.mic_generation() => generation?,
            },
        };
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        let mut pending = self.register(&generation, request.id, ReplyKind::Bound)?;
        let mut dispatch = BoundDispatch {
            bridge: self,
            generation: &generation,
            armed: false,
        };
        let reply = tokio::select! {
            biased;
            () = cancel.cancelled() => Err(cancelled()),
            () = tokio::time::sleep_until(deadline) => Err(timed_out()),
            reply = async {
                let attempt = if matches!(target, BoundTarget::VoiceCaptureStart) {
                    WriteAttempt::VoiceCaptureStart(&mut dispatch.armed)
                } else {
                    WriteAttempt::Bound(&mut dispatch.armed)
                };
                generation
                    .write(self, &line, attempt)
                    .await
                    .map_err(|e| DesktopError::BridgeFailed(format!("write stdin: {e}")))?;
                pending.receive().await
            } => reply,
        };
        drop(pending);
        let (accepted, result) = match reply {
            Ok(reply) => {
                let (accepted, value) = classify_reply(reply);
                (accepted, Ok(value))
            }
            Err(err) => (false, Err(err)),
        };
        if dispatch.armed && !accepted {
            self.retire(&generation, Retirement::BoundAbandoned);
            generation.wait_reaped().await;
        }
        dispatch.armed = false;
        drop(dispatch);
        result.map(|value| (value, accepted.then_some(generation)))
    }

    /// Send the Stage 0 `bridge.handshake` request and return the helper's
    /// version + supported method list. Mostly used at startup + in e2e
    /// tests; individual capabilities do not need to call this first.
    pub async fn handshake(
        &self,
        rust_version: &str,
    ) -> Result<aleph_protocol::desktop_bridge::methods::bridge::HandshakeResult> {
        use aleph_protocol::desktop_bridge::methods::bridge::{
            HandshakeParams, HandshakeResult, METHOD_HANDSHAKE,
        };
        let result: HandshakeResult = self
            .call(
                METHOD_HANDSHAKE,
                HandshakeParams {
                    rust_version: rust_version.into(),
                    protocol_version: BRIDGE_PROTOCOL_VERSION,
                },
            )
            .await?;
        // Negotiate explicitly: a helper speaking a different envelope version
        // would mis-decode params/results silently, so fail loud and early.
        if result.protocol_version != BRIDGE_PROTOCOL_VERSION {
            return Err(DesktopError::BridgeFailed(format!(
                "protocol mismatch: core speaks v{BRIDGE_PROTOCOL_VERSION}, \
                 helper '{}' speaks v{}",
                result.swift_version, result.protocol_version
            )));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod stdout_frame_tests {
    use super::*;

    #[tokio::test]
    async fn stdout_frame_rejects_at_byte_limit_without_eof_or_newline() {
        let (mut tx, rx) = tokio::io::duplex(16);
        let mut reader = BufReader::with_capacity(3, rx);
        let mut frame = Vec::new();
        tx.write_all(b"1234567").await.unwrap();
        tokio::select! {
            biased;
            result = read_stdout_frame(&mut reader, &mut frame, 8) => panic!("rejected early: {result:?}"),
            () = std::future::ready(()) => {},
        }
        assert_eq!(frame, b"1234567");
        tx.write_all(b"8").await.unwrap();
        tokio::select! {
            biased;
            result = read_stdout_frame(&mut reader, &mut frame, 8) => assert_eq!(result, Err(STDOUT_FRAME_TOO_LARGE)),
            () = std::future::ready(()) => panic!("waited for EOF or newline at the limit"),
        }
        assert_eq!(frame, b"1234567");
        assert!(frame.capacity() <= 8);
    }

    #[tokio::test]
    async fn stdout_frame_accepts_limit_including_newline_but_not_partial_eof() {
        let (mut tx, rx) = tokio::io::duplex(16);
        let mut reader = BufReader::with_capacity(3, rx);
        let mut frame = Vec::new();
        tx.write_all(b"1234567\n{}").await.unwrap();
        drop(tx);
        assert_eq!(
            read_stdout_frame(&mut reader, &mut frame, 8).await,
            Ok(Some("1234567"))
        );
        frame.clear();
        assert_eq!(
            read_stdout_frame(&mut reader, &mut frame, 8).await,
            Err("helper stdout frame ended without newline")
        );
    }
}

#[cfg(test)]
mod mic_dispatch_tests {
    use super::*;

    #[tokio::test]
    async fn generic_dispatch_cannot_bypass_mic_acquisition_tracking() {
        let bridge = SwiftBridge::new(PathBuf::new());
        let cancel = CancellationToken::new();
        for (method, params) in [
            (METHOD_AUDIO_RECORD_START, serde_json::json!({})),
            (METHOD_AUDIO_RECORD, serde_json::json!({})),
            (METHOD_CAMERA_CLIP, serde_json::json!({"with_audio": true})),
            (METHOD_VOICE_CAPTURE_START, serde_json::json!({})),
        ] {
            let result = bridge
                .call_bound(method, params, Duration::from_secs(1), &cancel, |_| true)
                .await;
            assert!(
                matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail == "microphone acquisition requires its own dispatch path")
            );
        }
        let result = bridge
            .call::<_, serde_json::Value>(METHOD_VOICE_CAPTURE_START, ())
            .await;
        assert!(
            matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail == "native capture requires bound start")
        );
        assert!(!bridge.is_running());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Fake helper: reads a JSON line, extracts `"id":<N>`, and emits a
    /// matching success response.
    fn fake_helper_script() -> &'static str {
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"result":{"pong":true}}\n' "$id"
done
"#
    }

    /// Fake helper that emits an error response for every request.
    fn failing_helper_script() -> &'static str {
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"bad params"}}\n' "$id"
done
"#
    }

    /// Fake helper that consumes every request but never replies, simulating
    /// a wedged helper that keeps stdout open (so crash recovery never fires).
    fn silent_helper_script() -> &'static str {
        r#"#!/bin/sh
while IFS= read -r line; do
  :
done
"#
    }

    fn install_fake(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("fake-bridge");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn call_returns_result_from_fake_helper() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, fake_helper_script());

        let bridge = SwiftBridge::new(path);
        bridge.ensure_running().await.unwrap();
        let v: serde_json::Value = bridge
            .call("bridge.ping", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(v["pong"], true);
    }

    #[tokio::test]
    async fn call_propagates_bridge_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, failing_helper_script());

        let bridge = SwiftBridge::new(path);
        bridge.ensure_running().await.unwrap();
        let result: Result<serde_json::Value> =
            bridge.call("bridge.ping", serde_json::json!({})).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("bad params"), "unexpected error: {msg}");
    }

    #[test]
    fn map_bridge_error_routes_server_codes_to_typed_variants() {
        let err = |code: i32| RpcError {
            code,
            message: "detail".into(),
            data: None,
        };
        // -32002 must stay distinguishable from a -32601 wiring gap: it means
        // the capability is deliberately absent (e.g. pim.mail.* on macOS).
        assert!(matches!(
            map_bridge_error(err(ERR_NOT_IMPLEMENTED)),
            DesktopError::NotImplemented(_)
        ));
        assert!(matches!(
            map_bridge_error(err(ERR_UNSUPPORTED_LANGUAGE)),
            DesktopError::UnsupportedLanguage(_)
        ));
        assert!(matches!(
            map_bridge_error(err(ERR_PLATFORM)),
            DesktopError::PlatformError(_)
        ));
        assert!(matches!(
            map_bridge_error(err(ERR_TIMEOUT)),
            DesktopError::BridgeTimeout(_)
        ));
        assert!(matches!(
            map_bridge_error(err(ERR_BRIDGE_DISABLED)),
            DesktopError::BridgeDisabled(_)
        ));
        // Codes with no typed home fall through to BridgeFailed.
        assert!(matches!(
            map_bridge_error(err(-32601)),
            DesktopError::BridgeFailed(_)
        ));
    }

    #[tokio::test]
    async fn concurrent_calls_get_distinct_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, fake_helper_script());

        let bridge = Arc::new(SwiftBridge::new(path));
        bridge.ensure_running().await.unwrap();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let b = Arc::clone(&bridge);
            handles.push(tokio::spawn(async move {
                let v: serde_json::Value =
                    b.call("bridge.ping", serde_json::json!({})).await.unwrap();
                assert_eq!(v["pong"], true);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn auto_restart_after_crash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash_once");
        let marker = dir.path().join("started_once");

        // First invocation: exit 1 immediately. Second invocation: fake_helper_script.
        let body = format!(
            "#!/bin/sh\nif [ ! -f {m} ]; then touch {m}; exit 1; fi\n{rest}",
            m = marker.display(),
            rest = fake_helper_script().trim_start_matches("#!/bin/sh\n"),
        );
        tokio::fs::write(&path, body).await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .await
            .unwrap();

        let bridge = SwiftBridge::new(path);
        // First call races a premature exit: either the call fails fast or
        // ensure_running surfaces a spawn error. Either way the marker is
        // written so the second spawn succeeds.
        let _ = bridge
            .call::<_, serde_json::Value>("bridge.ping", serde_json::json!({}))
            .await;
        // Wait past the first backoff rung (1s): the reader observes stdout EOF,
        // resets state, and arms the spawn gate. A respawn before the window
        // elapses is now (correctly) refused with BridgeBackoff, so the sleep
        // must clear the 1s cooldown (plus reader-task scheduling jitter) before
        // the second call.
        tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;

        let v: serde_json::Value = bridge
            .call("bridge.ping", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(v["pong"], true);
    }

    #[tokio::test]
    async fn call_with_timeout_trips_when_helper_never_replies() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, silent_helper_script());

        let bridge = SwiftBridge::new(path);
        let generation = bridge.ensure_generation().unwrap();

        let result: Result<serde_json::Value> = bridge
            .call_with_timeout(
                "bridge.ping",
                serde_json::json!({}),
                Duration::from_millis(200),
            )
            .await;

        let err = result.unwrap_err();
        assert!(
            matches!(err, DesktopError::BridgeTimeout(_)),
            "expected BridgeTimeout, got: {err:?}"
        );
        // The timed-out request must not leak in the inflight table.
        assert!(generation.replies.slots.lock().unwrap().is_empty());
    }

    /// The protocol's per-method budget is what `call` actually spends.
    ///
    /// Guards the wiring itself: these numbers sat in the protocol with no
    /// consumer for long enough that a one-minute catch-all was being applied to
    /// a three-second call on the keystroke hot path.
    #[test]
    fn call_spends_the_deadline_the_protocol_declares() {
        use aleph_protocol::desktop_bridge::methods::{ax, pim, screen};

        assert_eq!(
            rpc_timeout_for(ax::METHOD_QUERY_FOCUSED),
            Duration::from_millis(ax::TIMEOUT_MS_QUERY_FOCUSED)
        );
        assert_eq!(
            rpc_timeout_for(screen::METHOD_OCR),
            Duration::from_millis(screen::TIMEOUT_MS_OCR)
        );
        // No override: the namespace default applies.
        assert_eq!(
            rpc_timeout_for(pim::METHOD_MAIL_SEARCH),
            Duration::from_millis(pim::DEFAULT_TIMEOUT_MS)
        );
        // Outside every namespace the client's own fallback is what is left.
        assert_eq!(rpc_timeout_for("window.move"), DEFAULT_RPC_TIMEOUT);
    }

    #[tokio::test]
    async fn default_call_succeeds_against_fast_helper() {
        // call() delegates to call_with_timeout(rpc_timeout_for(method));
        // a fast helper must still resolve well within that window.
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, fake_helper_script());

        let bridge = SwiftBridge::new(path);
        let v: serde_json::Value = bridge
            .call("bridge.ping", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(v["pong"], true);
    }

    #[tokio::test]
    async fn spawn_failures_back_off_instead_of_hammering() {
        // Non-existent binary guarantees every spawn fails instantly. The
        // backoff gate must space respawns out rather than burning through the
        // restart window in a tight loop (the old behaviour: 6 rapid calls
        // tripped the disable latch in microseconds). The disable threshold
        // itself is covered by the SpawnGate unit tests, which don't need to
        // wait out real backoff windows.
        let bridge = SwiftBridge::new(std::path::PathBuf::from(
            "/tmp/aleph-nonexistent-helper-for-test-0f3d",
        ));

        // First attempt actually tries to spawn → surfaces the spawn error and
        // arms the 1s backoff.
        let first = bridge
            .call::<_, serde_json::Value>("bridge.ping", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(first, DesktopError::BridgeFailed(_)),
            "first call should report the spawn failure, got: {first:?}"
        );

        // An immediate retry must be refused by the gate (BridgeBackoff), NOT
        // attempt another spawn and NOT trip the permanent disable latch.
        let second = bridge
            .call::<_, serde_json::Value>("bridge.ping", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(second, DesktopError::BridgeBackoff(_)),
            "rapid retry should back off, got: {second:?}"
        );
    }

    #[tokio::test]
    async fn is_running_reflects_spawn_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, fake_helper_script());

        let bridge = SwiftBridge::new(path);
        assert!(!bridge.is_running(), "fresh bridge must not be running");
        bridge.ensure_running().await.unwrap();
        assert!(
            bridge.is_running(),
            "bridge must report running after ensure_running"
        );
    }
    const FIXTURE_GUARD: Duration = Duration::from_secs(8);

    fn running_pid(bridge: &SwiftBridge, generation: &BridgeGeneration) -> i32 {
        let state = bridge.state.lock().unwrap();
        let process = state.process.as_ref().unwrap();
        assert!(std::ptr::eq(process.generation.as_ref(), generation));
        process.child.id().unwrap().try_into().unwrap()
    }

    fn assert_process_exists(pid: i32) {
        // SAFETY: signal 0 does not send a signal; pid is the spawned child.
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    }

    fn assert_process_gone(pid: i32) {
        // SAFETY: signal 0 does not send a signal; pid is the spawned child.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "PID {pid} still exists");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    async fn wait_reaped_and_gone(generation: &BridgeGeneration, pid: i32) {
        tokio::time::timeout(FIXTURE_GUARD, generation.wait_reaped())
            .await
            .expect("no reaping proof");
        assert_process_gone(pid);
    }

    async fn wait_for_fixture_line(path: &std::path::Path) -> String {
        tokio::time::timeout(FIXTURE_GUARD, async {
            loop {
                match std::fs::read_to_string(path) {
                    Ok(text) if text.ends_with('\n') => return text,
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => panic!("read fixture marker {}: {err}", path.display()),
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no fixture marker: {}", path.display()))
    }

    fn file_holding_helper(dir: &tempfile::TempDir, close_stdout: bool) -> PathBuf {
        std::fs::write(dir.path().join("input.wav"), b"fixture").unwrap();
        let eof = if close_stdout { "exec 1>&-" } else { ":" };
        install_fake(
            dir,
            &format!(
                r#"#!/bin/sh
set -eu
d=${{0%/*}}
if [ -f "$d/old-requests" ]; then
  while IFS= read -r line; do
    printf '%s\n' "$line" >> "$d/replacement-requests"
    id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
    printf '{{"jsonrpc":"2.0","id":%s,"result":"replacement"}}\n' "$id"
  done
  exit 0
fi
IFS= read -r request || exit 1
exec 3< "$d/input.wav"
printf '%s\n' "$request" > "$d/old-requests"
{eof}
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$d/old-requests"
done
"#
            ),
        )
    }

    fn resource_binding_helper(dir: &tempfile::TempDir) -> PathBuf {
        install_fake(
            dir,
            r#"#!/bin/sh
set -eu
d=${0%/*}
log="$d/resource-requests"
[ ! -f "$log" ] || log="$d/replacement-requests"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  case "$line" in
    *fixture.pending*) continue ;;
  esac
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
done
"#,
        )
    }

    fn fixture_ack(reply: BridgeRemoteReply) -> Result<()> {
        assert_eq!(reply.unwrap(), serde_json::json!({}));
        Ok(())
    }

    fn fixture_activation(reply: BridgeRemoteReply) -> BoundStartReply {
        use aleph_protocol::desktop_bridge::methods::media::voice_capture_start_rejected_for_legacy_busy;
        match reply {
            Ok(value) if value == serde_json::json!({}) => BoundStartReply::Active,
            Err(error) if voice_capture_start_rejected_for_legacy_busy(&error) => {
                BoundStartReply::NotAcquired(DesktopError::BridgeFailed("legacy busy".into()))
            }
            _ => BoundStartReply::Unproven(DesktopError::BridgeFailed("invalid reply".into())),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bound_generation_exact_start_refusal_preserves_ordinary_sibling() {
        for (data, exact) in [
            (r#"{"reason":"voice_capture_legacy_busy"}"#, true),
            (r#"{"reason":"unknown"}"#, false),
            (
                r#"{"reason":"voice_capture_legacy_busy","extra":true}"#,
                false,
            ),
            (
                r#"{"reason":"unknown","reason":"voice_capture_legacy_busy"}"#,
                false,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let script = format!(
                r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *fixture.start*) printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":-32004,"message":"busy","data":{data}}}}}\n' "$id" ;;
    *) printf '{{"jsonrpc":"2.0","id":%s,"result":%s}}\n' "$id" "$$" ;;
  esac
done
"#
            );
            let bridge = Arc::new(SwiftBridge::new(install_fake(&dir, &script)));
            let old = bridge.ensure_generation().unwrap();
            let pid = running_pid(&bridge, &old);
            let cancel = CancellationToken::new();
            let start = tokio::time::timeout(
                FIXTURE_GUARD,
                bridge.call_bound_start("fixture.start", (), &cancel, fixture_activation),
            )
            .await
            .unwrap();
            if exact {
                assert!(
                    matches!(start, Err(DesktopError::BridgeFailed(detail)) if detail == "legacy busy")
                );
                let ping: i32 = bridge
                    .call_with_timeout("fixture.ping", (), FIXTURE_GUARD)
                    .await
                    .unwrap();
                assert_eq!(ping, pid);
                assert_eq!(running_pid(&bridge, &old), pid);
                assert_eq!(
                    bridge
                        .state
                        .lock()
                        .unwrap()
                        .process
                        .as_ref()
                        .unwrap()
                        .mic_claims,
                    0
                );
            } else {
                assert!(start.is_err());
                assert!(old.retired.is_cancelled());
                assert_process_gone(pid);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bound_generation_keeps_same_child_after_accepting_poll_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(SwiftBridge::new(resource_binding_helper(&dir)));
        let cancel = CancellationToken::new();
        let mut binding = tokio::time::timeout(
            FIXTURE_GUARD,
            bridge.call_bound_start("fixture.start", (), &cancel, fixture_activation),
        )
        .await
        .unwrap()
        .unwrap();
        let old = Arc::clone(&binding.generation);
        let pid = running_pid(&bridge, &old);
        tokio::time::timeout(FIXTURE_GUARD, async {
            binding
                .call("fixture.poll", (), &cancel, fixture_ack)
                .await
                .unwrap();
            binding
                .finish("fixture.finish", (), &cancel, fixture_ack)
                .await
                .unwrap();
        })
        .await
        .unwrap();
        assert_eq!(running_pid(&bridge, &old), pid);
        let requests = std::fs::read_to_string(dir.path().join("resource-requests")).unwrap();
        let methods: Vec<_> = requests
            .lines()
            .map(|line| decode_line::<Request>(line).unwrap().method)
            .collect();
        assert_eq!(methods, ["fixture.start", "fixture.poll", "fixture.finish"]);
        assert!(!dir.path().join("replacement-requests").exists());

        let (release_reaper, pause_reaper) = oneshot::channel();
        *old.pause_reaper.lock().unwrap() = Some(pause_reaper);
        bridge.retire(&old, Retirement::BoundAbandoned);
        // A proven release leaves no capture barrier on a later retirement.
        bridge
            .call_with_timeout::<_, serde_json::Value>(METHOD_AUDIO_RECORD_START, (), FIXTURE_GUARD)
            .await
            .unwrap();
        assert_process_exists(pid);
        release_reaper.send(()).unwrap();
        wait_reaped_and_gone(&old, pid).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bound_generation_cancelled_poll_and_late_finish_never_touch_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(SwiftBridge::new(resource_binding_helper(&dir)));
        let cancel = CancellationToken::new();
        let mut binding = tokio::time::timeout(
            FIXTURE_GUARD,
            bridge.call_bound_start("fixture.start", (), &cancel, fixture_activation),
        )
        .await
        .unwrap()
        .unwrap();
        let old = Arc::clone(&binding.generation);
        let old_pid = running_pid(&bridge, &old);
        let old_log = dir.path().join("resource-requests");
        let mut poll = Box::pin(binding.call("fixture.pending_poll", (), &cancel, fixture_ack));
        tokio::select! {
            result = &mut poll => panic!("poll returned before cancellation: {result:?}"),
            marker = tokio::time::timeout(FIXTURE_GUARD, async {
                while !std::fs::read_to_string(&old_log).unwrap().contains("fixture.pending_poll") {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }) => marker.unwrap(),
        }
        cancel.cancel();
        tokio::select! {
            biased;
            result = &mut poll => panic!("poll returned before reaping: {result:?}"),
            () = std::future::ready(()) => {},
        }
        assert!(old.retired.is_cancelled());
        assert_process_exists(old_pid);
        let replacement = bridge.ensure_generation().unwrap();
        let replacement_pid = running_pid(&bridge, &replacement);
        assert!(!Arc::ptr_eq(&old, &replacement));
        let result = tokio::time::timeout(FIXTURE_GUARD, &mut poll)
            .await
            .unwrap();
        assert!(
            matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail.contains("cancelled"))
        );
        drop(poll);
        assert_process_gone(old_pid);
        let fresh = CancellationToken::new();
        let result = tokio::time::timeout(
            FIXTURE_GUARD,
            binding.finish("fixture.finish", (), &fresh, fixture_ack),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        let _: serde_json::Value = bridge
            .call_with_timeout("fixture.ping", (), FIXTURE_GUARD)
            .await
            .unwrap();
        assert_eq!(running_pid(&bridge, &replacement), replacement_pid);
        let old_requests = std::fs::read_to_string(old_log).unwrap();
        assert_eq!(old_requests.lines().count(), 2);
        let replacement_requests =
            std::fs::read_to_string(dir.path().join("replacement-requests")).unwrap();
        assert_eq!(replacement_requests.lines().count(), 1);
        assert!(replacement_requests.contains("fixture.ping"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bound_generation_dropped_start_after_dispatch_retires_without_cancel_rpc() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(SwiftBridge::new(resource_binding_helper(&dir)));
        let old = bridge.ensure_generation().unwrap();
        let pid = running_pid(&bridge, &old);
        let cancel = CancellationToken::new();
        let mut start = Box::pin(bridge.call_bound_start(
            "fixture.pending_start",
            (),
            &cancel,
            fixture_activation,
        ));
        let old_log = dir.path().join("resource-requests");
        let request = tokio::select! {
            _ = &mut start => panic!("start returned without activation"),
            request = wait_for_fixture_line(&old_log) => request,
        };
        assert!(request.contains("fixture.pending_start"));
        drop(start);
        assert!(old.retired.is_cancelled());
        assert!(!*old.reaped.borrow(), "drop must not claim release proof");
        wait_reaped_and_gone(&old, pid).await;
        assert_eq!(std::fs::read_to_string(old_log).unwrap(), request);
        assert!(!dir.path().join("replacement-requests").exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bound_generation_drop_blocks_replacement_mic_until_exact_capture_child_is_reaped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("input.wav"), b"fixture").unwrap();
        let path = install_fake(
            &dir,
            r#"#!/bin/sh
set -eu
d=${0%/*}
reply() {
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
}
if [ ! -f "$d/old-pid" ]; then
  IFS= read -r line
  exec 3< "$d/input.wav"
  printf '%s\n' "$$" > "$d/old-pid"
  printf '%s\n' "$line" > "$d/old-requests"
  reply
  while IFS= read -r line; do printf '%s\n' "$line" >> "$d/old-requests"; done
else
  read -r old_pid < "$d/old-pid"
  while IFS= read -r line; do
    printf '%s\n' "$line" >> "$d/replacement-requests"
    case "$line" in
      *media.audio.record*|*'"with_audio":true'*)
        if kill -0 "$old_pid" 2>/dev/null; then exit 1; fi ;;
    esac
    reply
  done
fi
"#,
        );
        let bridge = Arc::new(SwiftBridge::new(path));
        let old = bridge.ensure_generation().unwrap();
        let pid = running_pid(&bridge, &old);
        let (release_reaper, pause_reaper) = oneshot::channel();
        *old.pause_reaper.lock().unwrap() = Some(pause_reaper);
        let cancel = CancellationToken::new();
        let capture = tokio::time::timeout(
            FIXTURE_GUARD,
            bridge.call_bound_start("fixture.start", (), &cancel, fixture_activation),
        )
        .await
        .unwrap()
        .unwrap();
        // A Drop is only retirement, not release. Retain the old generation
        // Arc so its child remains blocked in read with FD 3 still open.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("old-pid"))
                .unwrap()
                .trim(),
            pid.to_string()
        );
        drop(capture);
        assert!(old.retired.is_cancelled());
        assert_process_exists(pid);

        let replacement = bridge.ensure_generation().unwrap();
        assert!(!Arc::ptr_eq(&old, &replacement));
        let replacement_pid = running_pid(&bridge, &replacement);
        assert_ne!(pid, replacement_pid);
        let replacement_requests = dir.path().join("replacement-requests");
        let mut waiting = [
            (METHOD_AUDIO_RECORD, serde_json::json!({})),
            (METHOD_AUDIO_RECORD_START, serde_json::json!({})),
            (METHOD_CAMERA_CLIP, serde_json::json!({"with_audio": true})),
        ]
        .map(|(method, params)| {
            Box::pin(bridge.call_with_timeout::<_, serde_json::Value>(
                method,
                params,
                FIXTURE_GUARD,
            ))
        });
        let mut native =
            Box::pin(bridge.call_bound_start("fixture.start", (), &cancel, fixture_activation));
        for waiter in &mut waiting {
            tokio::select! {
                biased;
                result = waiter => panic!("legacy acquisition completed before capture reap: {result:?}"),
                () = std::future::ready(()) => {},
            }
        }
        tokio::select! {
            biased;
            _ = &mut native => panic!("native acquisition completed before capture reap"),
            () = std::future::ready(()) => {},
        }
        // Same serialized write lane: an ordinary video-only clip must still
        // reach the replacement and its log must expose any premature writes.
        bridge
            .call_with_timeout::<_, serde_json::Value>(
                METHOD_CAMERA_CLIP,
                serde_json::json!({"with_audio": false}),
                FIXTURE_GUARD,
            )
            .await
            .unwrap();
        let requests = std::fs::read_to_string(&replacement_requests).unwrap();
        assert_eq!(requests.lines().count(), 1);
        assert!(requests.contains(r#""with_audio":false"#));
        assert_process_exists(pid);
        assert!(!*old.reaped.borrow());
        cancel.cancel();
        assert!(
            matches!(native.await, Err(DesktopError::BridgeFailed(detail)) if detail.contains("cancelled"))
        );
        assert_eq!(running_pid(&bridge, &replacement), replacement_pid);

        release_reaper.send(()).unwrap();
        for waiter in waiting {
            waiter.await.unwrap();
        }
        assert_process_gone(pid);
        assert!(*old.reaped.borrow());
        assert_eq!(running_pid(&bridge, &replacement), replacement_pid);
        let old_requests = std::fs::read_to_string(dir.path().join("old-requests")).unwrap();
        assert_eq!(old_requests.lines().count(), 1);
        let requests = std::fs::read_to_string(replacement_requests).unwrap();
        assert_eq!(requests.lines().count(), 4);
        assert!(!requests.contains("fixture.start"));
        let state = bridge.state.lock().unwrap();
        let process = state.process.as_ref().unwrap();
        assert_eq!(
            process.mic_claims, 0,
            "timed audio and camera proved release"
        );
        assert!(process.open_recording_dispatched);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bound_generation_native_start_waits_for_legacy_record_start_child_to_be_reaped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("input.wav"), b"fixture").unwrap();
        let path = install_fake(
            &dir,
            r#"#!/bin/sh
set -eu
d=${0%/*}
reply() {
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
}
if [ ! -f "$d/old-pid" ]; then
  IFS= read -r line
  case "$line" in *media.audio.record_start*) ;; *) exit 1 ;; esac
  exec 3< "$d/input.wav"
  printf '%s\n' "$$" > "$d/old-pid"
  printf '%s\n' "$line" > "$d/old-requests"
  reply
  while IFS= read -r line; do printf '%s\n' "$line" >> "$d/old-requests"; done
else
  read -r old_pid < "$d/old-pid"
  while IFS= read -r line; do
    printf '%s\n' "$line" >> "$d/replacement-requests"
    case "$line" in
      *media.audio.voice_capture_start*)
        if kill -0 "$old_pid" 2>/dev/null; then exit 1; fi ;;
    esac
    reply
  done
fi
"#,
        );
        let bridge = Arc::new(SwiftBridge::new(path));
        let old = bridge.ensure_generation().unwrap();
        let pid = running_pid(&bridge, &old);
        let (release_reaper, pause_reaper) = oneshot::channel();
        *old.pause_reaper.lock().unwrap() = Some(pause_reaper);
        bridge
            .call_with_timeout::<_, serde_json::Value>(METHOD_AUDIO_RECORD_START, (), FIXTURE_GUARD)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("old-pid"))
                .unwrap()
                .trim(),
            pid.to_string()
        );
        bridge.retire(&old, Retirement::BoundAbandoned);
        assert!(old.retired.is_cancelled());
        assert_process_exists(pid);

        let replacement = bridge.ensure_generation().unwrap();
        let replacement_pid = running_pid(&bridge, &replacement);
        assert!(!Arc::ptr_eq(&old, &replacement));
        assert_ne!(pid, replacement_pid);
        let cancel = CancellationToken::new();
        let mut start = Box::pin(bridge.call_bound_start(
            METHOD_VOICE_CAPTURE_START,
            (),
            &cancel,
            fixture_activation,
        ));
        tokio::select! {
            biased;
            _ = &mut start => panic!("native start completed before legacy reap"),
            () = std::future::ready(()) => {},
        }
        bridge
            .call_with_timeout::<_, serde_json::Value>("fixture.ping", (), FIXTURE_GUARD)
            .await
            .unwrap();
        let requests = dir.path().join("replacement-requests");
        let before = std::fs::read_to_string(&requests).unwrap();
        assert_eq!(before.lines().count(), 1);
        assert!(before.contains("fixture.ping"));
        assert_process_exists(pid);
        assert!(!*old.reaped.borrow());

        release_reaper.send(()).unwrap();
        let binding = tokio::time::timeout(FIXTURE_GUARD, start)
            .await
            .unwrap()
            .unwrap();
        assert_process_gone(pid);
        assert!(*old.reaped.borrow());
        assert!(Arc::ptr_eq(&binding.generation, &replacement));
        assert_eq!(running_pid(&bridge, &replacement), replacement_pid);
        let after = std::fs::read_to_string(&requests).unwrap();
        assert_eq!(after.lines().count(), 2);
        assert!(after.starts_with(&before));
        assert!(after
            .lines()
            .last()
            .unwrap()
            .contains(METHOD_VOICE_CAPTURE_START));
        let old_requests = std::fs::read_to_string(dir.path().join("old-requests")).unwrap();
        assert_eq!(old_requests.lines().count(), 1);
        assert!(old_requests.contains(METHOD_AUDIO_RECORD_START));
        binding
            .finish("fixture.finish", (), &cancel, fixture_ack)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bound_generation_unterminated_stdout_limit_retires_and_reaps_capture_child() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(
            &dir,
            r#"#!/bin/sh
set -eu
log="${0%/*}/requests"
IFS= read -r line
printf '%s\n' "$line" >> "$log"
id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
IFS= read -r line
printf '%s\n' "$line" >> "$log"
# Write exactly the limit, with neither a delimiter nor EOF.
printf '%128s' ''
while IFS= read -r line; do printf '%s\n' "$line" >> "$log"; done
"#,
        );
        let mut bridge = SwiftBridge::new(path);
        bridge.stdout_frame_limit = 128;
        let bridge = Arc::new(bridge);
        let old = bridge.ensure_generation().unwrap();
        let pid = running_pid(&bridge, &old);
        let (release_reaper, pause_reaper) = oneshot::channel();
        *old.pause_reaper.lock().unwrap() = Some(pause_reaper);
        let cancel = CancellationToken::new();
        let mut capture = tokio::time::timeout(
            FIXTURE_GUARD,
            bridge.call_bound_start("fixture.start", (), &cancel, fixture_activation),
        )
        .await
        .unwrap()
        .unwrap();
        let mut poll = Box::pin(capture.call("fixture.poll", (), &cancel, fixture_ack));
        tokio::time::timeout(FIXTURE_GUARD, async {
            tokio::select! {
                biased;
                result = &mut poll => panic!("capture returned before reaping: {result:?}"),
                () = old.retired.cancelled() => {},
            }
        })
        .await
        .unwrap();
        assert_process_exists(pid);
        assert!(!*old.reaped.borrow());
        assert!(!bridge.is_running());
        release_reaper.send(()).unwrap();
        let result = tokio::time::timeout(FIXTURE_GUARD, poll).await.unwrap();
        assert!(
            matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail == STDOUT_FRAME_TOO_LARGE)
        );
        assert_process_gone(pid);
        assert!(*old.reaped.borrow());
        let requests = std::fs::read_to_string(dir.path().join("requests")).unwrap();
        assert_eq!(requests.lines().count(), 2);
        assert!(requests.contains("fixture.poll"));
        assert!(!bridge.is_running());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn call_bound_cancel_waits_for_exact_reaping_without_replay_on_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = SwiftBridge::new(file_holding_helper(&dir, false));
        let old = bridge.ensure_generation().unwrap();
        let old_pid = running_pid(&bridge, &old);
        let cancel = CancellationToken::new();
        let mut bound =
            Box::pin(
                bridge.call_bound("fixture.hold_file", (), FIXTURE_GUARD, &cancel, |_| {
                    panic!("cancelled request must not obtain remote release proof")
                }),
            );
        let old_log = dir.path().join("old-requests");
        let request = tokio::select! {
            result = &mut bound => panic!("returned before cancellation: {result:?}"),
            request = wait_for_fixture_line(&old_log) => request,
        };
        assert!(request.contains("fixture.hold_file"));
        let mut first_waiter = Box::pin(wait_reaped_and_gone(&old, old_pid));
        let mut second_waiter = Box::pin(wait_reaped_and_gone(&old, old_pid));
        for waiter in [&mut first_waiter, &mut second_waiter] {
            tokio::select! {
                biased;
                () = waiter => panic!("proof completed before retirement"),
                () = std::future::ready(()) => {},
            }
        }
        cancel.cancel();
        // A single poll triggers retirement without yielding to the reaper on
        // this current-thread runtime. The live child cannot be release proof.
        tokio::select! {
            biased;
            result = &mut bound => panic!("returned before reaping: {result:?}"),
            () = std::future::ready(()) => {},
        }
        assert!(old.retired.is_cancelled());
        assert_process_exists(old_pid);
        let replacement = bridge.ensure_generation().unwrap();
        assert!(!Arc::ptr_eq(&old, &replacement));
        let replacement_pid = running_pid(&bridge, &replacement);
        assert_ne!(old_pid, replacement_pid);
        bridge.retire(&old, Retirement::StdoutClosed);
        let ping = bridge.call_with_timeout::<_, String>("fixture.ping", (), FIXTURE_GUARD);
        let (result, (), (), ping) = tokio::time::timeout(FIXTURE_GUARD, async {
            tokio::join!(bound, first_waiter, second_waiter, ping)
        })
        .await
        .unwrap();
        assert!(
            matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail.contains("cancelled"))
        );
        wait_reaped_and_gone(&old, old_pid).await;
        assert_eq!(ping.unwrap(), "replacement");
        assert_eq!(running_pid(&bridge, &replacement), replacement_pid);
        assert_process_exists(replacement_pid);
        assert_eq!(std::fs::read_to_string(old_log).unwrap(), request);
        let replacement_requests =
            std::fs::read_to_string(dir.path().join("replacement-requests")).unwrap();
        assert_eq!(replacement_requests.lines().count(), 1);
        assert!(replacement_requests.contains("fixture.ping"));
        std::fs::remove_file(dir.path().join("input.wav")).unwrap();
    }

    #[tokio::test]
    async fn call_bound_stdout_eof_returns_only_after_exact_child_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = SwiftBridge::new(file_holding_helper(&dir, true));
        let generation = bridge.ensure_generation().unwrap();
        let pid = running_pid(&bridge, &generation);
        let cancel = CancellationToken::new();
        let result = tokio::time::timeout(
            FIXTURE_GUARD,
            bridge.call_bound("fixture.eof", (), FIXTURE_GUARD, &cancel, |_| {
                panic!("EOF must not invoke the reply predicate")
            }),
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail == "helper stdout closed")
        );
        assert_process_gone(pid);
        assert!(*generation.reaped.borrow());
        let requests = std::fs::read_to_string(dir.path().join("old-requests")).unwrap();
        assert_eq!(requests.lines().count(), 1);
        assert!(requests.contains("fixture.eof"));
        assert!(!dir.path().join("replacement-requests").exists());
    }

    #[tokio::test]
    async fn call_bound_positional_rpc_error_never_reaches_release_predicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(
            &dir,
            r#"#!/bin/sh
set -eu
IFS= read -r line
id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
printf '{"jsonrpc":"2.0","id":%s,"error":[-32602,"positional",null]}\n' "$id"
# Keep stdout open; only the malformed RPC should retire this generation.
while IFS= read -r line; do :; done
"#,
        );
        let bridge = SwiftBridge::new(path);
        let generation = bridge.ensure_generation().unwrap();
        let pid = running_pid(&bridge, &generation);
        let cancel = CancellationToken::new();
        let result = tokio::time::timeout(
            FIXTURE_GUARD,
            bridge.call_bound(
                "fixture.malformed_reply",
                (),
                FIXTURE_GUARD,
                &cancel,
                |_| panic!("a malformed bound response must not reach the release predicate"),
            ),
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail == "malformed bound response envelope")
        );
        assert!(*generation.reaped.borrow());
        assert_process_gone(pid);
    }

    #[tokio::test]
    async fn call_bound_accepted_remote_release_keeps_helper_running() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = SwiftBridge::new(install_fake(&dir, fake_helper_script()));
        let generation = bridge.ensure_generation().unwrap();
        let response = bridge
            .call_bound(
                "fixture.released",
                (),
                FIXTURE_GUARD,
                &CancellationToken::new(),
                |reply| reply.as_ref().is_ok_and(|value| value["pong"] == true),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response["pong"], true);
        assert!(Arc::ptr_eq(
            &generation,
            &bridge.ensure_generation().unwrap()
        ));
        assert!(!generation.retired.is_cancelled());
    }

    fn fixture_fifo(path: &std::path::Path) {
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: path is NUL terminated and alive for the syscall.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    }

    async fn open_fixture_trigger(path: &std::path::Path) -> std::fs::File {
        use std::os::unix::fs::OpenOptionsExt;

        tokio::time::timeout(FIXTURE_GUARD, async {
            loop {
                match std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(path)
                {
                    Ok(fifo) => return fifo,
                    Err(err) if err.raw_os_error() == Some(libc::ENXIO) => {}
                    Err(err) => panic!("open fixture FIFO: {err}"),
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("helper never reached FIFO rendezvous")
    }

    async fn let_retry_helper_close_stdin(path: &std::path::Path) {
        use std::io::Write;

        open_fixture_trigger(path)
            .await
            .write_all(b"close\n")
            .unwrap();
    }

    async fn send_fixture_request<'a>(
        bridge: &SwiftBridge,
        generation: &'a BridgeGeneration,
        method: &str,
    ) -> PendingReply<'a> {
        let request = bridge.request(method, ()).unwrap();
        let line = encode(&request).unwrap();
        tokio::time::timeout(
            FIXTURE_GUARD,
            bridge.send(generation, request.id, &line, WriteAttempt::Ordinary),
        )
        .await
        .unwrap()
        .unwrap()
    }

    const BLOCKED_STDIN_BYTES: usize = 2 * 1024 * 1024;

    async fn unread_stdin_helper(
        dir: &tempfile::TempDir,
    ) -> (SwiftBridge, Arc<BridgeGeneration>, i32) {
        fixture_fifo(&dir.path().join("park"));
        let path = install_fake(
            dir,
            r#"#!/bin/sh
set -eu
d=${0%/*}
if [ ! -f "$d/parked-pid" ]; then
  printf '%s\n' "$$" > "$d/parked-pid"
  # Block this shell opening a different FD, leaving stdin unread and stdout open.
  exec 3< "$d/park"
  exit 91
fi
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$d/replacement-requests"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"result":"replacement"}\n' "$id"
done
"#,
        );
        let bridge = SwiftBridge::new(path);
        let generation = bridge.ensure_generation().unwrap();
        let pid = running_pid(&bridge, &generation);
        let parked_pid = wait_for_fixture_line(&dir.path().join("parked-pid")).await;
        assert_eq!(parked_pid.trim().parse::<i32>().unwrap(), pid);
        (bridge, generation, pid)
    }

    fn assert_detached_without_crash_charge(bridge: &SwiftBridge, generation: &BridgeGeneration) {
        assert!(generation.retired.is_cancelled());
        let state = bridge.state.lock().unwrap();
        assert!(state.process.is_none());
        assert!(!state.disabled);
        assert!(matches!(state.gate.poll(), SpawnDecision::Go));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn call_bound_blocked_stdin_cancel_and_drop_do_not_charge_or_replay() {
        for cancel_token in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let (bridge, old, old_pid) = unread_stdin_helper(&dir).await;
            let cancel = CancellationToken::new();
            let mut call = Box::pin(bridge.call_bound(
                "fixture.blocked_stdin",
                "x".repeat(BLOCKED_STDIN_BYTES),
                FIXTURE_GUARD,
                &cancel,
                |_| panic!("unread stdin cannot produce a release reply"),
            ));
            // The payload cannot fit in the unread pipe. Poll until the writer owns
            // stdin and is pending, rather than cancelling an undispatched call.
            tokio::select! {
                biased;
                result = &mut call => panic!("returned before cancellation: {result:?}"),
                () = std::future::ready(()) => {},
            }
            assert!(
                old.stdin.try_lock().is_err(),
                "bound writer did not own stdin"
            );
            assert_eq!(old.replies.slots.lock().unwrap().len(), 1);
            assert!(!old.retired.is_cancelled());
            assert!(Arc::ptr_eq(&old, &bridge.ensure_generation().unwrap()));
            assert_process_exists(old_pid);

            let cancelled_call = if cancel_token {
                cancel.cancel();
                // No reaper task can run during this poll on the current thread.
                tokio::select! {
                    biased;
                    result = &mut call => panic!("returned before exact reaping: {result:?}"),
                    () = std::future::ready(()) => {},
                }
                Some(call)
            } else {
                drop(call);
                None
            };
            assert_detached_without_crash_charge(&bridge, &old);
            assert!(!*old.reaped.borrow());
            assert_process_exists(old_pid);
            if let Some(call) = cancelled_call {
                let result = tokio::time::timeout(FIXTURE_GUARD, call).await.unwrap();
                assert!(
                    matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail.contains("cancelled"))
                );
                assert!(
                    *old.reaped.borrow(),
                    "call returned without exact wait proof"
                );
                assert_process_gone(old_pid);
            } else {
                wait_reaped_and_gone(&old, old_pid).await;
            }
            assert_blocked_stdin_not_replayed(&bridge, &dir).await;
        }
    }

    async fn assert_blocked_stdin_not_replayed(bridge: &SwiftBridge, dir: &tempfile::TempDir) {
        let log = dir.path().join("replacement-requests");
        assert!(!log.exists());
        let ping: String = tokio::time::timeout(
            FIXTURE_GUARD,
            bridge.call_with_timeout("fixture.ping", (), FIXTURE_GUARD),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(ping, "replacement");
        let requests = std::fs::read_to_string(log).unwrap();
        assert_eq!(requests.lines().count(), 1);
        assert!(requests.contains("fixture.ping"));
        assert!(!requests.contains("fixture.blocked_stdin"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocked_stdin_write_drop_classifies_before_closing_lane() {
        for bound in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let (bridge, generation, pid) = unread_stdin_helper(&dir).await;
            let line = "x".repeat(BLOCKED_STDIN_BYTES);
            let mut armed = false;
            let attempt = if bound {
                WriteAttempt::Bound(&mut armed)
            } else {
                WriteAttempt::OrdinaryRetry
            };
            // Drive only the real write, with no BoundDispatch or call_bound cleanup.
            let mut write = Box::pin(generation.write(&bridge, &line, attempt));
            tokio::select! {
                biased;
                result = &mut write => panic!("write was not blocked: {result:?}"),
                () = std::future::ready(()) => {},
            }
            assert!(generation.stdin.try_lock().is_err());
            assert!(!generation.retired.is_cancelled());
            drop(write);

            assert_eq!(armed, bound);
            assert!(generation.stdin.try_lock().unwrap().is_none());
            assert_detached_without_crash_charge(&bridge, &generation);
            assert_process_exists(pid);
            wait_reaped_and_gone(&generation, pid).await;
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ordinary_blocked_stdin_drop_causes_helper_eof_without_crash_charge() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let resume = dir.path().join("resume");
        fixture_fifo(&resume);
        let path = install_fake(
            &dir,
            r#"#!/bin/sh
set -eu
d=${0%/*}
IFS= read -r resume < "$d/resume"
while IFS= read -r line; do :; done
printf '%s\n' "$$" > "$d/observed-eof"
"#,
        );
        let bridge = SwiftBridge::new(path);
        // Any accidental crash charge remains visible after the exact wait.
        bridge.state.lock().unwrap().gate = SpawnGate::new(0, FIXTURE_GUARD);
        let generation = bridge.ensure_generation().unwrap();
        let pid = running_pid(&bridge, &generation);
        let mut resume = open_fixture_trigger(&resume).await;
        let mut call = Box::pin(bridge.call_with_timeout::<_, String>(
            "fixture.ordinary_blocked_stdin",
            "x".repeat(BLOCKED_STDIN_BYTES),
            FIXTURE_GUARD,
        ));
        tokio::select! {
            biased;
            result = &mut call => panic!("write was not blocked: {result:?}"),
            () = std::future::ready(()) => {},
        }
        assert!(generation.stdin.try_lock().is_err());
        assert!(!generation.retired.is_cancelled());
        drop(call);
        resume.write_all(b"read\n").unwrap();
        // Do not yield to the current-thread reaper: the actual helper must
        // observe EOF and exit its stdin loop independently of the kill request.
        let marker = dir.path().join("observed-eof");
        let deadline = std::time::Instant::now() + FIXTURE_GUARD;
        loop {
            if let Ok(observed) = std::fs::read_to_string(&marker) {
                if observed.ends_with('\n') {
                    assert_eq!(observed.trim().parse::<i32>().unwrap(), pid);
                    break;
                }
            }
            assert!(std::time::Instant::now() < deadline, "helper never saw EOF");
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_reaped_and_gone(&generation, pid).await;
        assert_detached_without_crash_charge(&bridge, &generation);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn call_bound_real_epipe_waits_for_exact_reaping_and_charges_once() {
        let dir = tempfile::tempdir().unwrap();
        fixture_fifo(&dir.path().join("park"));
        let path = install_fake(
            &dir,
            r#"#!/bin/sh
set -eu
d=${0%/*}
exec 0<&-
printf '%s\n' "$$" > "$d/closed-stdin"
# Keep stdout open so the real pipe write, not reader EOF, triggers retirement.
IFS= read -r park < "$d/park"
"#,
        );
        let bridge = SwiftBridge::new(path);
        // One failure backs off; a second charge within the window disables.
        bridge.state.lock().unwrap().gate = SpawnGate::new(1, FIXTURE_GUARD);
        let generation = bridge.ensure_generation().unwrap();
        let pid = running_pid(&bridge, &generation);
        let closed = wait_for_fixture_line(&dir.path().join("closed-stdin")).await;
        assert_eq!(closed.trim().parse::<i32>().unwrap(), pid);
        assert_process_exists(pid);
        assert!(!generation.retired.is_cancelled());
        let cancel = CancellationToken::new();
        let mut call =
            Box::pin(
                bridge.call_bound("fixture.bound_epipe", (), FIXTURE_GUARD, &cancel, |_| {
                    panic!("closed stdin cannot produce a release reply")
                }),
            );
        // Drive I/O until retirement. When it happens the same poll must wait
        // for reaping, so the reader and reaper cannot race the first gate check.
        tokio::time::timeout(
            FIXTURE_GUARD,
            std::future::poll_fn(|cx| {
                use std::future::Future;
                use std::task::Poll;

                if let Poll::Ready(result) = call.as_mut().poll(cx) {
                    panic!("returned before exact reaping: {result:?}");
                }
                if generation.retired.is_cancelled() {
                    assert!(!*generation.reaped.borrow());
                    assert!(matches!(
                        bridge.ensure_generation(),
                        Err(DesktopError::BridgeBackoff(_))
                    ));
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }),
        )
        .await
        .unwrap();
        let result = tokio::time::timeout(FIXTURE_GUARD, call).await.unwrap();
        let epipe = std::io::Error::from_raw_os_error(libc::EPIPE);
        assert!(matches!(result, Err(DesktopError::BridgeFailed(detail))
            if detail == format!("write stdin: {epipe}")));
        assert!(*generation.reaped.borrow());
        assert_process_gone(pid);
        assert!(!bridge.state.lock().unwrap().disabled);
    }

    #[tokio::test]
    async fn ordinary_double_write_failure_backs_off_and_keeps_delivered_sibling_replies() {
        let dir = tempfile::tempdir().unwrap();
        let close_retry = dir.path().join("close-retry");
        fixture_fifo(&close_retry);
        fixture_fifo(&dir.path().join("park"));
        let path = install_fake(
            &dir,
            r#"#!/bin/sh
set -eu
id() { printf '%s\n' "$1" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p'; }
d=${0%/*}
if [ ! -f "$d/first" ]; then
  printf '%s\n' "$$" > "$d/first"
  IFS= read -r successful || exit 1
  IFS= read -r remote_error || exit 2
  IFS= read -r unresolved || exit 3
  IFS= read -r sync || exit 4
  exec 0<&-
  printf '{"jsonrpc":"2.0","id":%s,"result":"sibling success"}\n' "$(id "$successful")"
  printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"sibling error"}}\n' "$(id "$remote_error")"
  printf '{"jsonrpc":"2.0","id":%s,"result":"stdin closed"}\n' "$(id "$sync")"
elif [ ! -f "$d/second" ]; then
  printf '%s\n' "$$" > "$d/second"
  IFS= read -r close < "$d/close-retry"
  exec 0<&-
else
  printf '%s\n' "$$" >> "$d/unexpected-spawn"
fi
# Remain alive with stdout open after closing stdin, without a descendant.
IFS= read -r park < "$d/park"
"#,
        );
        let bridge = SwiftBridge::new(path);
        let first = bridge.ensure_generation().unwrap();
        let first_pid = running_pid(&bridge, &first);
        let mut success = send_fixture_request(&bridge, &first, "fixture.success").await;
        let mut remote_error = send_fixture_request(&bridge, &first, "fixture.remote_error").await;
        let mut unresolved = send_fixture_request(&bridge, &first, "fixture.unresolved").await;
        let ack: String = bridge
            .call_with_timeout("fixture.closed_ack", (), FIXTURE_GUARD)
            .await
            .unwrap();
        assert_eq!(ack, "stdin closed");
        assert_process_exists(first_pid);
        assert!(!first.retired.is_cancelled());
        // The retry cannot complete by fitting its request into the new pipe
        // before the shell closes stdin. The new helper never reads that pipe.
        let mut call = Box::pin(bridge.call_with_timeout::<_, String>(
            "fixture.double_failure",
            "x".repeat(2 * 1024 * 1024),
            FIXTURE_GUARD,
        ));
        let second_log = dir.path().join("second");
        let second_pid: i32 = tokio::select! {
            result = &mut call => panic!("call finished before the retry's stdin closed: {result:?}"),
            pid = wait_for_fixture_line(&second_log) => pid.trim().parse().unwrap(),
        };
        assert!(first.retired.is_cancelled());
        let second = bridge.ensure_generation().unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(!second.retired.is_cancelled());
        assert_eq!(running_pid(&bridge, &second), second_pid);
        assert_process_exists(second_pid);
        // An EOF cleanup from the already detached generation cannot charge
        // the gate or retire the still-current retry generation.
        bridge.retire(&first, Retirement::StdoutClosed);
        assert!(matches!(
            bridge.state.lock().unwrap().gate.poll(),
            SpawnDecision::Go
        ));
        assert_eq!(running_pid(&bridge, &second), second_pid);
        let_retry_helper_close_stdin(&close_retry).await;
        let result = tokio::time::timeout(FIXTURE_GUARD, call).await.unwrap();
        assert!(
            matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail.starts_with("write stdin retry:"))
        );
        assert!(second.retired.is_cancelled());
        assert!(bridge.state.lock().unwrap().process.is_none());
        let later = tokio::time::timeout(
            FIXTURE_GUARD,
            bridge.call_with_timeout::<_, String>("fixture.no_third_spawn", (), FIXTURE_GUARD),
        )
        .await
        .unwrap();
        assert!(matches!(later, Err(DesktopError::BridgeBackoff(_))));
        assert!(!dir.path().join("unexpected-spawn").exists());
        // The ACK ordered these valid replies before retirement, from the
        // same helper after it closed stdin. Consuming them later still works.
        let (success, remote_error, unresolved) = tokio::time::timeout(FIXTURE_GUARD, async {
            tokio::join!(
                success.receive(),
                remote_error.receive(),
                unresolved.receive()
            )
        })
        .await
        .unwrap();
        assert_eq!(
            success.unwrap().unwrap(),
            serde_json::json!("sibling success")
        );
        assert_eq!(remote_error.unwrap().unwrap_err().code, -32602);
        assert!(
            matches!(unresolved, Err(DesktopError::BridgeFailed(detail)) if detail == "write stdin failed")
        );
        tokio::join!(
            wait_reaped_and_gone(&first, first_pid),
            wait_reaped_and_gone(&second, second_pid)
        );
    }
}

#[cfg(test)]
mod bound_reply_tests {
    use super::*;
    use serde_json::{json, Value};

    #[cfg(any(unix, windows))]
    #[tokio::test(flavor = "current_thread")]
    async fn call_bound_queued_on_stdin_can_cancel_timeout_or_drop_without_retirement() {
        enum End {
            Cancel,
            Timeout,
            Drop,
        }

        fn assert_send<T: Send>(_: &T) {}

        // A bare shell waits on its piped stdin. The fixture never sends it
        // data: the lock and all cancellation paths are driven in-process.
        #[cfg(unix)]
        let binary = PathBuf::from("/bin/sh");
        #[cfg(windows)]
        let binary =
            std::env::var_os("COMSPEC").map_or_else(|| PathBuf::from("cmd.exe"), PathBuf::from);
        let bridge = SwiftBridge::new(binary);
        let generation = bridge.ensure_generation().unwrap();
        let stdin = generation.stdin.lock().await;
        let watchdog = Duration::from_secs(8);
        for end in [End::Cancel, End::Timeout, End::Drop] {
            let cancel = CancellationToken::new();
            let timeout = if matches!(end, End::Timeout) {
                Duration::from_millis(250)
            } else {
                watchdog
            };
            let mut call =
                Box::pin(
                    bridge.call_bound("fixture.queued_stdin", (), timeout, &cancel, |_| {
                        panic!("a queued call cannot produce a release reply")
                    }),
                );
            assert_send(&call);
            tokio::select! {
                biased;
                result = &mut call => panic!("write did not queue: {result:?}"),
                () = std::future::ready(()) => {},
            }
            assert_eq!(generation.replies.slots.lock().unwrap().len(), 1);
            match end {
                End::Cancel => {
                    cancel.cancel();
                    let result = tokio::select! {
                        biased;
                        result = &mut call => result,
                        () = std::future::ready(()) => panic!("queued cancellation awaited reaping"),
                    };
                    assert!(matches!(result, Err(DesktopError::BridgeFailed(detail))
                        if detail.contains("cancelled")));
                }
                End::Timeout => {
                    let result = tokio::time::timeout(watchdog, &mut call).await.unwrap();
                    assert!(matches!(result, Err(DesktopError::BridgeTimeout(_))));
                }
                End::Drop => {}
            }
            drop(call);
            assert!(generation.replies.slots.lock().unwrap().is_empty());
            assert!(!generation.retired.is_cancelled());
            assert!(!*generation.reaped.borrow());
            assert!(Arc::ptr_eq(
                &generation,
                &bridge.ensure_generation().unwrap()
            ));
            let mut state = bridge.state.lock().unwrap();
            let child = &mut state.process.as_mut().unwrap().child;
            assert!(child.try_wait().unwrap().is_none());
        }
        assert!(stdin.is_some());
        drop(stdin);
        drop(bridge);
        tokio::time::timeout(watchdog, generation.wait_reaped())
            .await
            .unwrap();
    }

    #[cfg(any(unix, windows))]
    #[tokio::test(flavor = "current_thread")]
    async fn bound_generation_queued_start_cancel_and_drop_leave_unwritten_helper_running() {
        #[cfg(unix)]
        let binary = PathBuf::from("/bin/sh");
        #[cfg(windows)]
        let binary =
            std::env::var_os("COMSPEC").map_or_else(|| PathBuf::from("cmd.exe"), PathBuf::from);
        let bridge = Arc::new(SwiftBridge::new(binary));
        let generation = bridge.ensure_generation().unwrap();
        let stdin = generation.stdin.lock().await;
        for explicit_cancel in [true, false] {
            let cancel = CancellationToken::new();
            let mut start = Box::pin(bridge.call_bound_start("fixture.start", (), &cancel, |_| {
                panic!("unwritten start cannot be acknowledged")
            }));
            tokio::select! {
                biased;
                _ = &mut start => panic!("start did not queue"),
                () = std::future::ready(()) => {},
            }
            assert_eq!(generation.replies.slots.lock().unwrap().len(), 1);
            if explicit_cancel {
                cancel.cancel();
                tokio::select! {
                    biased;
                    result = &mut start => assert!(matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail.contains("cancelled"))),
                    () = std::future::ready(()) => panic!("queued start cancellation awaited reaping"),
                }
            }
            drop(start);
            assert!(generation.replies.slots.lock().unwrap().is_empty());
            assert!(!generation.retired.is_cancelled());
            assert!(Arc::ptr_eq(
                &generation,
                &bridge.ensure_generation().unwrap()
            ));
        }
        assert!(stdin.is_some());
        drop(stdin);
        drop(bridge);
        tokio::time::timeout(Duration::from_secs(8), generation.wait_reaped())
            .await
            .unwrap();
    }

    #[test]
    fn bound_reply_envelope_rejects_missing_mismatched_and_duplicate_fields() {
        for line in [
            r#"["2.0",7,null]"#,
            r#"{"id":7,"result":null}"#,
            r#"{"jsonrpc":"2.0","id":8,"result":null}"#,
            r#"{"jsonrpc":"2.0","id":7}"#,
            r#"{"jsonrpc":"2.0","id":7,"error":null}"#,
            r#"{"jsonrpc":"2.0","jsonrpc":"2.0","id":7,"result":null}"#,
            r#"{"jsonrpc":"2.0","id":7,"id":7,"result":null}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":null,"result":true}"#,
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-1,"message":"a"},"error":{"code":-2,"message":"b"}}"#,
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32004,"message":"busy","data":{"reason":"unknown","reason":"voice_capture_legacy_busy"}}}"#,
        ] {
            assert!(
                matches!(
                    BoundReplyEnvelope::decode(line, 7),
                    Err(DesktopError::BridgeFailed(_))
                ),
                "accepted {line}"
            );
        }
    }

    #[test]
    fn generation_replies_routes_bad_bound_envelopes_without_tightening_ordinary_replies() {
        let cases = [
            (r#"{"jsonrpc":"1.0","id":7,"result":true}"#, Ok(json!(true))),
            (
                r#"{"jsonrpc":"2.0","id":7,"result":null,"error":{"code":-1,"message":"legacy wins"}}"#,
                Err(-1),
            ),
            (
                r#"{"jsonrpc":"2.0","id":7,"error":[-32602,"positional",null]}"#,
                Err(-32602),
            ),
            (
                r#"{"jsonrpc":"2.0","id":7,"result":true,"error":null}"#,
                Ok(json!(true)),
            ),
            (
                r#"{"jsonrpc":"2.0","id":7,"result":true,"method":"not.a.response","params":{}}"#,
                Ok(json!(true)),
            ),
            (
                r#"{"jsonrpc":"2.0","id":7,"result":{"released":false,"released":true}}"#,
                Ok(json!({"released": true})),
            ),
        ];
        for (line, ordinary) in cases {
            for kind in [ReplyKind::Bound, ReplyKind::Ordinary] {
                let is_bound = matches!(kind, ReplyKind::Bound);
                let replies = GenerationReplies::default();
                let (tx, mut rx) = oneshot::channel();
                replies.register(7, ReplySlot { kind, tx }).unwrap();
                replies.route_line(line);
                let result = rx
                    .try_recv()
                    .unwrap_or_else(|err| panic!("not routed: {line}: {err}"));
                if is_bound {
                    assert!(
                        matches!(result, Err(DesktopError::BridgeFailed(detail)) if detail == "malformed bound response envelope"),
                        "accepted {line}"
                    );
                } else {
                    assert_eq!(result.unwrap().map_err(|err| err.code), ordinary, "{line}");
                }
            }
        }
    }

    #[test]
    fn bound_reply_envelope_accepts_a_single_result_or_error() {
        for (literal, value) in [
            ("null", Value::Null),
            ("true", json!(true)),
            ("[1]", json!([1])),
            (r#"{"released":true}"#, json!({"released": true})),
        ] {
            let line = format!(r#"{{"jsonrpc":"2.0","id":7,"result":{literal}}}"#);
            assert_eq!(
                BoundReplyEnvelope::decode(&line, 7).unwrap().unwrap(),
                value
            );
        }
        let err = BoundReplyEnvelope::decode(
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32004,"message":"timeout"}}"#,
            7,
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(err.code, ERR_TIMEOUT);
    }

    #[tokio::test]
    async fn wait_reaped_fails_closed_on_lost_proof() {
        let (sender, reaped) = watch::channel(false);
        let generation = BridgeGeneration {
            stdin: tokio::sync::Mutex::new(None),
            replies: GenerationReplies::default(),
            retired: CancellationToken::new(),
            drain_ready_replies: AtomicBool::new(false),
            reaped,
            runtime: Handle::current(),
            #[cfg(unix)]
            pause_reaper: Mutex::new(None),
        };
        generation.retired.cancel();
        drop(sender);
        tokio::select! {
            biased;
            () = generation.wait_reaped() => panic!("closed proof channel was treated as reaped"),
            () = std::future::ready(()) => {},
        }
    }
}
