//! Wasm audio glue for the immersive voice mode. No business logic here —
//! VAD/splitting/phase decisions live in the pure modules.
//!
//! Capture is a *continuous* PCM tap, not a per-utterance `MediaRecorder`. A
//! `MediaRecorder` can only start capturing at the moment VAD declares speech —
//! by then the first syllable is already gone (RMS must cross the threshold
//! first, then the recorder spins up), which is exactly the "hello -> lo"
//! leading-clip bug. With a continuous tap we keep a rolling pre-roll ring and
//! seed each segment with it, so the onset is always present. Raw PCM is
//! sliceable (webm chunks are not), so the pre-roll is free; the segment is
//! WAV-encoded for the STT round-trip.
//!
//! The tap has two implementations behind one ingest path ([`ingest_pcm`]):
//!
//! 1. **`AudioWorklet` (primary)** — sample copying runs on the audio render
//!    thread; ~4096-sample batches arrive on the main thread as transferable
//!    `Float32Array` messages (zero-copy hand-off, ~12 msgs/s at 48 kHz).
//!    Mirrors FluidVoice's 4096-frame `AVAudioEngine` tap.
//! 2. **`ScriptProcessorNode` (fallback)** — the deprecated main-thread tap,
//!    kept for webviews whose `AudioContext` lacks `audioWorklet`. Same buffer
//!    size, same ingest, strictly worse scheduling (main-thread jank can drop
//!    audio callbacks).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::wav;
use crate::context::DashboardState;

/// Pre-roll lead-in retained ahead of every segment so the first syllable is
/// never clipped.
const PREROLL_MS: f32 = 320.0;
/// Hard cap on a single utterance so a stuck VAD can't grow an unbounded buffer.
const MAX_SEGMENT_SECS: f32 = 30.0;
/// `ScriptProcessorNode` buffer size (frames). 4096 ≈ 85 ms at 48 kHz — small
/// enough for a tight pre-roll, large enough to avoid callback churn.
const SCRIPT_BUF: u32 = 4096;

/// Rolling capture state, shared between the audio-thread callback (writer) and
/// the VAD tick (which arms/disarms a segment).
struct Capture {
    preroll: VecDeque<f32>,
    preroll_cap: usize,
    segment: Option<Vec<f32>>,
    max_segment: usize,
    /// Streaming-ASR tap (independent of the batch WAV segment). When `true`,
    /// the audio callback accumulates device-rate samples and, once a chunk is
    /// full, resamples → 16 kHz s16le → base64 and hands it to `on_frame`.
    streaming: bool,
    /// Device-rate mono accumulator flushed once it holds ~200 ms of audio.
    frame_accum: Vec<f32>,
    /// Upward sink for one base64 s16le-16k frame. Single-threaded wasm, so `Rc`
    /// (no `Send`/`Sync`). `None` until `set_frame_sink` is called.
    on_frame: Option<Rc<dyn Fn(String)>>,
}

/// Convert mono f32 [-1,1] PCM to little-endian s16 bytes (clamped). Scales by
/// 32768 so the full negative bound is reachable (`-1.0 → -32768`); the positive
/// side saturates at the i16 max (`1.0 → 32767`) via the clamp.
pub(crate) fn f32_to_s16le(pcm: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() * 2);
    for &s in pcm {
        let v = (s.clamp(-1.0, 1.0) * 32768.0).round() as i32;
        out.extend_from_slice(&(v.clamp(-32768, 32767) as i16).to_le_bytes());
    }
    out
}

/// The live PCM tap — worklet (audio-thread) or script-processor (fallback).
/// Both feed [`ingest_pcm`]; only wiring and teardown differ.
enum CaptureTap {
    Worklet {
        node: web_sys::AudioWorkletNode,
        _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    },
    Script {
        node: web_sys::ScriptProcessorNode,
        _on_audio: Closure<dyn FnMut(web_sys::AudioProcessingEvent)>,
    },
}

/// Microphone session: one getUserMedia stream feeding two taps — an
/// `AnalyserNode` (RMS level, polled on an interval) and a continuous PCM
/// [`CaptureTap`] (pre-roll ring + the active segment + streaming frames).
pub(crate) struct MicSession {
    stream: web_sys::MediaStream,
    ctx: web_sys::AudioContext,
    analyser: web_sys::AnalyserNode,
    tap: CaptureTap,
    _source: web_sys::MediaStreamAudioSourceNode,
    rms_buf: RefCell<Vec<u8>>,
    sample_rate: u32,
    cap: Rc<RefCell<Capture>>,
}

/// Why [`MicSession::open`] failed, mapped from the getUserMedia DOMException so
/// the immersive view shows an honest, actionable caption instead of always
/// blaming microphone permission (the old behaviour collapsed every failure —
/// busy device, insecure context, missing hardware — into "grant permission").
#[derive(Clone)]
pub(crate) enum MicError {
    /// `NotAllowedError` / `SecurityError`: the user or OS refused capture.
    Denied,
    /// `NotFoundError` / `OverconstrainedError`: no usable mic device.
    NotFound,
    /// `NotReadableError` / `AbortError`: device present but unreadable (busy,
    /// held by another app).
    NotReadable,
    /// `mediaDevices` / getUserMedia unavailable — non-secure context or a
    /// webview that doesn't expose the API.
    Unsupported,
    /// AudioContext or graph wiring failed.
    AudioContext,
    /// Any other DOMException, carrying its `name: message` verbatim so the
    /// real cause is never hidden.
    Other(String),
}

