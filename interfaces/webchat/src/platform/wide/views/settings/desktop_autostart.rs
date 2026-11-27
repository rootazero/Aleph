//! Host login-item settings, never the remote server service's autostart.
//! The NSH host reads and writes the login item through the typed shell
//! bridge (`__alephShellBridge.autostartStatus` / `autostartSet`); outside
//! it the section is hidden.

use crate::api::native_credentials::nsh_shell_present;
use aleph_protocol::panel_shell::AutostartStatus;

use crate::i18n::{t, use_i18n};
use js_sys::{Array, Function, Promise, Reflect};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Call `__alephShellBridge[method](...args)`; both methods resolve with the
/// status the host read.
async fn bridge_status(method: &str, args: &Array) -> Result<AutostartStatus, ()> {
    let window = web_sys::window().ok_or(())?;
    let bridge = Reflect::get(&window, &JsValue::from_str("__alephShellBridge")).map_err(|_| ())?;
    let call: Function = Reflect::get(&bridge, &JsValue::from_str(method))
        .map_err(|_| ())?
        .dyn_into()
        .map_err(|_| ())?;
    let promise: Promise = call
        .apply(&bridge, args)
        .map_err(|_| ())?
        .dyn_into()
        .map_err(|_| ())?;
    let value = JsFuture::from(promise).await.map_err(|_| ())?;
    let raw = js_sys::JSON::stringify(&value)
        .map_err(|_| ())?
        .as_string()
        .ok_or(())?;
    serde_json::from_str(&raw).map_err(|_| ())
}

/// Launch-at-login for the current host. The toggle is enabled only for a
/// confirmed Enabled or Disabled; Unavailable, Failed and RequiresApproval
/// show their status with the toggle disabled, never as an unchecked switch.
#[component]
#[must_use]
pub fn DesktopAutostartSection() -> impl IntoView {
    let i18n = use_i18n();
    let available = nsh_shell_present();
    let (status, set_status) = signal(None);
    let (busy, set_busy) = signal(false);

    if available {
        spawn_local(async move {
            let value = bridge_status("autostartStatus", &Array::new())
                .await
                .unwrap_or(AutostartStatus::Failed {});
            let _ = set_status.try_set(Some(value));
        });
    }

    let confirmed = move || {
        matches!(
            status.get(),
            Some(AutostartStatus::Enabled {} | AutostartStatus::Disabled {})
        )
    };
    let on_toggle = move |ev| {
        let want = event_target_checked(&ev);
        set_busy.set(true);
        spawn_local(async move {
            // The reply is the status read back after the write, so the
            // switch never shows a state the system does not have.
            let value = bridge_status("autostartSet", &Array::of1(&JsValue::from_bool(want)))
                .await
                .unwrap_or(AutostartStatus::Failed {});
            let _ = set_status.try_set(Some(value));
            let _ = set_busy.try_set(false);
        });
    };

    move || {
        available.then(|| {
            view! {
                <div class="bg-surface-sunken/40 rounded-xl p-4" data-testid="desktop-autostart-section">
                    <div class="flex items-center justify-between gap-4">
                        <div>
                            <h2 class="text-xl font-semibold text-text-primary mb-2">
                                {t!(i18n, settings.general.autostart.title)}
                            </h2>
                            <p class="text-sm text-text-secondary">
                                {t!(i18n, settings.general.autostart.description)}
                            </p>
                        </div>
                        <div class="flex items-center gap-3 shrink-0">
                            <p class="text-sm text-text-secondary" role="status" aria-live="polite" data-testid="desktop-autostart-status">
                                {move || match status.get() {
                                    None => t!(i18n, settings.general.autostart.status_loading).into_any(),
                                    Some(AutostartStatus::Enabled {}) => t!(i18n, settings.general.autostart.status_enabled).into_any(),
                                    Some(AutostartStatus::Disabled {}) => t!(i18n, settings.general.autostart.status_disabled).into_any(),
                                    Some(AutostartStatus::RequiresApproval {}) => t!(i18n, settings.general.autostart.status_approval).into_any(),
                                    Some(AutostartStatus::Unavailable { .. }) => t!(i18n, settings.general.autostart.status_unavailable).into_any(),
                                    Some(AutostartStatus::Failed {}) => t!(i18n, settings.general.autostart.status_failed).into_any(),
                                }}
                            </p>
                            <label class="relative inline-flex items-center cursor-pointer has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-50">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    data-testid="desktop-autostart-toggle"
                                    prop:checked=move || status.get() == Some(AutostartStatus::Enabled {})
                                    prop:disabled=move || busy.get() || !confirmed()
                                    on:change=on_toggle
                                />
                                <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                            </label>
                        </div>
                    </div>
                </div>
            }
        })
    }
}
