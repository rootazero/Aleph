//! Host login-item settings, never the remote server service's autostart.
//! NSH exposes a typed read-only status until its setter is validated. Tauri
//! retains its existing plugin path and remote-origin ACL behavior.

use crate::api::native_credentials::nsh_shell_present;
use aleph_protocol::panel_shell::AutostartStatus;

use crate::i18n::{t, use_i18n};
use crate::platform::wide::views::voice::audio::is_native_shell;
use js_sys::{Function, Promise, Reflect};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Resolve `window.__TAURI__.core.invoke` and call it with `(cmd, args)`.
/// Returns the resolved JS value, or a string error if Tauri IPC is unavailable
/// (remote origin / plain browser) or the command rejected.
async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("no window")?;
    let tauri = Reflect::get(&window, &JsValue::from_str("__TAURI__"))
        .map_err(|_| "no __TAURI__".to_string())?;
    if tauri.is_undefined() || tauri.is_null() {
        return Err("Tauri IPC unavailable".to_string());
    }
    let core =
        Reflect::get(&tauri, &JsValue::from_str("core")).map_err(|_| "no core".to_string())?;
    let invoke_fn =
        Reflect::get(&core, &JsValue::from_str("invoke")).map_err(|_| "no invoke".to_string())?;
    let invoke_fn: Function = invoke_fn
        .dyn_into()
        .map_err(|_| "invoke is not a function".to_string())?;
    let ret = invoke_fn
        .call2(&core, &JsValue::from_str(cmd), &args)
        .map_err(|e| format!("invoke threw: {e:?}"))?;
    let promise: Promise = ret
        .dyn_into()
        .map_err(|_| "invoke did not return a Promise".to_string())?;
    JsFuture::from(promise).await.map_err(|e| format!("{e:?}"))
}

/// Read launch-at-login state via the autostart plugin's `is_enabled` command.
async fn get_autostart() -> Result<bool, String> {
    invoke("plugin:autostart|is_enabled", JsValue::NULL)
        .await?
        .as_bool()
        .ok_or_else(|| "is_enabled did not return a bool".to_string())
}

/// Enable or disable launch-at-login via the autostart plugin. The plugin's
/// `enable` / `disable` commands are idempotent and take no arguments.
async fn set_autostart(enabled: bool) -> Result<(), String> {
    let cmd = if enabled {
        "plugin:autostart|enable"
    } else {
        "plugin:autostart|disable"
    };
    invoke(cmd, JsValue::NULL).await.map(|_| ())
}

async fn nsh_status() -> Result<AutostartStatus, ()> {
    let window = web_sys::window().ok_or(())?;
    let bridge = Reflect::get(&window, &JsValue::from_str("__alephShellBridge")).map_err(|_| ())?;
    let query: Function = Reflect::get(&bridge, &JsValue::from_str("autostartStatus"))
        .map_err(|_| ())?
        .dyn_into()
        .map_err(|_| ())?;
    let promise: Promise = query
        .call0(&bridge)
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

/// Launch-at-login status for the current host. NSH never renders an unchecked
/// toggle for Unavailable, Failed or RequiresApproval; writes remain Tauri-only.
#[component]
#[must_use]
pub fn DesktopAutostartSection() -> impl IntoView {
    let i18n = use_i18n();
    // `available`: probe resolved AND we are in the shell. `enabled`: current state.
    let (available, set_available) = signal(false);
    let (enabled, set_enabled) = signal(false);
    let (status, set_status) = signal(None);
    let nsh = nsh_shell_present();

    if nsh {
        set_available.set(true);
        spawn_local(async move {
            let value = nsh_status().await.unwrap_or(AutostartStatus::Failed {});
            let _ = set_status.try_set(Some(value));
        });
    } else if is_native_shell() {
        spawn_local(async move {
            if let Ok(state) = get_autostart().await {
                set_enabled.set(state);
                set_available.set(true);
            }
            // On error (remote origin / no IPC): stay unavailable → section hidden.
        });
    }

    let on_toggle = move |ev| {
        let want = event_target_checked(&ev);
        set_enabled.set(want); // optimistic
        spawn_local(async move {
            if set_autostart(want).await.is_err() {
                // Revert on failure so the checkbox never lies about OS state.
                if let Ok(actual) = get_autostart().await {
                    set_enabled.set(actual);
                }
            }
        });
    };

    move || {
        available.get().then(|| {
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
                        {if nsh {
                            view! {
                                <div class="text-sm text-text-secondary" role="status" aria-live="polite" data-testid="desktop-autostart-status">
                                    <p>{move || match status.get() {
                                        None => t!(i18n, settings.general.autostart.status_loading).into_any(),
                                        Some(AutostartStatus::Enabled {}) => t!(i18n, settings.general.autostart.status_enabled).into_any(),
                                        Some(AutostartStatus::Disabled {}) => t!(i18n, settings.general.autostart.status_disabled).into_any(),
                                        Some(AutostartStatus::RequiresApproval {}) => t!(i18n, settings.general.autostart.status_approval).into_any(),
                                        Some(AutostartStatus::Unavailable { .. }) => t!(i18n, settings.general.autostart.status_unavailable).into_any(),
                                        Some(AutostartStatus::Failed {}) => t!(i18n, settings.general.autostart.status_failed).into_any(),
                                    }}</p>
                                    <p>{t!(i18n, settings.general.autostart.read_only)}</p>
                                </div>
                            }.into_any()
                        } else {
                        view! {
                        <label class="relative inline-flex items-center cursor-pointer shrink-0">
                            <input
                                type="checkbox"
                                class="sr-only peer"
                                prop:checked=move || enabled.get()
                                on:change=on_toggle
                            />
                            <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                        </label>
                        }.into_any()
                        }}
                    </div>
                </div>
            }
        })
    }
}