/// Is the Panel running inside the native desktop shell? The shell injects
/// `data-shell="aleph-tauri"` on `<html>` for every document it loads (including
/// the remote-served Panel), so this is reliable where `window.__TAURI__` is not
/// (Tauri does not inject its globals into a remotely-navigated origin).
///
/// The distinction matters for a mic *denial*: in the shell, wry auto-grants the
/// webview capture request, so a denial is an OS-level (macOS TCC) refusal a
/// deep-link can fix. In a plain browser the denial is a *per-origin* browser
/// permission — the macOS Settings pane is the wrong place to send the user (the
/// browser app is already allowed there); they must re-grant the site itself.
pub(crate) fn is_native_shell() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|e| e.get_attribute("data-shell"))
        .as_deref()
        == Some("aleph-tauri")
}

impl MicError {
    /// Map a getUserMedia rejection (a DOMException) to a variant by `.name`.
    fn from_get_user_media(err: JsValue) -> Self {
        let field = |k: &str| {
            js_sys::Reflect::get(&err, &k.into())
                .ok()
                .and_then(|v| v.as_string())
        };
        match field("name").unwrap_or_default().as_str() {
            "NotAllowedError" | "SecurityError" | "PermissionDeniedError" => Self::Denied,
            "NotFoundError" | "OverconstrainedError" => Self::NotFound,
            "NotReadableError" | "AbortError" | "TrackStartError" => Self::NotReadable,
            "" => Self::Other("getUserMedia rejected without an error name".to_string()),
            other => Self::Other(format!("{other}: {}", field("message").unwrap_or_default())),
        }
    }

    /// Human-facing caption for the immersive view. `native` selects the right
    /// remediation for a denial (OS TCC vs. browser per-site permission).
    pub(crate) fn caption(&self, native: bool) -> String {
        match self {
            Self::Denied if native => "需要麦克风权限：系统设置 → 隐私与安全 → 麦克风".to_string(),
            // Browser: the block is the site permission, not the OS — sending the
            // user to macOS Settings (where the browser is already allowed) is the
            // dead end they reported.
            Self::Denied => {
                "麦克风被浏览器拒绝：点击地址栏的权限图标，把本站麦克风改为「允许」，然后刷新页面"
                    .to_string()
            }
            Self::NotFound => "未检测到麦克风设备".to_string(),
            Self::NotReadable => {
                "麦克风被占用或无法读取，关闭其他占用麦克风的程序后重试".to_string()
            }
            Self::Unsupported => {
                "当前环境不支持麦克风（需安全上下文：https 或 localhost）".to_string()
            }
            Self::AudioContext => "音频上下文启动失败".to_string(),
            Self::Other(s) => format!("麦克风打开失败：{s}"),
        }
    }

    /// macOS deep-link to the Microphone privacy pane — offered only for a
    /// denial *inside the native shell*, where wry auto-grants the webview and
    /// the real gate is OS TCC. In a browser the denial is per-site, so there is
    /// no OS pane to jump to and we return `None`.
    pub(crate) fn settings_url(&self, native: bool) -> Option<&'static str> {
        match self {
            Self::Denied if native => {
                Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            }
            _ => None,
        }
    }
}

impl MicSession {
    /// Open the mic with system AEC on (spec decision: system AEC).
    pub(crate) async fn open() -> Result<Rc<Self>, MicError> {
        let window = web_sys::window().ok_or(MicError::Unsupported)?;
        // `mediaDevices` is undefined outside a secure context (and in some
        // webviews over plain http) — surface that distinctly from a denial.
        let devices = window
            .navigator()
            .media_devices()
            .map_err(|_| MicError::Unsupported)?;
        let constraints = web_sys::MediaStreamConstraints::new();
        let audio = js_sys::Object::new();
        js_sys::Reflect::set(&audio, &"echoCancellation".into(), &true.into())
            .map_err(|_| MicError::Unsupported)?;
        js_sys::Reflect::set(&audio, &"noiseSuppression".into(), &true.into())
            .map_err(|_| MicError::Unsupported)?;
        constraints.set_audio(&audio.into());
        let promise = devices
            .get_user_media_with_constraints(&constraints)
            .map_err(|_| MicError::Unsupported)?;
        // The getUserMedia promise rejects with a DOMException whose `name`
        // pinpoints the cause — switch on it instead of blaming permission.
        let stream: web_sys::MediaStream = JsFuture::from(promise)
            .await
            .map_err(MicError::from_get_user_media)?
            .dyn_into()
            .map_err(|_| MicError::Unsupported)?;

        let ctx = web_sys::AudioContext::new().map_err(|_| MicError::AudioContext)?;
        let sample_rate = ctx.sample_rate() as u32;
        let source = ctx
            .create_media_stream_source(&stream)
            .map_err(|_| MicError::AudioContext)?;
        let analyser = ctx.create_analyser().map_err(|_| MicError::AudioContext)?;
        analyser.set_fft_size(1024);
        source
            .connect_with_audio_node(&analyser)
            .map_err(|_| MicError::AudioContext)?;

        let preroll_cap = (sample_rate as f32 * PREROLL_MS / 1000.0) as usize;
        let max_segment = (sample_rate as f32 * MAX_SEGMENT_SECS) as usize;
        let cap = Rc::new(RefCell::new(Capture {
            preroll: VecDeque::with_capacity(preroll_cap + SCRIPT_BUF as usize),
            preroll_cap,
            segment: None,
            max_segment,
            streaming: false,
            frame_accum: Vec::new(),
            on_frame: None,
        }));

        // Flush a streaming frame once the accumulator holds ~200 ms of
        // device-rate audio. Kept ≥ 1 so a degenerate rate can't divide to 0.
        let frame_chunk = (sample_rate / 5).max(1) as usize;

        // Primary: AudioWorklet tap (audio-thread copy, batched hand-off).
        // Any failure — no audioWorklet on the context (older WKWebView),
        // module rejection, node construction — falls back to the deprecated
        // ScriptProcessorNode path, which still works everywhere.
        let tap = match open_worklet_tap(&ctx, &source, &cap, sample_rate, frame_chunk).await {
            Ok(tap) => tap,
            Err(e) => {
                web_sys::console::debug_1(
                    &format!("voice capture: AudioWorklet unavailable ({e:?}), using ScriptProcessor fallback").into(),
                );
                open_script_tap(&ctx, &source, &cap, sample_rate, frame_chunk)?
            }
        };

        Ok(Rc::new(Self {
            stream,
            ctx,
            analyser,
            tap,
            _source: source,
            rms_buf: RefCell::new(vec![0u8; 1024]),
            sample_rate,
            cap,
        }))
    }

