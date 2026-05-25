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
mod deeplink;
mod hotkey;
#[cfg(target_os = "macos")]
mod menu;
mod notify;
mod tray;
mod update;

use std::time::Duration;

use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_window_state::{StateFlags, WindowExt};

/// The local daemon's Panel origin. The webview navigates here once the
/// daemon reports ready; until then it shows the bundled splash.
const PANEL_URL: &str = "http://127.0.0.1:18790";

/// How often the daemon health supervisor probes `/ready`.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Consecutive failed probes before the daemon is declared down. At the
/// poll interval above this is ~15s of sustained silence — long enough to
/// ride out a brief stall, short enough to recover quickly.
const FAILURES_TO_DECLARE_DOWN: u32 = 3;

/// Injected into every document the webview loads (splash and Panel).
/// Marks the page as shell-hosted, and on macOS records the platform so
/// the Panel's CSS can adapt — leave room for the overlay traffic
/// lights, let its translucent theme show the vibrancy material through.
///
/// The `alephShell.pickDirectory` / `alephShell.createProjectDirectory`
/// bridge that used to live here was removed: the directory picker now
/// browses the *server's* filesystem via JSON-RPC (`fs.*`), which is the
/// only correct semantics for the remote/Tailnet case (R6 — one core,
/// many channels). The Tauri `pick_project_directory` /
/// `create_project_directory` commands + `tauri-plugin-dialog` dep are
/// gone along with it.
#[cfg(target_os = "macos")]
const SHELL_MARKER_JS: &str = "var e=document.documentElement;\
    e.setAttribute('data-shell','aleph-tauri');\
    e.setAttribute('data-platform','macos');";
#[cfg(not(target_os = "macos"))]
const SHELL_MARKER_JS: &str = "document.documentElement.setAttribute('data-shell','aleph-tauri');";

/// The window-geometry facets the shell persists across restarts. Visibility
/// stays out of it — the shell drives that itself (hidden until the daemon
/// is ready, hidden again when the user closes to the tray).
fn window_state_flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION
}

/// Load the device token from the OS keychain. Returns None on any error
/// (missing entry, locked keychain, platform-specific failures) so the
/// shell can still boot — it will simply prompt for pairing again.
fn load_pairing_token() -> Option<String> {
    let entry = keyring::Entry::new("aleph-gateway", "desktop-shell").ok()?;
    match entry.get_password() {
        Ok(t) if !t.is_empty() => Some(t),
        _ => None,
    }
}

fn main() {
    // Inject the device token from the OS keychain before any subsystems
    // boot. The notify.rs subsystem reads ALEPH_GATEWAY_TOKEN during
    // connect_request — this must be set before Tauri/Tokio threads start.
    // The guard ensures an explicit env override is never clobbered.
    if std::env::var_os("ALEPH_GATEWAY_TOKEN").is_none() {
        if let Some(token) = load_pairing_token() {
            std::env::set_var("ALEPH_GATEWAY_TOKEN", token);
        }
    }

    init_tracing();

    let builder = tauri::Builder::default()
        // Single-instance must be registered first: a second launch focuses
        // the running shell instead of spawning a duplicate window. Its
        // `deep-link` feature also routes a second-launch `aleph://` link to
        // the running shell on Windows and Linux.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            focus_window(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        // Remember the window's size and position across restarts.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(window_state_flags())
                .skip_initial_state("main")
                .build(),
        )
        // Background auto-update, the `aleph://` scheme, and the global
        // summon hotkey — all pure OS integration.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // No `invoke_handler` — every panel-facing operation now goes
        // through the gateway's JSON-RPC (see `fs.*` for directory
        // browsing, `projects.*` for the catalogue). The shell stays
        // pure UI: window, tray, updater, hotkey.
        // Shared update state: the background checker, the tray, and the
        // macOS menu all read it.
        .manage(update::Updater::default());

    // macOS shows an app menu in the system menu bar regardless of window
    // chrome; give it shell-aware items. Windows and Linux stay chromeless.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(menu::build)
        .on_menu_event(|app, event| menu::on_event(app, event.id().as_ref()));

    builder
        .setup(|app| {
            let handle = app.handle().clone();

            build_main_window(&handle)?;

            // System tray — the resident, always-available face of the app.
            tray::build(&handle)?;

            // A resident assistant should come back after a reboot; enable
            // autostart once, then never fight the user's later choice.
            ensure_autostart(&handle);

            // Global summon hotkey and the `aleph://` deep-link scheme.
            hotkey::setup(&handle);
            deeplink::setup(&handle);

            #[cfg(target_os = "macos")]
            apply_macos_vibrancy(&handle);

            // Background worker: bring the daemon up, reveal the Panel, then
            // keep the notification bridge, daemon supervisor, and update
            // checker alive. It runs on its own Tokio runtime so the shell
            // never depends on Tauri runtime internals.
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
            // explicit "Quit" calls `app.exit(code)` and is allowed.
            if let RunEvent::ExitRequested { code, api, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

/// Create the single main window, hosting the splash until the daemon is up.
fn build_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
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
    let window = builder.build()?;

    // Restore the window's last size and position. On first run there is
    // nothing saved, so it keeps the centered default set above.
    if let Err(e) = window.restore_state(window_state_flags()) {
        tracing::warn!("could not restore window geometry: {e}");
    }
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
                // First launch / post-update: force any stale daemon offline
                // so the `aleph-server` bundled in this app takes over.
                let version = handle.package_info().version.to_string();
                daemon::reconcile_for_version(&version).await;

                let daemon_up = match daemon::ensure_ready().await {
                    Ok(()) => {
                        reveal_panel(&handle);
                        true
                    }
                    Err(e) => {
                        tracing::error!("daemon did not become ready: {e}");
                        show_daemon_error(&handle, &e);
                        false
                    }
                };

                // Three resident background tasks for the lifetime of the
                // shell: the OS-notification bridge, the daemon health
                // supervisor, and the auto-update checker. None returns;
                // run them together.
                tokio::join!(
                    notify::run_notification_bridge(handle.clone()),
                    supervise_daemon(handle.clone(), daemon_up),
                    update::run_update_checker(handle),
                );
            });
        });
    if let Err(e) = spawned {
        tracing::error!("failed to spawn background thread: {e}");
    }
}

