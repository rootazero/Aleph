//! Panel-side TTS playback — the endpoint half of the voice channel.
//!
//! The core synthesizes the agent's reply (`voice.synthesize`, reusing the
//! channel TTS path) and hands the audio back; the Panel plays it through a
//! detached `<audio>` element. Triggered from [`super::events`] when a run the
//! mic button registered for speaking completes. Capture and playback live at
//! the endpoint (R1/R6); STT/LLM/TTS stay in the core.

use super::state::ChatState;
use crate::context::DashboardState;
use crate::views::voice::audio::{base64_to_bytes, bytes_to_object_url};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// `HTMLMediaElement.error.code` for "the resource is not supported".
///
/// This is the ONLY code that means "this system cannot decode these bytes".
/// A rejected `play()` promise is a different thing entirely — usually the
/// autoplay policy — and pointing that user at a GStreamer package would be a
/// confident wrong answer.
const MEDIA_ERR_SRC_NOT_SUPPORTED: u16 = 4;

/// Should this media error be reported to the user as a missing decoder?
///
/// Narrow by construction: only the not-supported code, and only on Linux,
/// where WebKitGTK's decoding depends on GStreamer plugin packages the
/// distribution may not have installed. On Windows (WebView2) and macOS
/// (WKWebView) the OS decodes and there is no package to name, so a
/// not-supported error there is a real bug worth a console warning, not a
/// user-facing install instruction.
fn is_missing_decoder(code: Option<u16>, platform: crate::platform_host::HostPlatform) -> bool {
    code == Some(MEDIA_ERR_SRC_NOT_SUPPORTED) && decoder_advice_applies(platform)
}

/// Is "install a decoder package" actionable advice on this platform?
///
/// Both legs of the receipt need this and neither may restate it. The
/// pre-check leg (`canPlayType` returned the engine's definite "no") and the
/// error leg ([`is_missing_decoder`]) had two copies of the clause, and only
/// the second had tests — so the first could have drifted to a different
/// answer with nothing red.
///
/// Only Linux: WebKitGTK decodes through GStreamer plugin packages a
/// distribution may not install. Windows (WebView2) and macOS (WKWebView)
/// decode through the OS, where there is no package to name and a
/// not-supported error is a real bug rather than a missing install.
const fn decoder_advice_applies(platform: crate::platform_host::HostPlatform) -> bool {
    matches!(platform, crate::platform_host::HostPlatform::Linux)
}

/// Synthesize `text` via the core and play it back. Best-effort: a missing TTS
/// provider or a playback rejection is logged and otherwise ignored — the text
/// reply the user already sees must never be broken by the spoken layer.
pub fn speak(dash: &DashboardState, chat: &ChatState, text: String) {
    let dash = *dash;
    let chat = *chat;
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
        // Pre-check leg. An empty string from canPlayType is the engine saying
        // "definitely not"; "maybe"/"probably" are hedges we do NOT trust,
        // because whether WebKitGTK's canPlayType consults the GStreamer
        // registry is unverified. So a hedge falls through and plays, and the
        // error receipt in `play` is the second leg. Two legs, neither
        // load-bearing alone.
        if let Ok(audio) = web_sys::HtmlAudioElement::new() {
            if audio.can_play_type(mime).is_empty()
                && decoder_advice_applies(crate::platform_host::host())
            {
                chat.voice_notice.set(Some(format!(
                    "This system cannot decode {mime}. Voice replies need the GStreamer \
                     decoder plugins on THIS machine — gstreamer1.0-plugins-good, -bad \
                     and -ugly cover the usual set. If the Aleph server runs here too, \
                     `aleph doctor` names the exact one."
                )));
                return;
            }
        }
        // Inline bytes play via a blob object URL — WKWebView is unreliable
        // with a large `data:` URL through `<audio>` (the no-sound bug the
        // immersive player already works around); remote URLs pass through.
        if let Some(bytes) = val
            .get("audio_base64")
            .and_then(|b| b.as_str())
            .and_then(base64_to_bytes)
        {
            if let Some(url) = bytes_to_object_url(&bytes, mime) {
                play(chat, &url, true);
            }
            return;
        }
        if let Some(url) = val.get("audio_url").and_then(|u| u.as_str()) {
            play(chat, url, false);
        }
    });
}

/// Create a detached `HTMLAudioElement` and start playback. A one-shot
/// `onended` closure keeps the element alive for the duration of playback,
/// then releases it (and revokes the blob URL when `revoke`) so both can be
/// garbage-collected. A rejected `play()` is logged and the URL reclaimed —
/// a silent reply with no trace is the no-sound bug.
fn play(chat: ChatState, src: &str, revoke: bool) {
    let Ok(audio) = web_sys::HtmlAudioElement::new_with_src(src) else {
        if revoke {
            let _ = web_sys::Url::revoke_object_url(src);
        }
        return;
    };
    let keep = audio.clone();
    // Third clone, held alongside `keep`, so the error object is still
    // reachable inside the async block below after the rejection.
    let keep_for_error = audio.clone();
    let ended_url = revoke.then(|| src.to_string());
    let on_ended = Closure::once_into_js(move || {
        if let Some(u) = ended_url {
            let _ = web_sys::Url::revoke_object_url(&u);
        }
        chat.voice_notice.set(None);
        drop(keep);
    });
    audio.set_onended(Some(on_ended.unchecked_ref()));
    let rejected_url = revoke.then(|| src.to_string());
    if let Ok(promise) = audio.play() {
        spawn_local(async move {
            if let Err(e) = wasm_bindgen_futures::JsFuture::from(promise).await {
                web_sys::console::warn_1(&format!("voice playback rejected: {e:?}").into());
                let code = keep_for_error.error().map(|err| err.code());
                if is_missing_decoder(code, crate::platform_host::host()) {
                    chat.voice_notice.set(Some(
                        // "THIS machine" is load-bearing: `aleph doctor` answers
                        // for the machine running the CORE, and its codec check
                        // returns nothing off Linux. In the co-located desktop
                        // App they are the same machine, but a Linux browser
                        // pointed at a remote core would otherwise be sent to a
                        // diagnostic about the wrong computer.
                        "This system cannot decode the voice reply. Install the GStreamer \
                         decoder plugins on THIS machine — gstreamer1.0-plugins-good, \
                         -bad and -ugly cover the usual set. If the Aleph server runs \
                         here too, `aleph doctor` names the exact one."
                            .to_string(),
                    ));
                }
                if let Some(u) = rejected_url {
                    let _ = web_sys::Url::revoke_object_url(&u);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_host::HostPlatform;

    #[test]
    fn only_the_not_supported_code_on_linux_is_a_decoder_problem() {
        assert!(is_missing_decoder(Some(4), HostPlatform::Linux));
    }

    #[test]
    fn other_media_errors_are_not_decoder_problems() {
        // 1 = ABORTED, 2 = NETWORK, 3 = DECODE (a corrupt stream, not a
        // missing decoder). None = the play() promise rejected with no media
        // error at all, which is usually the autoplay policy.
        for code in [Some(1), Some(2), Some(3), None] {
            assert!(
                !is_missing_decoder(code, HostPlatform::Linux),
                "code {code:?} must not be reported as a missing decoder"
            );
        }
    }

    #[test]
    fn other_platforms_decode_through_the_os_so_there_is_no_package_to_name() {
        assert!(!is_missing_decoder(Some(4), HostPlatform::Windows));
        assert!(!is_missing_decoder(Some(4), HostPlatform::MacOs));
    }
}