    /// Current input level as RMS in 0..1 (time-domain bytes centered at 128).
    pub(crate) fn rms(&self) -> f32 {
        let mut buf = self.rms_buf.borrow_mut();
        self.analyser.get_byte_time_domain_data(&mut buf);
        let sum: f32 = buf
            .iter()
            .map(|&b| {
                let v = (f32::from(b) - 128.0) / 128.0;
                v * v
            })
            .sum();
        (sum / buf.len() as f32).sqrt()
    }

    /// Arm a capture segment, seeded with the pre-roll ring so the utterance
    /// onset (already in the ring by the time VAD fires) is preserved.
    pub(crate) fn start_segment(&self) {
        let mut c = self.cap.borrow_mut();
        let pre: Vec<f32> = c.preroll.iter().copied().collect();
        c.segment = Some(pre);
    }

    /// Drop the in-flight segment (VAD decided it was a noise blip).
    pub(crate) fn discard_segment(&self) {
        self.cap.borrow_mut().segment = None;
    }

    /// Take the captured segment and encode it as a base64 WAV for STT. Returns
    /// `None` if no segment was armed or it was empty.
    pub(crate) fn take_segment_wav(&self) -> Option<(String, String)> {
        let mut seg = self.cap.borrow_mut().segment.take()?;
        if seg.is_empty() {
            return None;
        }
        // whisper.cpp asserts (hard-crashes) on buffers shorter than 1 s — the
        // exact `[voice.local]` BYO scenario for a snappy "yes/ok" — so pad short
        // segments with trailing silence to a safe 1.1 s.
        let min_len = (self.sample_rate as f32 * 1.1) as usize;
        if seg.len() < min_len {
            seg.resize(min_len, 0.0);
        }
        let wav_bytes = wav::encode_wav_mono(&seg, self.sample_rate);
        Some((wav::base64_encode(&wav_bytes), "audio/wav".to_string()))
    }

    /// Install the upward sink for streaming s16le-16k base64 frames. Stored as
    /// `Rc` (single-threaded wasm; no `Send`/`Sync`).
    pub(crate) fn set_frame_sink(&self, cb: impl Fn(String) + 'static) {
        self.cap.borrow_mut().on_frame = Some(Rc::new(cb));
    }

    /// Begin emitting streaming frames from the live tap, seeded with the
    /// pre-roll ring. The seed matters most after a barge-in: the user is
    /// already mid-word when the fresh backend stream opens, and without the
    /// ring the utterance onset is clipped — the same "hello -> lo" bug the
    /// batch path solved by seeding segments. In the quiet re-entry case the
    /// ring holds ~320 ms of silence, which the backend VAD ignores.
    pub(crate) fn start_streaming(&self) {
        let mut c = self.cap.borrow_mut();
        c.streaming = true;
        let pre: Vec<f32> = c.preroll.iter().copied().collect();
        c.frame_accum = pre;
    }

    /// Stop emitting streaming frames and drop the partial accumulator.
    pub(crate) fn stop_streaming(&self) {
        let mut c = self.cap.borrow_mut();
        c.streaming = false;
        c.frame_accum.clear();
    }

    pub(crate) fn close(&self) {
        match &self.tap {
            CaptureTap::Worklet { node, .. } => {
                if let Ok(port) = node.port() {
                    port.set_onmessage(None);
                }
                let _ = node.disconnect();
            }
            CaptureTap::Script { node, .. } => {
                node.set_onaudioprocess(None);
                let _ = node.disconnect();
            }
        }
        let _ = self._source.disconnect();
        for track in self.stream.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
        let _ = self.ctx.close();
    }
}

