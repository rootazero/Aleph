//! Panic-recovery overlay.
//!
//! WASM panics terminate the Leptos reactive runtime, so the existing
//! `ErrorBoundary` in `context.rs` (which catches `Result` errors during
//! render) cannot help. The default `console_error_panic_hook` writes a
//! formatted trace to the JS console but leaves the user staring at a
//! frozen or blank UI — there is no signal that a reload would unblock
//! them.
//!
//! Inspired by openhuman's `ErrorFallbackScreen.tsx` (a Sentry boundary
//! fallback with a one-click reset), but implemented at the panic-hook
//! layer because Leptos has no React-style runtime recovery to recover
//! into. The overlay is mounted via raw DOM mutation — no Leptos calls
//! after the panic — so it is robust to a corrupted reactive runtime.
//!
//! All formatting still goes through `console_error_panic_hook` so the
//! dev-tools console output is unchanged for engineers; the overlay is
//! purely additive for end-users.

use std::panic::PanicHookInfo;
use std::sync::Once;

#[cfg(target_arch = "wasm32")]
const OVERLAY_ID: &str = "aleph-panic-overlay";
/// localStorage key holding the crash-history ring buffer (JSON array).
#[cfg(target_arch = "wasm32")]
const CRASH_LOG_KEY: &str = "aleph.panel.crashes";
/// Maximum number of crash records retained across reloads.
#[cfg(target_arch = "wasm32")]
const CRASH_LOG_CAP: usize = 10;

/// Install the recovery panic hook. Idempotent — repeat calls are no-ops.
/// Call once at WASM module start, replacing the bare
/// `console_error_panic_hook::set_once()` call.
pub fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        std::panic::set_hook(Box::new(hook));
    });
}

fn hook(info: &PanicHookInfo<'_>) {
    // 1. Preserve dev-console behavior — same trace, same formatting.
    console_error_panic_hook::hook(info);

    // 2. Capture a symbolicated JS backtrace, persist a crash record, and
    //    mount a one-shot recovery overlay.
    //
    //    Every step degrades to a default instead of panicking, and that is
    //    the only protection there is: a panic raised INSIDE a panic hook
    //    aborts the process before any unwinding starts ("thread panicked
    //    while processing panic"), so the `catch_unwind` an earlier version
    //    wrapped this in — with a comment promising it would swallow
    //    recursion — could never catch anything. Measured on the native test
    //    harness: one failing assertion after `install()` had run took the
    //    whole test binary down, and every test sorted after this module
    //    with it.
    //
    //    That harness is the only place this hook runs off-wasm, and there
    //    the `js_sys`/`web_sys` calls below ARE the nested panic (every
    //    wasm-bindgen import panics on a non-wasm target). So the DOM half is
    //    wasm-only; on native the hook is exactly `console_error_panic_hook`,
    //    which writes to stderr and lets the panic unwind like any other.
    #[cfg(target_arch = "wasm32")]
    {
        let message = info.to_string();
        let stack = capture_js_stack();
        let crash_count = persist_crash(&message, &stack);
        mount_overlay(&message, &stack, crash_count);
    }
}

/// Best-effort capture of the JS call stack from within the panic hook. With
/// section preserved (the `wasm-release` profile), these frames carry Rust
/// symbol names. Returns an empty string if unavailable.
#[cfg(target_arch = "wasm32")]
fn capture_js_stack() -> String {
    use wasm_bindgen::JsValue;
    let err = js_sys::Error::new("");
    js_sys::Reflect::get(&err, &JsValue::from_str("stack"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default()
}

/// Append a crash record to the localStorage ring buffer and return the new
/// record count. No-op (returns 0) if localStorage is unavailable. Never
/// panics — every fallible step degrades to a default.
#[cfg(target_arch = "wasm32")]
fn persist_crash(message: &str, stack: &str) -> usize {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return 0;
    };
    let existing = storage
        .get_item(CRASH_LOG_KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| "[]".to_string());
    let record = serde_json::json!({
        "ts": js_sys::Date::now(),
        "version": env!("ALEPH_VERSION"),
        "message": message,
        "stack": stack,
        "url": current_url(),
    });
    let new_log = append_capped(&existing, &record.to_string(), CRASH_LOG_CAP);
    let _ = storage.set_item(CRASH_LOG_KEY, &new_log);
    serde_json::from_str::<Vec<serde_json::Value>>(&new_log)
        .map(|v| v.len())
        .unwrap_or(0)
}

/// Current page URL, or an empty string if unavailable.
///
/// The URL is dropped into the `aleph.panel.crashes` localStorage ring buffer
/// on every panic. The Page may carry credentials as query params
/// (`?token=…`, `?bt=…`) before the WS handshake runs `scrub_credentials_from_url`,
/// and that ring buffer survives a `clear_credentials()` call (it scrubs
/// sessionStorage, not localStorage). Strip the credential-bearing params
/// here so a panic during cold-start cannot leak the gateway token into a
/// key the XSS blast radius already covers.
#[cfg(target_arch = "wasm32")]
fn current_url() -> String {
    let raw = web_sys::window()
        .and_then(|w| w.location().href().ok())
        .unwrap_or_default();
    strip_credentials(&raw)
}

/// Drop `?token=…` and `?bt=…` (and their `&` siblings) from a URL, mirroring
/// `context::strip_params` so the format the crash ring stores cannot be
/// distinguished from a post-handshake URL.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn strip_credentials(url: &str) -> String {
    let Some(q_start) = url.find('?') else {
        return url.to_string();
    };
    let (path, query_with_fragment) = url.split_at(q_start);
    // Strip the leading `?` so we can split on `&` / `#` cleanly.
    let rest = &query_with_fragment[1..];
    let (query, fragment) = match rest.find('#') {
        Some(hash) => (&rest[..hash], &rest[hash..]),
        None => (rest, ""),
    };
    let mut kept: Vec<&str> = Vec::with_capacity(query.len() / 8);
    let mut changed = false;
    for pair in query.split('&') {
        if pair.is_empty() {
            kept.push(pair);
            continue;
        }
        let key = pair.split('=').next().unwrap_or(pair);
        if key == "token" || key == "bt" {
            changed = true;
            continue;
        }
        kept.push(pair);
    }
    if !changed {
        return url.to_string();
    }
    let joined = kept.join("&");
    if joined.is_empty() {
        format!("{path}{fragment}")
    } else {
        format!("{path}?{joined}{fragment}")
    }
}