/// Point the main window's webview at the live Panel, without disturbing the
/// window's visibility — used for the first reveal and for a silent reload
/// after the daemon recovers. Pass `token` only on the first reveal: the
/// Panel stashes it in localStorage and the webview keeps that storage
/// across in-window navigations, so subsequent reloads do not need it.
fn navigate_to_panel(handle: &tauri::AppHandle, token: Option<&str>) {
    let Some(window) = handle.get_webview_window("main") else {
        tracing::error!("main window missing — cannot reach the Panel");
        return;
    };
    match daemon::build_panel_url(token) {
        Ok(url) => {
            if let Err(e) = window.navigate(url) {
                tracing::error!("failed to navigate to the Panel: {e}");
            }
        }
        Err(e) => tracing::error!("invalid Panel URL: {e}"),
    }
}

/// Bring the main window forward: show it, un-minimise it, focus it. Shared
/// by the tray, the single-instance handler, and the first Panel reveal.
pub(crate) fn focus_window(handle: &tauri::AppHandle) {
    let Some(window) = handle.get_webview_window("main") else {
        return;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// Reveal the Panel for the first time: navigate to it and bring the window
/// forward. The window starts hidden, so this is what the user sees on a
/// normal launch.
fn reveal_panel(handle: &tauri::AppHandle) {
    let token = daemon::load_bootstrap_token();
    navigate_to_panel(handle, token.as_deref());
    focus_window(handle);
}

/// Surface a daemon-startup failure on the splash screen.
fn show_daemon_error(handle: &tauri::AppHandle, message: &str) {
    let Some(window) = handle.get_webview_window("main") else {
        return;
    };
    let _ = window.show();
    let safe = message.replace('\\', "\\\\").replace('\'', "\\'");
    let _ = window.eval(format!(
        "window.__alephError && window.__alephError('{safe}')"
    ));
}

/// Whether the daemon is currently believed to be serving the Panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonHealth {
    Up,
    Down,
}

/// What the supervisor must do after folding in one probe result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupervisorAction {
    /// The daemon's state is unchanged — do nothing.
    Idle,
    /// The daemon is unreachable; try to bring it back.
    Relaunch,
    /// The daemon just came back; reload the Panel webview.
    ReloadPanel,
}

/// A small state machine that turns a stream of `/ready` probe results into
/// daemon-lifecycle actions. Deliberately free of I/O so it can be
/// unit-tested without a running daemon.
struct Supervisor {
    health: DaemonHealth,
    consecutive_failures: u32,
}

impl Supervisor {
    /// Start supervising. `daemon_up` is the outcome of the initial boot:
    /// a failed boot starts the supervisor in `Down` so it keeps retrying.
    fn new(daemon_up: bool) -> Self {
        Self {
            health: if daemon_up {
                DaemonHealth::Up
            } else {
                DaemonHealth::Down
            },
            consecutive_failures: 0,
        }
    }