// ---------------------------------------------------------------------------
// Capture taps — shared ingest + the two wiring paths
// ---------------------------------------------------------------------------

/// Feed one batch of device-rate mono samples into the capture state:
/// pre-roll ring, active segment, and (when streaming) fixed ~200 ms frames
/// resampled to 16 kHz s16le for the backend ASR stream. Single ingest shared
/// by both taps so their behavior cannot drift.
fn ingest_pcm(cap: &Rc<RefCell<Capture>>, data: &[f32], sample_rate: u32, frame_chunk: usize) {
    let mut c = cap.borrow_mut();
    let pc = c.preroll_cap;
    for &s in data {
        if c.preroll.len() >= pc {
            c.preroll.pop_front();
        }
        c.preroll.push_back(s);
    }
    let mc = c.max_segment;
    if let Some(seg) = c.segment.as_mut() {
        if seg.len() < mc {
            seg.extend_from_slice(data);
        }
    }
    // Streaming tap: accumulate device-rate samples, then emit fixed ~200 ms
    // frames resampled to 16 kHz s16le. Independent of the batch WAV segment
    // above (which is the fallback path).
    if !c.streaming {
        return;
    }
    c.frame_accum.extend_from_slice(data);
    // Drain whole chunks; draining in a loop is robust to batch sizes larger
    // than one chunk (the worklet delivers ~85 ms batches, but a stalled main
    // thread can queue several).
    let mut frames: Vec<String> = Vec::new();
    while c.frame_accum.len() >= frame_chunk {
        let chunk: Vec<f32> = c.frame_accum.drain(..frame_chunk).collect();
        // Resample device rate → 16 kHz (nearest-neighbor decimation;
        // STT is robust to this). Handles non-integer ratios (44100→16000).
        let out_len = chunk.len() * 16_000 / sample_rate as usize;
        let mut resampled = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let src = i * sample_rate as usize / 16_000;
            resampled.push(chunk[src.min(chunk.len() - 1)]);
        }
        let bytes = f32_to_s16le(&resampled);
        frames.push(wav::base64_encode(&bytes));
    }
    // Snapshot the sink and DROP the borrow before invoking it: the callback
    // spawns an RPC upward and must not re-enter `Capture` while it is
    // mutably borrowed (BorrowMutError guard).
    if frames.is_empty() {
        return;
    }
    let sink = c.on_frame.clone();
    drop(c);
    if let Some(sink) = sink {
        for b64 in frames {
            sink(b64);
        }
    }
}

/// The AudioWorklet processor, registered from a blob URL. Copies input
/// samples on the render thread into ~[`SCRIPT_BUF`]-sample batches and posts
/// each batch to the main thread as a *transferred* `Float32Array` buffer
/// (zero-copy hand-off). Outputs stay zeroed — the node is pinned to the
/// destination only so the graph pulls it; it contributes silence.
const CAPTURE_WORKLET_JS: &str = r"
class AlephCapture extends AudioWorkletProcessor {
  constructor() { super(); this.buf = new Float32Array(4096); this.n = 0; }
  process(inputs) {
    const ch = inputs[0] && inputs[0][0];
    if (ch) {
      let i = 0;
      while (i < ch.length) {
        const take = Math.min(ch.length - i, 4096 - this.n);
        this.buf.set(ch.subarray(i, i + take), this.n);
        this.n += take; i += take;
        if (this.n === 4096) {
          const out = this.buf;
          this.port.postMessage(out, [out.buffer]);
          this.buf = new Float32Array(4096); this.n = 0;
        }
      }
    }
    return true;
  }
}
registerProcessor('aleph-capture', AlephCapture);
";

/// Wire the primary AudioWorklet tap: register the processor module from a
/// blob URL, build the node, pump its port messages into [`ingest_pcm`].
async fn open_worklet_tap(
    ctx: &web_sys::AudioContext,
    source: &web_sys::MediaStreamAudioSourceNode,
    cap: &Rc<RefCell<Capture>>,
    sample_rate: u32,
    frame_chunk: usize,
) -> Result<CaptureTap, JsValue> {
    // audioWorklet is undefined on some older webviews → Err → fallback.
    let worklet = ctx.audio_worklet()?;

    // Processor module via blob URL (no separate asset to serve; works under
    // the panel's `script-src 'self'` CSP because worklet modules are fetched
    // as same-origin blobs... revoked immediately after registration).
    let parts = js_sys::Array::new();
    parts.push(&JsValue::from_str(CAPTURE_WORKLET_JS));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type("application/javascript");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(parts.as_ref(), &bag)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let loaded = JsFuture::from(worklet.add_module(&url)?).await;
    let _ = web_sys::Url::revoke_object_url(&url);
    loaded?;

    let node = web_sys::AudioWorkletNode::new(ctx, "aleph-capture")?;
    let cap_w = Rc::clone(cap);
    let on_message = Closure::<dyn FnMut(_)>::new(move |ev: web_sys::MessageEvent| {
        let data = ev.data();
        let Ok(arr) = data.dyn_into::<js_sys::Float32Array>() else {
            return;
        };
        ingest_pcm(&cap_w, &arr.to_vec(), sample_rate, frame_chunk);
    });
    node.port()?
        .set_onmessage(Some(on_message.as_ref().unchecked_ref()));

    // Pin into the graph: source → worklet → destination. The destination hop
    // is what makes the graph pull the worklet (same requirement as the old
    // ScriptProcessor); its zeroed output contributes silence.
    source.connect_with_audio_node(&node)?;
    node.connect_with_audio_node(&ctx.destination())?;

    Ok(CaptureTap::Worklet {
        node,
        _on_message: on_message,
    })
}