#[cfg(target_arch = "wasm32")]
fn mount_overlay(message: &str, stack: &str, crash_count: usize) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };

    // Idempotent — first panic wins; subsequent panics during the dying
    // breath of the runtime should not stack overlays on top of overlays.
    if document.get_element_by_id(OVERLAY_ID).is_some() {
        return;
    }

    let body = match document.body() {
        Some(b) => b,
        None => return,
    };

    let Ok(overlay) = document.create_element("div") else {
        return;
    };
    let _ = overlay.set_attribute("id", OVERLAY_ID);
    let _ = overlay.set_attribute(
        "style",
        // Inline styles only — the Tailwind stylesheet may not have applied
        // yet on early-boot panics. High z-index sits above every panel
        // overlay (sidebar, modal, command palette at z 60).
        "position:fixed;inset:0;z-index:2147483647;\
         display:flex;align-items:center;justify-content:center;\
         background:rgba(15,15,15,0.92);\
         color:#fff;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Inter,sans-serif;\
         padding:24px;",
    );

    // Details pane shows the panic message followed by the symbolicated stack.
    let mut details = escape_html(message);
    if !stack.is_empty() {
        details.push_str("\n\n");
        details.push_str(&escape_html(stack));
    }
    let note = escape_html(&format!(
        "{crash_count} crash report(s) saved · localStorage key: {CRASH_LOG_KEY}"
    ));
    overlay.set_inner_html(&format!(
        "<div style=\"max-width:640px;width:100%;\
                      background:#1a1a1a;border:1px solid rgba(239,68,68,0.4);\
                      border-radius:16px;padding:28px;box-shadow:0 24px 64px rgba(0,0,0,0.5);\">\
           <div style=\"display:flex;align-items:center;gap:10px;margin-bottom:8px;\">\
             <span style=\"font-size:20px;\">⚠</span>\
             <h2 style=\"margin:0;font-size:20px;font-weight:600;color:#f87171;\">Aleph Panel crashed</h2>\
           </div>\
           <p style=\"margin:0 0 16px;font-size:14px;color:#d4d4d8;line-height:1.5;\">\
             The panel encountered an unrecoverable error. Reloading restores the connection — your data is safe.\
           </p>\
           <details style=\"margin-bottom:18px;\">\
             <summary style=\"cursor:pointer;font-size:12px;color:#a1a1aa;user-select:none;\">Show details</summary>\
             <pre style=\"margin:8px 0 0;padding:12px;background:#0a0a0a;border-radius:8px;\
                          font-size:11px;color:#fca5a5;overflow:auto;max-height:200px;\
                          white-space:pre-wrap;word-break:break-word;\">{details}</pre>\
           </details>\
           <p style=\"margin:0 0 18px;font-size:11px;color:#71717a;\">{note}</p>\
           <div style=\"display:flex;gap:10px;justify-content:flex-end;\">\
             <button id=\"{OVERLAY_ID}-dismiss\" style=\"padding:10px 16px;border:1px solid #404040;\
                          background:transparent;color:#d4d4d8;border-radius:10px;font-size:14px;\
                          cursor:pointer;\">Dismiss</button>\
             <button id=\"{OVERLAY_ID}-reload\" style=\"padding:10px 16px;border:0;\
                          background:#dc2626;color:#fff;border-radius:10px;font-size:14px;\
                          font-weight:500;cursor:pointer;\">Reload Panel</button>\
           </div>\
         </div>",
    ));

    let _ = body.append_child(&overlay);
    wire_buttons(&document);
}

