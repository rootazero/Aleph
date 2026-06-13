//! Wasm audio glue for the immersive voice mode. No business logic here —
//! VAD/splitting/phase decisions live in the pure modules.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::context::DashboardState;

/// Microphone session: one getUserMedia stream shared by the level meter
/// (AnalyserNode, polled on an interval) and utterance capture (MediaRecorder
/// started/stopped per VAD verdict).
pub(crate) struct MicSession {
    stream: web_sys::MediaStream,
    _ctx: web_sys::AudioContext,
    analyser: web_sys::AnalyserNode,
    recorder: RefCell<Option<web_sys::MediaRecorder>>,
    chunks: Rc<RefCell<Vec<web_sys::Blob>>>,
    _on_data: RefCell<Option<Closure<dyn FnMut(web_sys::BlobEvent)>>>,
    buf: RefCell<Vec<u8>>,
}

impl MicSession {
    /// Open the mic with system AEC on (spec decision: 系统 AEC).
    pub(crate) async fn open() -> Result<Rc<Self>, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let devices = window.navigator().media_devices()?;
        let constraints = web_sys::MediaStreamConstraints::new();
        let audio = js_sys::Object::new();
        js_sys::Reflect::set(&audio, &"echoCancellation".into(), &true.into())?;
        js_sys::Reflect::set(&audio, &"noiseSuppression".into(), &true.into())?;
        constraints.set_audio(&audio.into());
        let stream: web_sys::MediaStream =
            JsFuture::from(devices.get_user_media_with_constraints(&constraints)?)
                .await?
                .dyn_into()?;

        let ctx = web_sys::AudioContext::new()?;
        let source = ctx.create_media_stream_source(&stream)?;
        let analyser = ctx.create_analyser()?;
        analyser.set_fft_size(1024);
        source.connect_with_audio_node(&analyser)?;

        Ok(Rc::new(Self {
            stream,
            _ctx: ctx,
            analyser,
            recorder: RefCell::new(None),
            chunks: Rc::new(RefCell::new(Vec::new())),
            _on_data: RefCell::new(None),
            buf: RefCell::new(vec![0u8; 1024]),
        }))
    }

    /// Current input level as RMS in 0..1 (time-domain bytes centered at 128).
    pub(crate) fn rms(&self) -> f32 {
        let mut buf = self.buf.borrow_mut();
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

    /// Begin capturing an utterance segment.
    pub(crate) fn start_segment(&self) -> Result<(), JsValue> {
        self.chunks.borrow_mut().clear();
        let recorder = web_sys::MediaRecorder::new_with_media_stream(&self.stream)?;
        let chunks = Rc::clone(&self.chunks);
        let on_data = Closure::<dyn FnMut(_)>::new(move |ev: web_sys::BlobEvent| {
            if let Some(blob) = ev.data() {
                chunks.borrow_mut().push(blob);
            }
        });
        recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));
        recorder.start()?;
        *self._on_data.borrow_mut() = Some(on_data);
        *self.recorder.borrow_mut() = Some(recorder);
        Ok(())
    }

    /// Stop the segment and return (base64, mime). Mirrors composer/voice.rs's
    /// browser path: blob -> FileReader data URL -> strip prefix.
    pub(crate) async fn stop_segment(&self) -> Result<(String, String), JsValue> {
        let recorder = self
            .recorder
            .borrow_mut()
            .take()
            .ok_or_else(|| JsValue::from_str("no active segment"))?;
        let mime = recorder.mime_type();
        // onstop fires after the final dataavailable — await it.
        let (tx, rx) = futures::channel::oneshot::channel::<()>();
        let on_stop = Closure::once(move || {
            let _ = tx.send(());
        });
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));
        recorder.stop()?;
        let _ = rx.await;
        drop(on_stop);

        let parts = js_sys::Array::new();
        for blob in self.chunks.borrow().iter() {
            parts.push(blob);
        }
        let bag = web_sys::BlobPropertyBag::new();
        bag.set_type(&mime);
        let merged = web_sys::Blob::new_with_blob_sequence_and_options(parts.as_ref(), &bag)?;
        let data_url = read_blob_as_data_url(&merged).await?;
        let base64 = data_url
            .split_once(";base64,")
            .map(|(_, b)| b.to_string())
            .ok_or_else(|| JsValue::from_str("unexpected data url"))?;
        let mime = if mime.is_empty() {
            "audio/webm".to_string()
        } else {
            mime
        };
        Ok((base64, mime))
    }

    pub(crate) fn close(&self) {
        if let Some(rec) = self.recorder.borrow_mut().take() {
            let _ = rec.stop();
        }
        for track in self.stream.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
        let _ = self._ctx.close();
    }
}