/// Wire the legacy ScriptProcessorNode tap (deprecated API, main-thread
/// callbacks). Only used when [`open_worklet_tap`] fails.
fn open_script_tap(
    ctx: &web_sys::AudioContext,
    source: &web_sys::MediaStreamAudioSourceNode,
    cap: &Rc<RefCell<Capture>>,
    sample_rate: u32,
    frame_chunk: usize,
) -> Result<CaptureTap, MicError> {
    let node = ctx
        .create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(
            SCRIPT_BUF, 1, 1,
        )
        .map_err(|_| MicError::AudioContext)?;
    let cap_w = Rc::clone(cap);
    let on_audio = Closure::<dyn FnMut(_)>::new(move |ev: web_sys::AudioProcessingEvent| {
        let Ok(input) = ev.input_buffer() else { return };
        let Ok(data) = input.get_channel_data(0) else {
            return;
        };
        ingest_pcm(&cap_w, &data, sample_rate, frame_chunk);
    });
    node.set_onaudioprocess(Some(on_audio.as_ref().unchecked_ref()));
    // Tap the source into the processor and pin the processor to the
    // destination so its `onaudioprocess` fires (WebKit/Chrome require a
    // live downstream). The output buffer is left untouched (silent), so
    // routing through the destination causes no mic feedback.
    source
        .connect_with_audio_node(&node)
        .map_err(|_| MicError::AudioContext)?;
    node.connect_with_audio_node(&ctx.destination())
        .map_err(|_| MicError::AudioContext)?;
    Ok(CaptureTap::Script {
        node,
        _on_audio: on_audio,
    })
}

/// Decode a base64 payload (atob) into bytes. `atob` yields a Latin-1 string
/// whose code points are 0..=255 — `c as u8` is exact, unlike iterating the
/// Rust `String`'s UTF-8 bytes (which would split code points ≥ 0x80).
/// `pub(crate)`: shared with the dictation playback path (`voice_playback`).
pub(crate) fn base64_to_bytes(b64: &str) -> Option<Vec<u8>> {
    let bin = web_sys::window()?.atob(b64).ok()?;
    Some(bin.chars().map(|c| c as u8).collect())
}

/// Wrap raw audio bytes in a `blob:` object URL. WKWebView is unreliable playing
/// a large `data:` URL through `<audio>` (the no-sound bug); a blob URL loads
/// reliably. Caller revokes the URL when playback ends.
/// `pub(crate)`: shared with the dictation playback path (`voice_playback`).
pub(crate) fn bytes_to_object_url(bytes: &[u8], mime: &str) -> Option<String> {
    let arr = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
    arr.copy_from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&arr);
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(parts.as_ref(), &bag).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

/// Synthesized audio for one sentence: inline bytes (the common 302 path) or a
/// remote URL fallback.
enum Audio {
    Bytes(Vec<u8>, String),
    Url(String),
}

/// What is currently producing sound. The primary path decodes the TTS bytes
/// into the output graph as a buffer source (audible in WKWebView, and feeds the
/// analyser so the orb pulses to the reply). The element variant is the degraded
/// fallback (no output graph, decode failure, or a remote URL).
enum Playing {
    /// Buffer source on the output context — `stop()` to interrupt.
    Buffer(web_sys::AudioBufferSourceNode),
    /// Detached `<audio>` element; `url` is its object URL to revoke on teardown.
    Element {
        el: web_sys::HtmlAudioElement,
        url: Option<String>,
    },
}

/// Output graph for playback: an `AudioContext` whose analyser sits between the
/// playing element and the speakers, so the orb can pulse to the *reply* (the
/// mic hears ~nothing during playback thanks to AEC).
struct OutGraph {
    ctx: web_sys::AudioContext,
    analyser: web_sys::AnalyserNode,
}

/// Sequential TTS sentence player with interrupt support.
pub(crate) struct TtsPlayer {
    queue: Rc<RefCell<VecDeque<String>>>,
    current: Rc<RefCell<Option<Playing>>>,
    /// True once the run is complete AND the splitter flushed — drain means done.
    finalized: Rc<RefCell<bool>>,
    playing: Rc<RefCell<bool>>,
    /// Monotonic run epoch. `stop_all` (barge-in / reset / teardown) bumps it to
    /// invalidate every in-flight synthesize/decode: a continuation captures the
    /// epoch at pump time and aborts when it no longer matches, so an interrupted
    /// run cannot play stale audio, set a stale caption, or wedge the pump for the
    /// run that follows. This is what makes a mid-reply barge-in not corrupt the
    /// next turn.
    epoch: Rc<RefCell<u64>>,
    /// Component callbacks.
    on_first_audio: Rc<dyn Fn()>,
    on_drained: Rc<dyn Fn()>,
    on_sentence: Rc<dyn Fn(String)>,
    started_any: Rc<RefCell<bool>>,
    graph: Option<OutGraph>,
    level_buf: RefCell<Vec<u8>>,
}