/// Attach click handlers to the Reload / Dismiss buttons.
#[cfg(target_arch = "wasm32")]
fn wire_buttons(document: &web_sys::Document) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    if let Some(btn) = document.get_element_by_id(&format!("{OVERLAY_ID}-reload")) {
        let cb = Closure::<dyn FnMut()>::new(move || {
            if let Some(w) = web_sys::window() {
                let _ = w.location().reload();
            }
        });
        let _ = btn
            .dyn_ref::<web_sys::HtmlElement>()
            .map(|el| el.set_onclick(Some(cb.as_ref().unchecked_ref())));
        cb.forget();
    }

    if let Some(btn) = document.get_element_by_id(&format!("{OVERLAY_ID}-dismiss")) {
        let cb = Closure::<dyn FnMut()>::new(move || {
            if let Some(w) = web_sys::window() {
                if let Some(d) = w.document() {
                    if let Some(el) = d.get_element_by_id(OVERLAY_ID) {
                        el.remove();
                    }
                }
            }
        });
        let _ = btn
            .dyn_ref::<web_sys::HtmlElement>()
            .map(|el| el.set_onclick(Some(cb.as_ref().unchecked_ref())));
        cb.forget();
    }
}

/// Escape a string for embedding inside an HTML text node / `<pre>`. We
/// only need `<`, `>`, `&` here; quotes go inside attributes via
/// `set_attribute` which already escapes.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        }
    }
    out
}

/// Append `new_record_json` (a JSON object string) to the JSON array held in
/// `existing_json`, keeping only the most recent `cap` entries. Robust to a
/// missing or corrupt existing value (treated as an empty array) and to an
/// unparseable new record (skipped). Returns the serialized JSON array.
///
/// Pure — no DOM/JS dependency — so it is unit-testable on the host.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn append_capped(existing_json: &str, new_record_json: &str, cap: usize) -> String {
    let mut arr: Vec<serde_json::Value> = serde_json::from_str(existing_json).unwrap_or_default();
    if let Ok(record) = serde_json::from_str::<serde_json::Value>(new_record_json) {
        arr.push(record);
    }
    if arr.len() > cap {
        let drop = arr.len() - cap;
        arr.drain(0..drop);
    }
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_metacharacters() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("plain"), "plain");
    }

    #[test]
    fn escape_preserves_non_ascii() {
        // Non-Latin scripts must round-trip — the panic trace might include
        // file paths or user data with CJK / emoji.
        let s = "崩溃 🚨";
        assert_eq!(escape_html(s), s);
    }

    #[test]
    fn install_is_idempotent() {
        // Two calls must not panic; the inner Once gates the actual
        // set_hook so the second call is a cheap no-op.
        install();
        install();
    }

    /// `install()` is process-global, so once the test above has run every
    /// later panic in this binary goes through `hook`. A failing assertion
    /// must then unwind like any other panic. Before the DOM half was fenced
    /// to wasm this did not fail — it ABORTED the whole test binary ("thread
    /// panicked while processing panic"), and with it every test sorted
    /// after `panic_overlay`, which is how a red canvas test read as "the
    /// harness crashed" for a round.
    #[test]
    fn a_panic_after_install_unwinds_instead_of_aborting_the_process() {
        install();
        let outcome = std::panic::catch_unwind(|| panic!("deliberate"));
        assert!(outcome.is_err());
    }

    #[test]
    fn append_capped_adds_to_empty_log() {
        let out = append_capped("[]", r#"{"message":"boom"}"#, 10);
        let arr: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["message"], "boom");
    }

    #[test]
    fn append_capped_treats_corrupt_existing_as_empty() {
        let out = append_capped("not json at all", r#"{"message":"boom"}"#, 10);
        let arr: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn append_capped_keeps_only_most_recent() {
        let existing = r#"[{"message":"a"},{"message":"b"},{"message":"c"}]"#;
        let out = append_capped(existing, r#"{"message":"d"}"#, 3);
        let arr: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["message"], "b");
        assert_eq!(arr[2]["message"], "d");
    }

    #[test]
    fn append_capped_skips_invalid_new_record() {
        let out = append_capped(r#"[{"message":"a"}]"#, "garbage", 10);
        let arr: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["message"], "a");
    }

    #[test]
    fn strip_credentials_drops_token_and_bt() {
        // Cold-start paths hand the URL to `current_url` before
        // `scrub_credentials_from_url` runs; the crash log must not keep the
        // bare value alongside an unrelated parameter.
        assert_eq!(
            strip_credentials("https://panel/?token=sk-abc&bt=bt-xyz"),
            "https://panel/"
        );
        assert_eq!(
            strip_credentials("https://panel/?token=sk-abc&step=2"),
            "https://panel/?step=2"
        );
        assert_eq!(
            strip_credentials("https://panel/?bt=bt-xyz#hash"),
            "https://panel/#hash"
        );
        assert_eq!(
            strip_credentials("https://panel/?other=ok"),
            "https://panel/?other=ok"
        );
        assert_eq!(strip_credentials("https://panel/"), "https://panel/");
    }
}
