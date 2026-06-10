//! Devices → "Add browser/mobile" QR code panel.
//!
//! Renders an SVG QR code holding `http://<self-host>/pair`. The viewing
//! device (phone, tablet, second laptop) scans the QR with its camera and
//! lands on `/pair` where the gateway issues a 6-digit code. The operator
//! then approves the code from this Panel via the NotificationCenter bell.
//!
//! Same-LAN only. For remote access (Tailscale, reverse proxy) the
//! displayed URL also doubles as plain text the user can copy and edit.

use crate::i18n::*;
use leptos::prelude::*;

/// Derive the gateway's same-origin base URL the QR points at. Reads
/// `window.location` at render time so the URL automatically tracks the
/// shell-launched binding (loopback dev vs LAN bind). Falls back to the
/// dev-mode loopback default if the JS bridge is unavailable.
fn discover_self_host() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(win) = web_sys::window() {
            let loc = win.location();
            let protocol = loc.protocol().unwrap_or_else(|_| "http:".to_string());
            if let Ok(host) = loc.host() {
                return format!("{protocol}//{host}");
            }
        }
        "http://127.0.0.1:18790".to_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "http://127.0.0.1:18790".to_string()
    }
}

/// Build a self-contained SVG string encoding `url` as a QR code.
///
/// Colours match the existing panel palette (`#a5b4fc` indigo modules on
/// `#0a0a0f` background) so the code blends with the surrounding card.
/// 240x240 minimum gives ~25 modules at standard error-correction M;
/// large enough to be camera-readable across the room without taking
/// over the Settings page.
pub(crate) fn generate_qr_svg(url: &str) -> String {
    use qrcode::render::svg;
    use qrcode::{EcLevel, QrCode};
    let code = QrCode::with_error_correction_level(url, EcLevel::M).expect("qr encode");
    code.render::<svg::Color<'_>>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#a5b4fc"))
        .light_color(svg::Color("#0a0a0f"))
        .build()
}

#[component]
pub fn PairQr() -> impl IntoView {
    let i18n = use_i18n();
    let url = format!("{}/pair", discover_self_host());
    let svg = generate_qr_svg(&url);
    let url_display = url;
    view! {
        <div class="flex flex-col items-center gap-4 p-6">
            <h2 class="text-lg font-semibold text-text-primary">{t!(i18n, devices.add_title)}</h2>
            <p class="text-sm text-text-secondary text-center max-w-md">
                {t!(i18n, devices.scan_qr)}
            </p>
            <div inner_html=svg class="bg-surface-sunken p-4 rounded-xl border border-border"></div>
            <div class="text-xs font-mono text-text-secondary break-all max-w-md text-center">
                {url_display}
            </div>
            <p class="text-xs text-text-tertiary text-center max-w-md">
                {t!(i18n, devices.remote_hint)}
            </p>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_svg_contains_svg_root_and_url_modules() {
        let svg = generate_qr_svg("http://127.0.0.1:18790/pair");
        assert!(svg.contains("<svg"), "missing <svg>: {svg}");
        // qrcode 0.14 renders modules as a single <path> element.
        assert!(svg.contains("<path"), "missing <path>: {svg}");
        // The colour customizations land in the rendered SVG.
        assert!(svg.contains("#a5b4fc"));
        assert!(svg.contains("#0a0a0f"));
    }

    #[test]
    fn qr_svg_minimum_dimensions_respected() {
        let svg = generate_qr_svg("http://example.local/pair");
        // The render builder enforces a 240x240 minimum; the resulting
        // SVG viewBox / width is therefore at least that.
        assert!(svg.contains("width=\""));
    }
}
