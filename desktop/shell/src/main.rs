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

mod connection;
mod daemon;
mod deeplink;
mod external_link;
mod hotkey;
#[cfg(target_os = "macos")]
mod menu;
mod notify;
mod perm_monitor;
mod tray;
mod update;
mod webview_perms;

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

fn main() {
    // Inject the auto-provisioned shared token before any subsystems boot.
    // The notify.rs subsystem reads ALEPH_GATEWAY_TOKEN during connect_request
    // — this must be set before Tauri/Tokio threads start. The token is read
    // daemon-free from ~/.aleph/data/security.db via the bundled `aleph-server
    // bootstrap-token` subcommand: no OS keychain, so no Keychain UI prompt on
    // app updates. The guard ensures an explicit env override is never
    // clobbered.
    if std::env::var_os("ALEPH_GATEWAY_TOKEN").is_none() {
        if let Some(token) = daemon::load_shared_token() {
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
        // The only `invoke_handler` the shell exposes: connection-config
        // commands (local vs remote Gateway target). These are I/O config
        // toggles, not business or UI logic — the R2/R4 boundary holds
        // (spec §5.2 explicit exception). Every *panel-facing* operation
        // still goes through the gateway's JSON-RPC (`fs.*` for directory
        // browsing, `projects.*` for the catalogue); the shell stays pure
        // I/O: window, tray, updater, hotkey, connection target.
        .invoke_handler(tauri::generate_handler![
            connection::get_connection_target,
            connection::set_connection_target,
            connection::clear_connection_target,
        ])
        // Shared update state: the background checker, the tray, and the
        // macOS menu all read it.
        .manage(update::Updater::default());

    // macOS shows an app menu in the system menu bar regardless of window
    // chrome; give it shell-aware items. Windows and Linux stay chromeless.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(menu::build)
        .on_menu_event(|app, event| menu::on_event(app, event.id().as_ref()));

    let app = match builder
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

            // Watch for TCC permission grants that require a daemon restart
            // (input_monitoring, screen_recording). When the user grants one
            // in System Settings and returns to the app, the daemon is
            // restarted automatically — no manual "restart aleph-server" step.
            perm_monitor::start_monitor(handle.clone());

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
    {
        Ok(app) => app,
        Err(e) => {
            tracing::error!("failed to build the Aleph desktop shell: {e}");
            std::process::exit(1);
        }
    };

    app.run(|_app_handle, event| match event {
        // A window-close-driven exit is vetoed (stay in the tray); an
        // explicit "Quit" calls `app.exit(code)` and is allowed.
        RunEvent::ExitRequested { code, api, .. } => {
            if code.is_none() {
                api.prevent_exit();
            }
        }
        // macOS only: closing the window hides it (see `on_window_event`)
        // rather than destroying it, so the app stays in the dock. Clicking
        // the dock icon then fires `Reopen` with no visible windows — bring
        // the hidden window back, matching native dock behaviour so users
        // don't have to reach for the menu's "Show Aleph". Windows/Linux
        // re-entry is the tray icon and single-instance relaunch.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            focus_window(_app_handle);
        }
        _ => {}
    });
}

/// Create the single main window, hosting the splash until the daemon is up.
fn build_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    // `mut` is only needed on macOS, where the title-bar overlay is applied
    // via reassignment below; other platforms never rebind `builder`.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Aleph")
        .inner_size(1180.0, 800.0)
        .min_inner_size(720.0, 520.0)
        .center()
        .resizable(true)
        .transparent(true)
        .visible(false)
        .initialization_script(SHELL_MARKER_JS)
        // Turn `target="_blank"` clicks into a top-level navigation so the
        // guard below can externalise them; pin the lone webview to the
        // Panel origin and hand outside URLs to the OS browser (R5).
        .initialization_script(external_link::CLICK_INTERCEPTOR_JS)
        .on_navigation(external_link::route);

    // Overlay traffic lights over the content for a native-mac feel; the
    // Panel's CSS (Phase 2) leaves room for them.
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }
    let window = builder.build()?;

    // Grant the Panel webview microphone access (voice-input button). No-op on
    // macOS where wry auto-grants; installs handlers on Windows/Linux.
    webview_perms::grant_microphone(&window);

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
                let version = handle.package_info().version.to_string();
                let target = connection::load_target();

                // Bring the chosen target online. Local owns + supervises the
                // bundled daemon (today's behaviour, kept byte-for-byte);
                // Remote never touches a local daemon — we only probe TCP
                // reachability and navigate the webview at the remote origin.
                let up = match &target {
                    connection::ConnectionTarget::Local => {
                        // Explicitly clear the remote allow-list (idempotent):
                        // Local navigations only ever touch loopback.
                        external_link::set_remote_host(None);
                        // First launch / post-update: force any stale daemon
                        // offline so the `aleph-server` bundled in this app
                        // takes over.
                        daemon::reconcile_for_version(&version).await;
                        match daemon::ensure_ready().await {
                            Ok(()) => {
                                reveal_panel(&handle);
                                true
                            }
                            Err(e) => {
                                tracing::error!("daemon did not become ready: {e}");
                                show_daemon_error(&handle, &e);
                                false
                            }
                        }
                    }
                    connection::ConnectionTarget::Remote(url) => {
                        // Allow the remote origin in the link guard so in-Panel
                        // navigations stay in the webview instead of escaping
                        // to the OS browser.
                        external_link::set_remote_host(Some(url.clone()));
                        let host = url.host_str().unwrap_or("").to_string();
                        let port = url.port_or_known_default().unwrap_or(18790);
                        let reachable = daemon::tcp_reachable(&host, port).await;
                        if reachable {
                            // Navigate the webview to the remote root; an
                            // unauthenticated client is redirected to `/pair`.
                            if let Some(window) = handle.get_webview_window("main") {
                                let _ = window.navigate(url.clone());
                            }
                            focus_window(&handle);
                        } else {
                            // Spec §5.5: a remote that is unreachable at startup
                            // must land on the bundled connection page (retry /
                            // back-to-local), never on the dead remote origin —
                            // otherwise the user stares at the webview's native
                            // "can't connect" page until the supervisor's first
                            // tick (one HEALTH_POLL_INTERVAL later) corrects it.
                            tracing::warn!(
                                "remote Gateway {host}:{port} not reachable at startup — showing connection page"
                            );
                            show_connection_page(
                                &handle,
                                "Remote Gateway unreachable. Retry or go back to local.",
                            );
                            focus_window(&handle);
                        }
                        reachable
                    }
                };

                // Three resident background tasks for the lifetime of the
                // shell: the OS-notification bridge, the daemon/Gateway health
                // supervisor, and the auto-update checker. None returns;
                // run them together.
                tokio::join!(
                    notify::run_notification_bridge(handle.clone()),
                    supervise_daemon(handle.clone(), up),
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
/// after the daemon recovers.
///
/// `bootstrap_url`: when present (Phase 2 default), the daemon-issued
/// `/auth/bootstrap?nonce=…` URL — the daemon validates the nonce and
/// sets the session cookie, the Panel never sees a token in the URL.
/// Pass `None` for in-window reloads (the Panel already holds a session
/// cookie) and for the rare cold path where nonce-issue fails — the
/// gateway redirects unauthenticated browsers to `/pair`.
fn navigate_to_panel(handle: &tauri::AppHandle, bootstrap_url: Option<&str>) {
    let Some(window) = handle.get_webview_window("main") else {
        tracing::error!("main window missing — cannot reach the Panel");
        return;
    };
    match daemon::build_panel_url(bootstrap_url) {
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

/// Re-route the shell for a freshly-chosen target: update the link allow-list,
/// then navigate + (re)start supervision. Called by the connection commands
/// (`set_connection_target` / `clear_connection_target`) after the new target
/// has been persisted. The resident `supervise_daemon` loop picks up the
/// persisted switch on its next tick and re-arms in the matching mode.
pub(crate) fn reroute_for_target(app: &tauri::AppHandle, target: connection::ConnectionTarget) {
    match &target {
        connection::ConnectionTarget::Remote(url) => {
            external_link::set_remote_host(Some(url.clone()));
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.navigate(url.clone());
            }
        }
        connection::ConnectionTarget::Local => {
            external_link::set_remote_host(None);
            // bring the local daemon up and reveal the Panel
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let version = handle.package_info().version.to_string();
                daemon::reconcile_for_version(&version).await;
                let _ = daemon::ensure_ready().await;
                reveal_panel(&handle);
            });
        }
    }
}

