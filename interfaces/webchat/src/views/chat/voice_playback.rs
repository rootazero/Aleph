//! Panel-side TTS playback — the endpoint half of the voice channel.
//!
//! The core synthesizes the agent's reply (`voice.synthesize`, reusing the
//! channel TTS path) and hands the audio back; the Panel plays it through a
//! detached `<audio>` element. Triggered from [`super::events`] when a run the
//! mic button registered for speaking completes. Capture and playback live at
//! the endpoint (R1/R6); STT/LLM/TTS stay in the core.

use crate::context::DashboardState;
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
        // Prefer inline bytes as a data URL; fall back to a remote URL.
        let src = if let Some(b64) = val.get("audio_base64").and_then(|b| b.as_str()) {
            format!("data:{mime};base64,{b64}")
        } else if let Some(url) = val.get("audio_url").and_then(|u| u.as_str()) {
            url.to_string()
        } else {
            return;
        };
        play(&src);
    });
}

/// Create a detached `HTMLAudioElement` and start playback. A one-shot
/// `onended` closure keeps the element alive for the duration of playback, then
/// releases it so it can be garbage-collected (the element is never attached to
/// the DOM).
fn play(src: &str) {
    let Ok(audio) = web_sys::HtmlAudioElement::new_with_src(src) else {
        return;
    };
    let keep = audio.clone();
    let on_ended = Closure::once_into_js(move || drop(keep));
    audio.set_onended(Some(on_ended.unchecked_ref()));
    // play() returns a Promise; a rejection (e.g. autoplay policy) just means no
    // sound — acceptable for a user-initiated voice turn.
    let _ = audio.play();
}