async fn read_blob_as_data_url(blob: &web_sys::Blob) -> Result<String, JsValue> {
    let reader = web_sys::FileReader::new()?;
    let (tx, rx) = futures::channel::oneshot::channel::<Result<String, JsValue>>();
    let reader_c = reader.clone();
    let onload = Closure::once(move || {
        let res = reader_c.result().and_then(|v| {
            v.as_string()
                .ok_or_else(|| JsValue::from_str("not a string"))
        });
        let _ = tx.send(res);
    });
    reader.set_onloadend(Some(onload.as_ref().unchecked_ref()));
    reader.read_as_data_url(blob)?;
    let out = rx
        .await
        .map_err(|_| JsValue::from_str("reader dropped"))??;
    drop(onload);
    Ok(out)
}

/// Sequential TTS sentence player with interrupt support.
pub(crate) struct TtsPlayer {
    queue: Rc<RefCell<VecDeque<String>>>,
    current: Rc<RefCell<Option<web_sys::HtmlAudioElement>>>,
    /// True once the run is complete AND the splitter flushed — drain means done.
    finalized: Rc<RefCell<bool>>,
    playing: Rc<RefCell<bool>>,
    /// Component callbacks.
    on_first_audio: Rc<dyn Fn()>,
    on_drained: Rc<dyn Fn()>,
    on_sentence: Rc<dyn Fn(String)>,
    started_any: Rc<RefCell<bool>>,
}

impl TtsPlayer {
    pub(crate) fn new(
        on_first_audio: impl Fn() + 'static,
        on_drained: impl Fn() + 'static,
        on_sentence: impl Fn(String) + 'static,
    ) -> Rc<Self> {
        Rc::new(Self {
            queue: Rc::new(RefCell::new(VecDeque::new())),
            current: Rc::new(RefCell::new(None)),
            finalized: Rc::new(RefCell::new(false)),
            playing: Rc::new(RefCell::new(false)),
            on_first_audio: Rc::new(on_first_audio),
            on_drained: Rc::new(on_drained),
            on_sentence: Rc::new(on_sentence),
            started_any: Rc::new(RefCell::new(false)),
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
        self.queue.borrow_mut().clear();
        if let Some(audio) = self.current.borrow_mut().take() {
            audio.pause().ok();
            audio.set_onended(None);
            audio.set_src("");
        }
        *self.playing.borrow_mut() = false;
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
        let this = Rc::clone(self);
        spawn_local(async move {
            let src = synthesize_to_src(&dash, &sentence).await;
            match src {
                Some(src) => {
                    if !*this.started_any.borrow() {
                        *this.started_any.borrow_mut() = true;
                        (this.on_first_audio)();
                    }
                    (this.on_sentence)(sentence);
                    this.play_then_pump(dash, &src);
                }
                None => {
                    // TTS failed: caption-only for this sentence, keep going (P7).
                    (this.on_sentence)(sentence);
                    *this.playing.borrow_mut() = false;
                    this.pump(dash);
                }
            }
        });
    }

    fn play_then_pump(self: &Rc<Self>, dash: DashboardState, src: &str) {
        let Ok(audio) = web_sys::HtmlAudioElement::new_with_src(src) else {
            *self.playing.borrow_mut() = false;
            self.pump(dash);
            return;
        };
        let this = Rc::clone(self);
        // Known small leak: `once_into_js` only frees itself when invoked — if
        // stop_all() drops the audio before `ended` fires, one closure leaks
        // per interrupted sentence. Acceptable for v1.
        let on_ended = Closure::once_into_js(move || {
            *this.playing.borrow_mut() = false;
            *this.current.borrow_mut() = None;
            this.pump(dash);
        });
        audio.set_onended(Some(on_ended.unchecked_ref()));
        let _ = audio.play();
        *self.current.borrow_mut() = Some(audio);
    }
}

/// voice.synthesize -> playable src (data url or remote url). None on failure.
async fn synthesize_to_src(dash: &DashboardState, text: &str) -> Option<String> {
    let resp = dash
        .rpc_call("voice.synthesize", serde_json::json!({ "text": text }))
        .await
        .ok()?;
    let mime = resp
        .get("mime_type")
        .and_then(|v| v.as_str())
        .unwrap_or("audio/mpeg");
    if let Some(b64) = resp.get("audio_base64").and_then(|v| v.as_str()) {
        return Some(format!("data:{mime};base64,{b64}"));
    }
    resp.get("audio_url")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}