/// Reveal the Panel for the first time: navigate to it and bring the window
/// forward. The window starts hidden, so this is what the user sees on a
/// normal launch.
///
/// Ask the daemon for a one-shot bootstrap URL (Phase 2 default — no token
/// in the URL bar; the daemon validates the nonce on loopback and sets the
/// session cookie). If that fails (binary missing, RPC unreachable), fall
/// through to the plain Panel URL — the gateway's session middleware will
/// redirect unauthenticated browsers to `/pair` so the user can complete
/// pairing manually. Phase 4 removed the legacy `?token=` URL fallback.
fn reveal_panel(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let bootstrap_url = tokio::task::spawn_blocking(daemon::load_bootstrap_url)
            .await
            .ok()
            .flatten();
        navigate_to_panel(&handle, bootstrap_url.as_deref());
        focus_window(&handle);
    });
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
    /// Remote target unreachable; we don't own that daemon — surface a
    /// connection error and offer retry / back-to-local instead of relaunch.
    ShowConnectionError,
}

/// A small state machine that turns a stream of `/ready` probe results into
/// daemon-lifecycle actions. Deliberately free of I/O so it can be
/// unit-tested without a running daemon.
struct Supervisor {
    health: DaemonHealth,
    consecutive_failures: u32,
    /// Whether this supervisor is tracking a remote Gateway (true) or the
    /// local daemon (false). Remote mode never relaunches — it surfaces a
    /// connection error instead, because the remote daemon is not ours to manage.
    remote: bool,
}