impl TtsPlayer {
    pub(crate) fn new(
        on_first_audio: impl Fn() + 'static,
        on_drained: impl Fn() + 'static,
        on_sentence: impl Fn(String) + 'static,
    ) -> Rc<Self> {
        // Build the output graph (analyser → destination) once. Constructed
        // within the entry gesture (the overlay mounts from the mic-button tap),
        // so resuming the context here satisfies the autoplay gate — later
        // programmatic playback then flows without a fresh gesture.
        let graph = web_sys::AudioContext::new().ok().and_then(|ctx| {
            let analyser = ctx.create_analyser().ok()?;
            analyser.set_fft_size(1024);
            analyser.connect_with_audio_node(&ctx.destination()).ok()?;
            let _ = ctx.resume();
            Some(OutGraph { ctx, analyser })
        });
        Rc::new(Self {
            queue: Rc::new(RefCell::new(VecDeque::new())),
            current: Rc::new(RefCell::new(None)),
            finalized: Rc::new(RefCell::new(false)),
            playing: Rc::new(RefCell::new(false)),
            epoch: Rc::new(RefCell::new(0)),
            on_first_audio: Rc::new(on_first_audio),
            on_drained: Rc::new(on_drained),
            on_sentence: Rc::new(on_sentence),
            started_any: Rc::new(RefCell::new(false)),
            graph,
            level_buf: RefCell::new(vec![0u8; 1024]),
        })
    }

    pub(crate) fn reset(&self) {
        self.stop_all();
        *self.finalized.borrow_mut() = false;
        *self.started_any.borrow_mut() = false;
    }

    pub(crate) fn enqueue(self: &Rc<Self>, dash: DashboardState, sentence: String) {
        self.queue.borrow_mut().push_back(sentence);
        self.pump(dash);
    }

    /// Mark that no more sentences will arrive; drain fires when queue empties.
    pub(crate) fn finalize(self: &Rc<Self>, dash: DashboardState) {
        *self.finalized.borrow_mut() = true;
        self.pump(dash);
    }

    pub(crate) fn stop_all(&self) {
        // Bump the epoch first so any synthesize/decode already in flight aborts
        // instead of playing into the run that replaces this one.
        *self.epoch.borrow_mut() += 1;
        self.queue.borrow_mut().clear();
        if let Some(playing) = self.current.borrow_mut().take() {
            match playing {
                Playing::Buffer(node) => {
                    // set_onended/stop live on the parent AudioScheduledSourceNode
                    // (the AudioBufferSourceNode copies are deprecated in web-sys).
                    let sched: &web_sys::AudioScheduledSourceNode = node.unchecked_ref();
                    sched.set_onended(None);
                    let _ = sched.stop();
                }
                Playing::Element { el, url } => {
                    el.pause().ok();
                    el.set_onended(None);
                    el.set_src("");
                    if let Some(url) = url {
                        let _ = web_sys::Url::revoke_object_url(&url);
                    }
                }
            }
        }
        *self.playing.borrow_mut() = false;
    }

    /// Full teardown for overlay close: stop playback AND close the output
    /// `AudioContext`. `stop_all` alone leaves `graph.ctx` running, and a running
    /// context keeps Core Audio's IO thread alive — leaking one context per
    /// voice-mode exit is what pinned `coreaudiod` at high CPU after a day of
    /// entering/leaving voice mode. The session's player is single-use (a fresh
    /// one is built on the next overlay mount), so closing here is safe.
    pub(crate) fn close(&self) {
        self.stop_all();
        if let Some(g) = &self.graph {
            let _ = g.ctx.close();
        }
    }

    /// Output level 0..1 from the playback analyser — drives the orb while the
    /// reply is speaking. Zero when no output graph (degraded) or silence.
    pub(crate) fn output_level(&self) -> f32 {
        let Some(g) = &self.graph else { return 0.0 };
        let mut buf = self.level_buf.borrow_mut();
        g.analyser.get_byte_time_domain_data(&mut buf);
        let sum: f32 = buf
            .iter()
            .map(|&b| {
                let v = (f32::from(b) - 128.0) / 128.0;
                v * v
            })
            .sum();
        (sum / buf.len() as f32).sqrt()
    }

    /// Resume the output context. Called at a strong user-activation point (the
    /// mic grant) so the gesture-gated autoplay unlock is already in place before
    /// the first reply's audio arrives — a buffer source only plays on a running
    /// context. Harmless no-op if already running.
    pub(crate) fn resume_output(&self) {
        if let Some(g) = &self.graph {
            let _ = g.ctx.resume();
        }
    }

