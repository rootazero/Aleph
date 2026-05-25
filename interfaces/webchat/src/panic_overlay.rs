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

const OVERLAY_ID: &str = "aleph-panic-overlay";

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

    // 2. Mount a one-shot overlay. If anything in the overlay path itself
    //    panics (it shouldn't — we only touch DOM), swallow it so we don't
    //    recurse into the hook.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mount_overlay(&info.to_string());
    }));
}

fn mount_overlay(message: &str) {
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

    let escaped = escape_html(message);
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
                          white-space:pre-wrap;word-break:break-word;\">{escaped}</pre>\
           </details>\
           <div style=\"display:flex;gap:10px;justify-content:flex-end;\">\
             <button id=\"{OVERLAY_ID}-dismiss\" style=\"padding:10px 16px;border:1px solid #404040;\
                          background:transparent;color:#d4d4d8;border-radius:10px;font-size:14px;\
                          cursor:pointer;\">Dismiss</button>\
             <button id=\"{OVERLAY_ID}-reload\" style=\"padding:10px 16px;border:0;\
                          background:#dc2626;color:#fff;border-radius:10px;font-size:14px;\
                          font-weight:500;cursor:pointer;\">Reload Panel</button>\
           </div>\
         </div>",
        OVERLAY_ID = OVERLAY_ID,
        escaped = escaped,
    ));

    let _ = body.append_child(&overlay);
    wire_buttons(&document);
}

/// Attach click handlers to the Reload / Dismiss buttons.
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
}