impl Supervisor {
    /// Start supervising. `daemon_up` is the outcome of the initial boot:
    /// a failed boot starts the supervisor in `Down` so it keeps retrying.
    const fn new(daemon_up: bool) -> Self {
        Self {
            health: if daemon_up {
                DaemonHealth::Up
            } else {
                DaemonHealth::Down
            },
            consecutive_failures: 0,
            remote: false,
        }
    }

    /// Start supervising a remote Gateway. `reachable` is the outcome of the
    /// initial TCP probe. Unlike Local mode, a failed probe does not relaunch
    /// the daemon — it surfaces a connection error instead.
    const fn new_remote(reachable: bool) -> Self {
        Self {
            health: if reachable {
                DaemonHealth::Up
            } else {
                DaemonHealth::Down
            },
            consecutive_failures: 0,
            remote: true,
        }
    }

    /// The action to take when the target transitions to Down. Local mode
    /// relaunches; Remote mode surfaces a connection error instead (the remote
    /// daemon is not ours to manage).
    const fn down_action(&self) -> SupervisorAction {
        if self.remote {
            SupervisorAction::ShowConnectionError
        } else {
            SupervisorAction::Relaunch
        }
    }

    /// Fold one probe result into the state machine and report the action
    /// the caller must take.
    const fn tick(&mut self, ready: bool) -> SupervisorAction {
        match (self.health, ready) {
            (DaemonHealth::Up, true) => {
                self.consecutive_failures = 0;
                SupervisorAction::Idle
            }
            (DaemonHealth::Up, false) => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= FAILURES_TO_DECLARE_DOWN {
                    self.health = DaemonHealth::Down;
                    self.down_action()
                } else {
                    SupervisorAction::Idle
                }
            }
            (DaemonHealth::Down, false) => self.down_action(),
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
async fn supervise_daemon(handle: tauri::AppHandle, up: bool) {
    // The supervisor adapts to a target switch made at runtime (via the
    // connection commands / tray / menu): each tick re-reads the persisted
    // target and, when it differs from the one currently supervised, rebuilds
    // the state machine in the matching mode. This keeps a single resident
    // supervisor — no second loop is ever spawned.
    let mut target = connection::load_target();
    let mut supervisor = match &target {
        connection::ConnectionTarget::Local => Supervisor::new(up),
        connection::ConnectionTarget::Remote(_) => Supervisor::new_remote(up),
    };
    loop {
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;

        // Re-read the target; a switch resets the supervisor to the new mode.
        let current = connection::load_target();
        if current != target {
            tracing::info!("connection target changed — re-arming supervisor");
            supervisor = match &current {
                connection::ConnectionTarget::Local => Supervisor::new(false),
                connection::ConnectionTarget::Remote(_) => Supervisor::new_remote(false),
            };
            target = current;
        }

        // Probe the live target: loopback `/ready` for Local, bare TCP for Remote.
        let ready = match &target {
            connection::ConnectionTarget::Local => daemon::is_ready().await,
            connection::ConnectionTarget::Remote(url) => {
                let host = url.host_str().unwrap_or("");
                let port = url.port_or_known_default().unwrap_or(18790);
                daemon::tcp_reachable(host, port).await
            }
        };

        match supervisor.tick(ready) {
            SupervisorAction::Idle => {}
            SupervisorAction::Relaunch => {
                tracing::warn!("daemon unreachable — attempting relaunch");
                daemon::relaunch_if_down().await;
            }
            SupervisorAction::ReloadPanel => {
                tracing::info!("Gateway recovered — reloading the Panel");
                match &target {
                    // No bootstrap nonce on in-window reloads — the Panel has
                    // already obtained an `aleph_session` cookie on the first
                    // reveal.
                    connection::ConnectionTarget::Local => navigate_to_panel(&handle, None),
                    // Remote recovery: re-point the webview at the remote root.
                    connection::ConnectionTarget::Remote(url) => {
                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.navigate(url.clone());
                        }
                    }
                }
            }
            SupervisorAction::ShowConnectionError => {
                tracing::warn!("remote Gateway unreachable — showing connection page");
                show_connection_page(
                    &handle,
                    "Remote Gateway unreachable. Retry or go back to local.",
                );
            }
        }
    }
}

/// Surface a remote-Gateway connection failure by navigating the main window
/// to the bundled connection page and forwarding the message to its
/// `window.__alephError` hook. Mirrors `show_daemon_error`, but targets the
/// connect page (where the user can retry or fall back to Local) rather than
/// the splash.
fn show_connection_page(handle: &tauri::AppHandle, message: &str) {
    let Some(window) = handle.get_webview_window("main") else {
        return;
    };
    let _ = window.show();
    if let Ok(url) = tauri::Url::parse("tauri://localhost/connect.html") {
        if let Err(e) = window.navigate(url) {
            tracing::warn!("could not navigate to the connection page: {e}");
        }
    }
    let safe = message.replace('\\', "\\\\").replace('\'', "\\'");
    let _ = window.eval(format!(
        "window.__alephError && window.__alephError('{safe}')"
    ));
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

    #[test]
    fn supervisor_remote_shows_error_instead_of_relaunch() {
        let mut sup = Supervisor::new_remote(true);
        // sustained failure on a remote target must NOT try to relaunch a
        // daemon we don't own — it surfaces a connection error instead.
        let mut action = SupervisorAction::Idle;
        for _ in 0..FAILURES_TO_DECLARE_DOWN {
            action = sup.tick(false);
        }
        assert_eq!(action, SupervisorAction::ShowConnectionError);
        assert_eq!(sup.health, DaemonHealth::Down);
    }

    #[test]
    fn supervisor_remote_reloads_on_recovery() {
        let mut sup = Supervisor::new_remote(false);
        assert_eq!(sup.tick(false), SupervisorAction::ShowConnectionError);
        assert_eq!(sup.tick(true), SupervisorAction::ReloadPanel);
    }

    #[test]
    fn supervisor_local_behaviour_unchanged() {
        // regression guard: local mode still relaunches
        let mut sup = Supervisor::new(true); // existing ctor = Local
        let mut action = SupervisorAction::Idle;
        for _ in 0..FAILURES_TO_DECLARE_DOWN {
            action = sup.tick(false);
        }
        assert_eq!(action, SupervisorAction::Relaunch);
    }
}