    fn pump(self: &Rc<Self>, dash: DashboardState) {
        if *self.playing.borrow() {
            return;
        }
        // Pop in its own statement so the queue borrow is released before the
        // drained callback runs (let-else holds scrutinee temporaries through
        // the else block).
        let next = self.queue.borrow_mut().pop_front();
        let Some(sentence) = next else {
            if *self.finalized.borrow() {
                (self.on_drained)();
            }
            return;
        };
        *self.playing.borrow_mut() = true;
        let my_epoch = *self.epoch.borrow();
        let this = Rc::clone(self);
        spawn_local(async move {
            // Single attempt — NO same-provider retry. A serial retry only
            // doubles the dead time on a slow chunk (the orb sits at "thinking"
            // twice as long) without improving the odds against a flaky endpoint;
            // openclaw's lesson is provider *fallback*, never a blocking retry.
            // Robustness comes instead from coarser chunking upstream (the
            // splitter merges short sentences → fewer, larger requests).
            let audio = synthesize(&dash, &sentence).await;
            // A barge-in / reset / new turn bumped the epoch while we were
            // synthesizing — drop this clip. Mutating playing/current now would
            // corrupt the run that superseded us (the stuck-at-thinking and
            // stale-playback bug).
            if *this.epoch.borrow() != my_epoch {
                return;
            }
            // Leave "thinking" the instant the first chunk is ready to DELIVER,
            // whether or not its audio synthesized. The caption shows the text
            // either way, so a TTS failure must not pin the phase at Processing
            // while the reply is already on screen (the "status stuck at thinking" bug).
            if !*this.started_any.borrow() {
                *this.started_any.borrow_mut() = true;
                (this.on_first_audio)();
            }
            (this.on_sentence)(sentence);
            match audio {
                Some(audio) => this.play_then_pump(dash, audio, my_epoch),
                None => {
                    // TTS failed: caption-only for this chunk, keep going (P7).
                    *this.playing.borrow_mut() = false;
                    this.pump(dash);
                }
            }
        });
    }

    /// Play one synthesized clip, then pump the next when it ends.
    ///
    /// Primary path (inline bytes + an output graph): decode the bytes and play
    /// them through the gesture-unlocked output context as a buffer source. This
    /// is the only path reliably audible inside WKWebView — playing an `<audio>`
    /// element is gated by the autoplay policy (the reply arrives long after the
    /// entry gesture), and routing one through `createMediaElementSource` hits a
    /// WebKit silent-bridge bug. A buffer source on a running context is governed
    /// only by the context state, so it just plays — and it still feeds the
    /// analyser, so the orb pulses to the reply.
    ///
    /// Fallbacks (no output graph, decode failure, or a remote URL) play a
    /// detached `<audio>` element; sound there depends on the autoplay policy,
    /// but these paths are rare — the core returns inline bytes.
    fn play_then_pump(self: &Rc<Self>, dash: DashboardState, audio: Audio, my_epoch: u64) {
        match audio {
            Audio::Bytes(bytes, mime) if self.graph.is_some() => {
                let this = Rc::clone(self);
                spawn_local(async move {
                    if !this.play_buffer(dash, &bytes, my_epoch).await {
                        // Decode/setup failed — fall back to an <audio> element.
                        this.play_element(dash, bytes_to_object_url(&bytes, &mime), true, my_epoch);
                    }
                });
            }
            Audio::Bytes(bytes, mime) => {
                self.play_element(dash, bytes_to_object_url(&bytes, &mime), true, my_epoch);
            }
            Audio::Url(u) => self.play_element(dash, Some(u), false, my_epoch),
        }
    }

    /// Decode `bytes` and play them as a buffer source on the output graph.
    /// Returns `true` once playback is armed (the source's `ended` handler pumps
    /// the next clip); `false` if decode/setup failed so the caller can fall
    /// back. Leaves `playing` set on success.
    async fn play_buffer(
        self: &Rc<Self>,
        dash: DashboardState,
        bytes: &[u8],
        my_epoch: u64,
    ) -> bool {
        let Some(g) = self.graph.as_ref() else {
            return false;
        };
        // A context resumed in the entry gesture stays running; re-issuing
        // resume() here is a harmless no-op but covers a missed unlock.
        let _ = g.ctx.resume();
        let array = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(bytes);
        // decodeAudioData detaches the ArrayBuffer; `array` is freshly allocated.
        let Ok(promise) = g.ctx.decode_audio_data(&array.buffer()) else {
            return false;
        };
        let buffer = match JsFuture::from(promise).await {
            Ok(v) => match v.dyn_into::<web_sys::AudioBuffer>() {
                Ok(b) => b,
                Err(_) => return false,
            },
            Err(e) => {
                web_sys::console::warn_1(&format!("voice decode failed: {e:?}").into());
                return false;
            }
        };
        // Decode is async. If a barge-in invalidated this run while we decoded,
        // do NOT arm the source — it would play over the next run and clobber
        // `current`. Report handled (true) so the caller skips the element
        // fallback instead of playing the stale clip there.
        if *self.epoch.borrow() != my_epoch {
            return true;
        }
        let Ok(node) = g.ctx.create_buffer_source() else {
            return false;
        };
        node.set_buffer(Some(&buffer));
        if node.connect_with_audio_node(&g.analyser).is_err() {
            return false;
        }
        let this = Rc::clone(self);
        // Known small leak: `once_into_js` frees itself only when invoked — if
        // stop_all() stops the source before `ended` fires, one closure leaks per
        // interrupted sentence. Acceptable for v1.
        let on_ended = Closure::once_into_js(move || {
            // Ignore the tail of a run a barge-in/reset already replaced.
            if *this.epoch.borrow() != my_epoch {
                return;
            }
            *this.playing.borrow_mut() = false;
            *this.current.borrow_mut() = None;
            this.pump(dash);
        });
        // set_onended/start live on the parent AudioScheduledSourceNode — the
        // AudioBufferSourceNode copies are deprecated in web-sys.
        let sched: &web_sys::AudioScheduledSourceNode = node.unchecked_ref();
        sched.set_onended(Some(on_ended.unchecked_ref()));
        // Start ~50ms ahead instead of "now": a context that was just resume()d
        // can still be ramping its output, and starting a buffer at `currentTime`
        // then drops the leading samples (WKWebView especially) — the reply loses
        // its opening syllables. A small lead-in lets the context settle so the
        // first words are fully audible. 50ms is imperceptible as latency.
        let when = g.ctx.current_time() + 0.05;
        if sched.start_with_when(when).is_err() {
            return false;
        }
        *self.current.borrow_mut() = Some(Playing::Buffer(node));
        true
    }

