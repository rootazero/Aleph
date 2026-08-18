//! iPhone Connection detail screen — read-only connection status. Reuses the
//! same `location.host` + loopback logic as the wide ConnectionSection
//! (connection form is decided by build, not toggled here — R4 pure I/O).

use crate::i18n::t;
use crate::platform::phone::shell::PhoneShell;
use leptos::prelude::*;

/// The host this Panel is served from. `pub(crate)` because the Settings
/// landing shows the same value in its Connection row — one source, so the row
/// can never drift from the detail screen it drills into.
pub(crate) fn current_host() -> String {
    web_sys::window()
        .and_then(|w| w.location().host().ok())
        .unwrap_or_default()
}
fn host_only(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.split(':').next().unwrap_or(host)
}
pub(crate) fn is_loopback_host(host: &str) -> bool {
    let h = host_only(host);
    h.eq_ignore_ascii_case("localhost") || h == "::1" || h.starts_with("127.")
}

#[component]
#[must_use]
pub fn PhoneConnection() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let host = current_host();
    let host_present = !host.is_empty();
    let remote = host_present && !is_loopback_host(&host);
    let badge = if remote { "remote" } else { "local" };
    let badge_style = if remote {
        "margin-left:auto; font-size:12px; padding:2px 8px; border-radius:9999px; background:color-mix(in oklch, var(--color-warning) 15%, transparent); color:var(--color-warning); flex:none;"
    } else {
        "margin-left:auto; font-size:12px; padding:2px 8px; border-radius:9999px; background:color-mix(in oklch, var(--color-success) 15%, transparent); color:var(--color-success); flex:none;"
    };
    view! {
        <PhoneShell title="Connection" back="/settings">
            <div>
                <div class="list-header">{t!(i18n, settings.phone.connection)}</div>
                <div class="list">
                    <div class="cell">
                        <div class="cell-body"><div class="cell-title">"Core"</div></div>
                        {if host_present {
                            view! { <span class="cell-value mono" style="font-size:13px;">{host.clone()}</span> }.into_any()
                        } else {
                            view! { <span class="cell-value">"—"</span> }.into_any()
                        }}
                        <span style=badge_style>{badge}</span>
                    </div>
                </div>
            </div>
        </PhoneShell>
    }
}
