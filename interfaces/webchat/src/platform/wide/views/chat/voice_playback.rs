//! Panel-side TTS playback — the endpoint half of the voice channel.
//!
//! The core synthesizes the agent's reply (`voice.synthesize`, reusing the
//! channel TTS path) and hands the audio back; the Panel plays it through a
//! detached `<audio>` element. Triggered from [`super::events`] when a run the
//! mic button registered for speaking completes. Capture and playback live at
//! the endpoint (R1/R6); STT/LLM/TTS stay in the core.

use crate::context::DashboardState;
use crate::views::voice::audio::{base64_to_bytes, bytes_to_object_url};
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// Synthesize `text` via the core and play it back. Best-effort: a missing TTS
/// provider or a playback rejection is logged and otherwise ignored — the text
/// reply the user already sees must never be broken by the spoken layer.
pub fn speak(dash: &DashboardState, text: String) {
    let dash = *dash;
    spawn_local(async move {
        let params = serde_json::json!({ "text": text });
        let val = match dash.rpc_call("voice.synthesize", params).await {
            Ok(v) => v,
            Err(e) => {
                web_sys::console::warn_1(&format!("voice.synthesize failed: {e}").into());
                return;
            }
        };

        let mime = val
            .get("mime_type")
            .and_then(|m| m.as_str())
            .unwrap_or("audio/mpeg");
        // Inline bytes play via a blob object URL — WKWebView is unreliable
        // with a large `data:` URL through `<audio>` (the no-sound bug the
        // immersive player already works around); remote URLs pass through.
        if let Some(bytes) = val
            .get("audio_base64")
            .and_then(|b| b.as_str())
            .and_then(base64_to_bytes)
        {
            if let Some(url) = bytes_to_object_url(&bytes, mime) {
                play(&url, true);
            }
            return;
        }
        if let Some(url) = val.get("audio_url").and_then(|u| u.as_str()) {
            play(url, false);
        }
    });
}

/// Create a detached `HTMLAudioElement` and start playback. A one-shot
/// `onended` closure keeps the element alive for the duration of playback,
/// then releases it (and revokes the blob URL when `revoke`) so both can be
/// garbage-collected. A rejected `play()` is logged and the URL reclaimed —
/// a silent reply with no trace is the no-sound bug.
fn play(src: &str, revoke: bool) {
    let Ok(audio) = web_sys::HtmlAudioElement::new_with_src(src) else {
        if revoke {
            let _ = web_sys::Url::revoke_object_url(src);
        }
        return;
    };
    let keep = audio.clone();
    let ended_url = revoke.then(|| src.to_string());
    let on_ended = Closure::once_into_js(move || {
        if let Some(u) = ended_url {
            let _ = web_sys::Url::revoke_object_url(&u);
        }
        drop(keep);
    });
    audio.set_onended(Some(on_ended.unchecked_ref()));
    let rejected_url = revoke.then(|| src.to_string());
    if let Ok(promise) = audio.play() {
        spawn_local(async move {
            if let Err(e) = wasm_bindgen_futures::JsFuture::from(promise).await {
                web_sys::console::warn_1(&format!("voice playback rejected: {e:?}").into());
                if let Some(u) = rejected_url {
                    let _ = web_sys::Url::revoke_object_url(&u);
                }
            }
        });
    }
}