    /// Degraded fallback: play through a detached `<audio>` element. `src` is the
    /// blob URL (when `revoke`) or a remote URL; `None` means URL creation failed
    /// and we skip to the next clip. `my_epoch` guards every continuation the
    /// same way the buffer path does — a barge-in/reset mid-flight must not let
    /// this clip mutate the run that replaced it.
    fn play_element(
        self: &Rc<Self>,
        dash: DashboardState,
        src: Option<String>,
        revoke: bool,
        my_epoch: u64,
    ) {
        let Some(src) = src else {
            *self.playing.borrow_mut() = false;
            self.pump(dash);
            return;
        };
        let url = revoke.then(|| src.clone());
        let Ok(audio_el) = web_sys::HtmlAudioElement::new_with_src(&src) else {
            if let Some(u) = url {
                let _ = web_sys::Url::revoke_object_url(&u);
            }
            *self.playing.borrow_mut() = false;
            self.pump(dash);
            return;
        };
        let this = Rc::clone(self);
        let ended_url = url.clone();
        let on_ended = Closure::once_into_js(move || {
            // The URL belongs to this clip — revoke unconditionally.
            if let Some(u) = ended_url {
                let _ = web_sys::Url::revoke_object_url(&u);
            }
            // But do not touch the pump state of a run that replaced us.
            if *this.epoch.borrow() != my_epoch {
                return;
            }
            *this.playing.borrow_mut() = false;
            *this.current.borrow_mut() = None;
            this.pump(dash);
        });
        audio_el.set_onended(Some(on_ended.unchecked_ref()));
        // A rejected play() (autoplay policy, decode failure) means `ended`
        // never fires — without clearing `playing` here the pump wedges and
        // every later sentence of the reply is silently dropped.
        let this = Rc::clone(self);
        match audio_el.play() {
            Ok(promise) => {
                spawn_local(async move {
                    if let Err(e) = JsFuture::from(promise).await {
                        web_sys::console::warn_1(
                            &format!("voice element playback rejected: {e:?}").into(),
                        );
                        if *this.epoch.borrow() != my_epoch {
                            return;
                        }
                        // Reclaim the clip's blob URL — `ended` will never fire.
                        if let Some(Playing::Element { url: Some(u), .. }) =
                            this.current.borrow_mut().take()
                        {
                            let _ = web_sys::Url::revoke_object_url(&u);
                        }
                        *this.playing.borrow_mut() = false;
                        this.pump(dash);
                    }
                });
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("voice element play() failed: {e:?}").into());
                *self.playing.borrow_mut() = false;
                *self.current.borrow_mut() = None;
                self.pump(dash);
                return;
            }
        }
        *self.current.borrow_mut() = Some(Playing::Element { el: audio_el, url });
    }
}

/// voice.synthesize -> decoded audio. None on failure.
async fn synthesize(dash: &DashboardState, text: &str) -> Option<Audio> {
    let resp = dash
        .rpc_call("voice.synthesize", serde_json::json!({ "text": text }))
        .await
        .ok()?;
    let mime = resp
        .get("mime_type")
        .and_then(|v| v.as_str())
        .unwrap_or("audio/mpeg")
        .to_string();
    if let Some(b64) = resp.get("audio_base64").and_then(|v| v.as_str()) {
        let bytes = base64_to_bytes(b64)?;
        return Some(Audio::Bytes(bytes, mime));
    }
    resp.get("audio_url")
        .and_then(|v| v.as_str())
        .map(|u| Audio::Url(u.to_string()))
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    #[test]
    fn f32_to_s16le_clamps_and_scales() {
        let pcm = [0.0f32, 1.0, -1.0, 2.0, -2.0];
        let bytes = f32_to_s16le(&pcm);
        assert_eq!(bytes.len(), pcm.len() * 2);
        // 0.0 → 0
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 0);
        // 1.0 → 32767 (clamped), -1.0 → -32768, 2.0 → clamped 32767
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), 32767);
        assert_eq!(i16::from_le_bytes([bytes[4], bytes[5]]), -32768);
        assert_eq!(i16::from_le_bytes([bytes[6], bytes[7]]), 32767);
    }
}
