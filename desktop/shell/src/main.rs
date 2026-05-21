//! Aleph desktop shell — a thin Tauri v2 native host for the Aleph Panel.
//!
//! Scope (and nothing beyond it): own a native window that hosts the Panel
//! webview, a system tray, the `aleph-server` daemon lifecycle, OS
//! notifications, and autostart. All business and UI logic lives in the
//! Panel (R2) and the daemon (R1/R3) — this crate is pure I/O and OS
//! integration, and must stay that way (R10/P6).
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod daemon;
mod notify;
mod tray;

use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// The local daemon's Panel origin. The webview navigates here once the
/// daemon reports ready; until then it shows the bundled splash.
const PANEL_URL: &str = "http://127.0.0.1:18790";

/// Injected into every document the webview loads (splash and Panel). It
/// marks the page as shell-hosted, and on macOS records the platform so the
/// Panel's CSS can adapt — leave room for the overlay traffic lights, let
/// its translucent theme show the vibrancy material through. The shell only
/// sets flags; it drives no behaviour.
#[cfg(target_os = "macos")]
const SHELL_MARKER_JS: &str = "var e=document.documentElement;\
    e.setAttribute('data-shell','aleph-tauri');\
    e.setAttribute('data-platform','macos');";
#[cfg(not(target_os = "macos"))]
const SHELL_MARKER_JS: &str =
    "document.documentElement.setAttribute('data-shell','aleph-tauri');";

fn main() {
    init_tracing();

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let handle = app.handle().clone();

            build_main_window(&handle)?;

            // System tray — the resident, always-available face of the app.
            tray::build(&handle)?;

            // A resident assistant should come back after a reboot; enable
            // autostart once, then never fight the user's later choice.
            ensure_autostart(&handle);

            #[cfg(target_os = "macos")]
            apply_macos_vibrancy(&handle);

            // Background worker: bring the daemon up, reveal the Panel, then
            // keep an OS-notification bridge alive. It runs on its own Tokio
            // runtime so the shell never depends on Tauri runtime internals.
            spawn_background(handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window must NOT kill the app or the daemon — the
            // shell lives on in the tray so the assistant stays reachable.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Aleph desktop shell")
        .run(|_app, event| {
            // A window-close-driven exit is vetoed (stay in the tray); an
            // explicit tray "Quit" calls `app.exit(code)` and is allowed.
            if let RunEvent::ExitRequested { code, api, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

/// Create the single main window, hosting the splash until the daemon is up.
fn build_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let mut builder = WebviewWindowBuilder::new(
        app,
        "main",
        WebviewUrl::App("index.html".into()),
    )
        .title("Aleph")
        .inner_size(1180.0, 800.0)
        .min_inner_size(720.0, 520.0)
        .center()
        .resizable(true)
        .transparent(true)
        .visible(false)
        .initialization_script(SHELL_MARKER_JS);

    // Overlay traffic lights over the content for a native-mac feel; the
    // Panel's CSS (Phase 2) leaves room for them.
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }
    builder.build()?;
    Ok(())
}

/// Spawn the background worker thread that owns the shell's async I/O.
fn spawn_background(handle: tauri::AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("aleph-shell-bg".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("failed to build background runtime: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                match daemon::ensure_ready().await {
                    Ok(()) => reveal_panel(&handle),
                    Err(e) => {
                        tracing::error!("daemon did not become ready: {e}");
                        show_daemon_error(&handle, &e);
                    }
                }
                // Best-effort OS-notification bridge; never blocks the shell.
                notify::run_notification_bridge(handle).await;
            });
        });
    if let Err(e) = spawned {
        tracing::error!("failed to spawn background thread: {e}");
    }
}

/// Point the main window's webview at the live Panel and bring it forward.
fn reveal_panel(handle: &tauri::AppHandle) {
    let Some(window) = handle.get_webview_window("main") else {
        tracing::error!("main window missing — cannot reveal the Panel");
        return;
    };
    match PANEL_URL.parse() {
        Ok(url) => {
            if let Err(e) = window.navigate(url) {
                tracing::error!("failed to navigate to the Panel: {e}");
            }
        }
        Err(e) => tracing::error!("invalid Panel URL: {e}"),
    }
    let _ = window.show();
    let _ = window.set_focus();
}

/// Surface a daemon-startup failure on the splash screen.
fn show_daemon_error(handle: &tauri::AppHandle, message: &str) {
    let Some(window) = handle.get_webview_window("main") else {
        return;
    };
    let _ = window.show();
    let safe = message.replace('\\', "\\\\").replace('\'', "\\'");
    let _ = window.eval(format!("window.__alephError && window.__alephError('{safe}')"));
}

/// Enable launch-at-login on first run only, leaving later user choices
/// (toggling it off in OS settings) untouched.
fn ensure_autostart(app: &tauri::AppHandle) {
    use tauri_plugin_autostart::ManagerExt;

    let Some(marker) = dirs::home_dir().map(|h| h.join(".aleph/.desktop-shell-autostart"))
    else {
        return;
    };
    if marker.exists() {
        return;
    }
    match app.autolaunch().enable() {
        Ok(()) => tracing::info!("autostart enabled (first run)"),
        Err(e) => tracing::warn!("could not enable autostart: {e}"),
    }
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, "1");
}

/// Apply the macOS vibrancy material behind the transparent webview so the
/// Panel's translucent theme (Phase 2) has a real material to sit on.
#[cfg(target_os = "macos")]
fn apply_macos_vibrancy(handle: &tauri::AppHandle) {
    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};

    let Some(window) = handle.get_webview_window("main") else {
        return;
    };
    if let Err(e) = apply_vibrancy(
        &window,
        NSVisualEffectMaterial::Sidebar,
        Some(NSVisualEffectState::Active),
        None,
    ) {
        tracing::warn!("macOS vibrancy unavailable: {e}");
    }
}

/// Initialise logging. Verbosity is controlled by `ALEPH_SHELL_LOG`
/// (e.g. `ALEPH_SHELL_LOG=debug`), defaulting to `info`.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("ALEPH_SHELL_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