    /// Fold one probe result into the state machine and report the action
    /// the caller must take.
    fn tick(&mut self, ready: bool) -> SupervisorAction {
        match (self.health, ready) {
            (DaemonHealth::Up, true) => {
                self.consecutive_failures = 0;
                SupervisorAction::Idle
            }
            (DaemonHealth::Up, false) => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= FAILURES_TO_DECLARE_DOWN {
                    self.health = DaemonHealth::Down;
                    SupervisorAction::Relaunch
                } else {
                    SupervisorAction::Idle
                }
            }
            (DaemonHealth::Down, false) => SupervisorAction::Relaunch,
            (DaemonHealth::Down, true) => {
                self.health = DaemonHealth::Up;
                self.consecutive_failures = 0;
                SupervisorAction::ReloadPanel
            }
        }
    }
}

/// Keep the daemon alive for the lifetime of the shell. After the initial
/// boot this is the only thing standing between a crashed daemon and a dead
/// Panel: it relaunches the daemon when it disappears and reloads the Panel
/// once it is back. Silent by design — it never shows or focuses the window
/// (R5), it just keeps the plumbing connected.
async fn supervise_daemon(handle: tauri::AppHandle, daemon_up: bool) {
    let mut supervisor = Supervisor::new(daemon_up);
    loop {
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
        match supervisor.tick(daemon::is_ready().await) {
            SupervisorAction::Idle => {}
            SupervisorAction::Relaunch => {
                tracing::warn!("daemon unreachable — attempting relaunch");
                daemon::relaunch_if_down().await;
            }
            SupervisorAction::ReloadPanel => {
                tracing::info!("daemon recovered — reloading the Panel");
                // token only on first reveal — Panel keeps localStorage thereafter
                navigate_to_panel(&handle, None);
            }
        }
    }
}

/// Enable launch-at-login on first run only, leaving later user choices
/// (toggling it off in OS settings) untouched.
fn ensure_autostart(app: &tauri::AppHandle) {
    use tauri_plugin_autostart::ManagerExt;

    let Some(marker) = dirs::home_dir().map(|h| h.join(".aleph/.desktop-shell-autostart")) else {
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

    let filter =
        EnvFilter::try_from_env("ALEPH_SHELL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervisor_stays_idle_while_the_daemon_is_healthy() {
        let mut sup = Supervisor::new(true);
        assert_eq!(sup.tick(true), SupervisorAction::Idle);
        assert_eq!(sup.tick(true), SupervisorAction::Idle);
        assert_eq!(sup.health, DaemonHealth::Up);
    }

    #[test]
    fn supervisor_tolerates_a_brief_stall() {
        let mut sup = Supervisor::new(true);
        // A blip short of the threshold must not declare the daemon down.
        for _ in 0..FAILURES_TO_DECLARE_DOWN - 1 {
            assert_eq!(sup.tick(false), SupervisorAction::Idle);
            assert_eq!(sup.health, DaemonHealth::Up);
        }
        // A recovery before the threshold resets the failure counter.
        assert_eq!(sup.tick(true), SupervisorAction::Idle);
        assert_eq!(sup.consecutive_failures, 0);
    }

    #[test]
    fn supervisor_declares_down_after_sustained_failure() {
        let mut sup = Supervisor::new(true);
        let mut action = SupervisorAction::Idle;
        for _ in 0..FAILURES_TO_DECLARE_DOWN {
            action = sup.tick(false);
        }
        assert_eq!(action, SupervisorAction::Relaunch);
        assert_eq!(sup.health, DaemonHealth::Down);
    }

    #[test]
    fn supervisor_keeps_relaunching_while_down() {
        // A failed boot starts the supervisor already down.
        let mut sup = Supervisor::new(false);
        assert_eq!(sup.tick(false), SupervisorAction::Relaunch);
        assert_eq!(sup.tick(false), SupervisorAction::Relaunch);
    }

    #[test]
    fn supervisor_reloads_the_panel_on_recovery() {
        let mut sup = Supervisor::new(false);
        assert_eq!(sup.tick(false), SupervisorAction::Relaunch);
        assert_eq!(sup.tick(true), SupervisorAction::ReloadPanel);
        assert_eq!(sup.health, DaemonHealth::Up);
        // Back to steady state once recovered.
        assert_eq!(sup.tick(true), SupervisorAction::Idle);
    }
}
