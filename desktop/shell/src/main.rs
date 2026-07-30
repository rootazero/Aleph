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

mod cert_trust;
mod connection;
// First-run connection setup (address entry + mDNS discovery) is panel-only:
// the full app brings its bundled daemon up and never needs a connect-first
// step. The panel-only shell hosts no daemon, so on first launch it has no
// Gateway to point at and must ask (spec §8).
#[cfg(not(feature = "embedded-core"))]
mod connect_setup;
// Daemon lifecycle + the TCC permission monitor are full-app-only: the
// panel-only (`--no-default-features`) shell hosts no local `aleph-server`,
// so there is nothing to launch, supervise, or watch permissions for.
#[cfg(feature = "embedded-core")]
mod daemon;
mod deeplink;
mod external_link;
mod hotkey;
#[cfg(target_os = "macos")]
mod menu;
mod notify;
#[cfg(feature = "embedded-core")]
mod perm_monitor;
mod tray;
mod update;
mod webview_perms;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_window_state::{StateFlags, WindowExt};

/// The loopback Gateway origin. In the full app the bundled daemon serves it;
/// in both variants the `Local` connection target resolves here.
pub(crate) const PANEL_URL: &str = "http://127.0.0.1:18790";

/// How often the daemon health supervisor probes `/ready`. Full-app-only.
#[cfg(feature = "embedded-core")]
const HEALTH_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
/// Consecutive failed probes before the daemon is declared down. At the
/// poll interval above this is ~15s of sustained silence — long enough to
/// ride out a brief stall, short enough to recover quickly.
/// Shared with the pure `Supervisor` state machine, which is always compiled.
const FAILURES_TO_DECLARE_DOWN: u32 = 3;

/// Poll interval for the lite-shell remote-health supervisor. Deliberately
/// chosen so `LITE_REMOTE_POLL × LITE_FAILURES_TO_RELOCATE ≥ 35s`, giving
/// the panel's own ~31s WebSocket reconnect budget a chance to recover in-panel
/// before the native layer yanks the webview to the connect page.
#[cfg(not(feature = "embedded-core"))]
const LITE_REMOTE_POLL: std::time::Duration = std::time::Duration::from_secs(5);
/// Consecutive failed probes before the lite supervisor relocates to the
/// connect page. 5s × 8 = 40s — safely past the ~31s panel reconnect budget.
#[cfg(not(feature = "embedded-core"))]
const LITE_FAILURES_TO_RELOCATE: u32 = 8;

/// Tracks whether the main window has been revealed for the first time. Set the
/// instant any real show path runs (`focus_window`); read by the single-instance
/// handler so a relaunch *during cold boot* (the user double-clicks the icon
/// again while the window is still hidden and the Panel is bootstrapping) cannot
/// surface the half-loaded window early — which is exactly the Windows startup
/// flicker/jump. Once revealed, relaunches focus the window as usual.
#[derive(Default)]
struct RevealGate {
    done: AtomicBool,
}

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
/// stays out of it — the shell drives that itself (created hidden so geometry is
/// restored off-screen, shown with the splash as soon as setup finishes, hidden
/// again when the user closes to the tray).
fn window_state_flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION
}

/// Repair the PATH for a GUI (Finder/Dock) launch.
///
/// A double-click launch never sources the user's shell rc, so the process
/// inherits a minimal PATH missing Homebrew / nvm / asdf / cargo / etc. The
/// detached daemon we spawn — and every `bash_exec` child *it* spawns — then
/// can't find tools the user has on their interactive PATH. Probe the login
/// shell's PATH once and merge any missing entries into this process's PATH
/// before the daemon is spawned. Unix-only: a Windows GUI launch inherits the
/// system PATH already. Best-effort and silent — a probe failure just leaves the
/// inherited PATH untouched.
///
/// Called first thing in `main`, before any thread is spawned, so the
/// `set_var` is sound.
#[cfg(unix)]
fn ensure_login_shell_path() {
    let shell = match std::env::var("SHELL") {
        Ok(s) if !s.is_empty() => s,
        _ => return,
    };
    // `-ilc` = interactive login shell running one command: this is what sources
    // ~/.zprofile / ~/.zshrc / ~/.bash_profile where PATH is usually extended.
    let output = std::process::Command::new(&shell)
        .args(["-ilc", "printf %s \"$PATH\""])
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return,
    };
    let probed = String::from_utf8_lossy(&output.stdout);
    let probed = probed.trim();
    if probed.is_empty() {
        return;
    }

    let current = std::env::var("PATH").unwrap_or_default();
    let existing: std::collections::HashSet<&str> = current.split(':').collect();
    let mut merged = current.clone();
    for entry in probed.split(':') {
        if !entry.is_empty() && !existing.contains(entry) {
            if !merged.is_empty() {
                merged.push(':');
            }
            merged.push_str(entry);
        }
    }
    if merged != current {
        std::env::set_var("PATH", merged);
    }
}

#[cfg(not(unix))]
fn ensure_login_shell_path() {}

/// Work around WebKitGTK render-process crashes / blank-white webviews.
///
/// On many Mesa/driver stacks WebKitGTK's DMABUF renderer produces a blank or
/// crashing webview, which for an AppImage build hits a slice of Linux users on
/// first launch. Disabling it is the standard fix. Set before Tauri initializes
/// WebKit, and only when the user has not already chosen a value. Linux-only,
/// pure env — no business logic (R2/R10 clean).
#[cfg(target_os = "linux")]
fn linux_webkit_compat() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

#[cfg(not(target_os = "linux"))]
fn linux_webkit_compat() {}

fn main() {
    // Fix up the environment for GUI launches before anything reads it: the PATH
    // probe must precede the first daemon spawn, and the WebKit workaround must
    // precede WebKit initialization inside `tauri::Builder`.
    ensure_login_shell_path();
    linux_webkit_compat();
    init_tracing();

    let builder = tauri::Builder::default()
        // Single-instance must be registered first: a second launch focuses
        // the running shell instead of spawning a duplicate window. Its
        // `deep-link` feature also routes a second-launch `aleph://` link to
        // the running shell on Windows and Linux.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A relaunch focuses the running shell — but NOT before the first
            // reveal. During cold boot the window is intentionally hidden until
            // the Panel has painted; surfacing it early (the user double-clicks
            // again while it is still starting) shows a blank, still-bootstrapping
            // window that then navigates and re-lays-out on screen — the Windows
            // startup flicker/jump. Until the window has been revealed once, drop
            // the relaunch; the reveal path will show it when it is ready.
            if app.state::<RevealGate>().done.load(Ordering::SeqCst) {
                focus_window(app);
            }
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
        .plugin(tauri_plugin_global_shortcut::Builder::new().build());

    // The only `invoke_handler` the shell exposes: connection-config commands
    // (local vs remote Gateway target). These are I/O config toggles, not
    // business or UI logic — the R2/R4 boundary holds (spec §5.2 explicit
    // exception). Every *panel-facing* operation still goes through the
    // gateway's JSON-RPC (`fs.*` for directory browsing, `projects.*` for the
    // catalogue); the shell stays pure I/O: window, tray, updater, hotkey,
    // connection target.
    //
    // Both variants register `is_lite_shell` so the connect page can resolve
    // which variant hosts it deterministically (full answers `false`). The
    // panel-only variant additionally exposes the first-run setup surface
    // (`discover_servers` for LAN mDNS discovery, `connect_to` for a
    // probe-guarded target switch); those are compiled out of the full app.
    // The two arms are mutually exclusive `generate_handler!` invocations —
    // only one is ever registered.
    #[cfg(feature = "embedded-core")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        connection::get_connection_target,
        connection::set_connection_target,
        connection::clear_connection_target,
        connection::is_lite_shell,
        cert_trust::pending::get_pending_cert,
        cert_trust::pending::approve_cert,
        cert_trust::pending::reject_cert,
    ]);
    #[cfg(not(feature = "embedded-core"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        connection::get_connection_target,
        connection::set_connection_target,
        connection::clear_connection_target,
        connection::is_lite_shell,
        connect_setup::discover_servers,
        connect_setup::connect_to,
        cert_trust::pending::get_pending_cert,
        cert_trust::pending::approve_cert,
        cert_trust::pending::reject_cert,
    ]);

    // Shared update state: the background checker, the tray, and the macOS
    // menu all read it.
    let builder = builder.manage(update::Updater::default());

    // Shared pending-cert slot: the TLS-error hook (later task) writes to it,
    // the `cert-trust.html` approval page's commands read/resolve it. Managed
    // in both variants — either shell may connect to a self-signed remote
    // Gateway.
    let builder = builder.manage(cert_trust::pending::PendingCert::default());

    // Tracks the first Panel reveal (see `RevealGate`). Managed in both variants
    // so the single-instance handler can always read it; the full app's reveal
    // path is what flips it.
    let builder = builder.manage(RevealGate::default());

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

            // Show the window immediately, displaying the bundled splash. Rust
            // boots fast and the user should see the window promptly — we do NOT
            // wait for the daemon. The splash gives branded feedback for the few
            // seconds the bundled `aleph-server` takes to reach `/ready`, then
            // the Panel is navigated into the same window. This path is identical
            // on every platform (macOS/Linux/Windows) for a consistent cold-start
            // experience; only the backing differs (macOS gets the transparent
            // vibrancy material, Windows/Linux stay opaque).
            focus_window(&handle);

            // System tray — the resident, always-available face of the app.
            tray::build(&handle)?;

            // Global summon hotkey and the `aleph://` deep-link scheme.
            hotkey::setup(&handle);
            deeplink::setup(&handle);

            #[cfg(target_os = "macos")]
            apply_macos_vibrancy(&handle);

            // Watch for TCC permission grants that require a daemon restart
            // (input_monitoring, screen_recording). When the user grants one
            // in System Settings and returns to the app, the daemon is
            // restarted automatically — no manual "restart aleph-server" step.
            // Full-app only: the panel-only shell owns no local daemon to restart.
            #[cfg(feature = "embedded-core")]
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
        RunEvent::ExitRequested {
            code: None, api, ..
        } => {
            api.prevent_exit();
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
    // If this build starts pointed at a remote Gateway, authorize that origin
    // for the window-drag IPC before the webview is created (belt-and-braces
    // with the `navigate_to_target` grant) so the Panel's drag strip is live on
    // the first remote load.
    if let connection::ConnectionTarget::Remote(url) = connection::load_target() {
        grant_remote_drag(app, &url);
    }
    // `mut` is only needed on macOS, where the title-bar overlay is applied
    // via reassignment below; other platforms never rebind `builder`.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Aleph")
        .inner_size(1180.0, 800.0)
        .min_inner_size(720.0, 520.0)
        .center()
        .resizable(true)
        .visible(false)
        .initialization_script(SHELL_MARKER_JS)
        // Turn `target="_blank"` clicks into a top-level navigation so the
        // guard below can externalise them; pin the lone webview to the
        // Panel origin and hand outside URLs to the OS browser (R5).
        .initialization_script(external_link::CLICK_INTERCEPTOR_JS)
        // Intercept the banner's sentinel control links (apply / dismiss)
        // before the external-link guard: perform the shell action and cancel
        // the navigation so the reserved path never loads. The sentinel is
        // honoured only from the Panel's own origin (see
        // `update::control_action`) — a foreign page forging the path is
        // refused. Everything else falls through to the normal
        // internal/external routing.
        .on_navigation({
            let handle = app.clone();
            move |url| {
                if let Some(action) = update::control_action(url, &connection::load_target()) {
                    update::handle_control(&handle, action);
                    return false;
                }
                external_link::route(url)
            }
        })
        // `SHELL_MARKER_JS` (which sets `data-platform=macos`) is injected via
        // `initialization_script`, but that runs reliably only for same-origin
        // pages (custom protocol + loopback). A panel-only shell pointed at a
        // *remote* Gateway loads the Panel from a foreign origin where the init
        // script does not run — leaving `data-platform` unset, so the Panel's
        // macOS `-webkit-app-region: drag` band never activates and the
        // frameless window can't be dragged (full app is unaffected: it loads
        // the loopback Panel). Re-assert the marker on every page-load via
        // `eval` (host→webview, origin-independent). Idempotent same-origin.
        .on_page_load(|window, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                let _ = window.eval(SHELL_MARKER_JS);
                // A daemon-recovery reload re-navigates the Panel and wipes the
                // injected banner; put it back if an update is still staged and
                // the user has not dismissed it this session.
                update::reinject_banner_if_staged(window.app_handle());
            }
        });

    // Transparent only on macOS, where `apply_macos_vibrancy` installs an
    // opaque material behind the webview. On Windows/Linux a transparent window
    // has no backing, so WebView2 flashes the empty surface white on every
    // full-surface repaint during the Panel's multi-pass bootstrap render —
    // keep those platforms opaque so repaints composite over a stable backing.
    //
    // Overlay traffic lights over the content for a native-mac feel; the
    // Panel's CSS (Phase 2) leaves room for them.
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .transparent(true)
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }
    let window = builder.build()?;

    // Grant the Panel webview microphone access (voice-input button). No-op on
    // macOS where wry auto-grants; installs handlers on Windows/Linux.
    webview_perms::grant_microphone(&window);

    // Install the in-app self-signed TLS trust hook on the Panel webview. No-op
    // on platforms without an adapter yet; on macOS injects a WKWebView challenge
    // handler that TOFU-pins self-signed certs (fail-closed).
    cert_trust::install::install_cert_trust(&window);

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
                // Bring the chosen target online and reveal the Panel. The full
                // app supervises the bundled daemon; the panel-only shell just
                // navigates to the configured origin (no local daemon to manage).
                let up = bring_target_online(&handle).await;

                // Resident background tasks for the lifetime of the shell. The
                // OS-notification bridge and the auto-update checker are common
                // to both variants; the daemon/Gateway health supervisor is
                // full-app-only (the panel-only shell owns no daemon to keep
                // alive). None returns; run them together.
                #[cfg(feature = "embedded-core")]
                tokio::join!(
                    notify::run_notification_bridge(handle.clone()),
                    supervise_daemon(handle.clone(), up),
                    update::run_update_checker(handle),
                );
                #[cfg(not(feature = "embedded-core"))]
                {
                    let _ = up;
                    // Spawn the resident remote-health supervisor as a separate
                    // task so the notification bridge and update checker are not
                    // blocked by its loop (and vice versa).
                    tauri::async_runtime::spawn(supervise_remote_lite(handle.clone()));
                    tokio::join!(
                        notify::run_notification_bridge(handle.clone()),
                        update::run_update_checker(handle),
                    );
                }
            });
        });
    if let Err(e) = spawned {
        tracing::error!("failed to spawn background thread: {e}");
    }
}

/// Bring the embedded local core online and reveal the Panel. Returns whether
/// the daemon is believed up (the full-app supervisor's initial state).
///
/// The full app is local-only: it owns + supervises the bundled daemon
/// (reconcile any stale daemon offline, launch the bundled one, wait for
/// `/ready`, then reveal) and never honors a remote target.
#[cfg(feature = "embedded-core")]
async fn bring_target_online(handle: &tauri::AppHandle) -> bool {
    let version = handle.package_info().version.to_string();
    // The full app is local-only: it always serves its embedded loopback core
    // and never honors a remote target. Reconcile any legacy persisted remote
    // (from a previous build that allowed switching) back to Local so every
    // surface — menu reload, open-in-browser — agrees on local.
    if !matches!(
        connection::load_target(),
        connection::ConnectionTarget::Local
    ) {
        let _ = connection::save_target(&connection::ConnectionTarget::Local);
    }
    // Local navigations only ever touch loopback — clear any remote allow-list.
    external_link::set_remote_host(None);
    // First launch / post-update: force any stale daemon offline so the
    // `aleph-server` bundled in this app takes over.
    daemon::reconcile_for_version(&version).await;
    match daemon::ensure_ready().await {
        Ok(()) => {
            reveal_panel(handle);
            true
        }
        Err(e) => {
            tracing::error!("daemon did not become ready: {e}");
            show_daemon_error(handle, &e);
            false
        }
    }
}

/// Bring the configured target online and reveal the Panel (panel-only shell).
///
/// There is no local daemon to launch or supervise — the shell only points the
/// webview at a Gateway someone else runs. The first-run / unreachable cases
/// must never leave the user on a blank "can't connect" page (spec §8):
///
///   * No target ever chosen (first run) → open the bundled connection page so
///     the user can type an address or pick a `aleph-server` discovered on the
///     LAN via mDNS.
///   * A target is chosen but unreachable → open the same connection page
///     (with the failed address prefilled) so they can retry, change it, or
///     rescan — not a dead origin.
///   * Reachable → navigate to it.
///
/// Returns `true` unconditionally — the panel-only variant runs no health
/// supervisor that would consume the value.
#[cfg(not(feature = "embedded-core"))]
async fn bring_target_online(handle: &tauri::AppHandle) -> bool {
    // First run: no target has ever been chosen. Ask before navigating
    // anywhere — there is no sensible default Gateway for a panel-only shell.
    if !connection::marker_exists() {
        tracing::info!("no connection target configured — opening first-run connect page");
        connect_setup::show_lite_connect_page(handle);
        return true;
    }

    let target = connection::load_target();
    match &target {
        connection::ConnectionTarget::Local => external_link::set_remote_host(None),
        connection::ConnectionTarget::Remote(url) => {
            external_link::set_remote_host(Some(url.clone()));
        }
    }

    if connect_setup::target_reachable(&target).await {
        navigate_to_target(handle, &target);
        focus_window(handle);
    } else {
        // Configured but down: land on the operable connection page rather than
        // the webview's native error page (spec §8 — no white screen).
        tracing::warn!("configured Gateway unreachable at startup — opening connect page");
        connect_setup::show_lite_connect_page(handle);
    }
    true
}

/// Point the main window's webview at the configured Gateway origin. Local
/// resolves to the loopback default; Remote to the configured URL. Used for
/// the first reveal, for a silent reload after the daemon recovers, and by the
/// "Reload Panel" menu (after clearing the WebKit data store).
pub(crate) fn navigate_to_target(handle: &tauri::AppHandle, target: &connection::ConnectionTarget) {
    let Some(window) = handle.get_webview_window("main") else {
        tracing::error!("main window missing — cannot reach the Panel");
        return;
    };
    let url = match target {
        connection::ConnectionTarget::Local => match PANEL_URL.parse() {
            Ok(url) => url,
            Err(e) => {
                tracing::error!("invalid Panel URL: {e}");
                return;
            }
        },
        connection::ConnectionTarget::Remote(url) => {
            // Authorize this remote origin for the window-drag IPC *before*
            // navigating, so the Panel's `data-tauri-drag-region` strip drags
            // the window there too (see `grant_remote_drag`).
            grant_remote_drag(handle, url);
            url.clone()
        }
    };
    if let Err(e) = window.navigate(url) {
        tracing::error!("failed to navigate to the Panel: {e}");
    }
}

/// Grant the configured remote Gateway origin the *minimal* window-drag
/// permission so the Panel's `data-tauri-drag-region` strip can drag the window
/// on a remote target. Tauri scopes `core:window:allow-start-dragging` (and the
/// injected drag handler) to a capability's `remote.urls`; the static `default`
/// capability lists only loopback, so a panel-only shell pointed at a remote
/// Gateway otherwise gets no drag region at all — and the macOS Overlay
/// title-bar has no native drag area to fall back on (full app is unaffected: it
/// serves the loopback Panel, already covered by the static grant).
///
/// Scoped to this one origin and this one permission: the remote page gains
/// window dragging and nothing else (no fs/shell/config IPC).
fn grant_remote_drag(app: &tauri::AppHandle, url: &url::Url) {
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return;
    };
    let scheme = url.scheme();
    let cap = tauri::ipc::CapabilityBuilder::new(format!("remote-drag-{host}-{port}"))
        .remote(format!("{scheme}://{host}:{port}/*"))
        .window("main")
        .local(false)
        .permission("core:window:allow-start-dragging");
    if let Err(e) = app.add_capability(cap) {
        // A duplicate identifier (same remote re-selected this session) lands
        // here; the existing grant already covers it, so this is benign.
        tracing::debug!("remote drag capability not added: {e}");
    }
}

/// Bring the main window forward: show it, un-minimise it, focus it. Shared
/// by the tray, the single-instance handler, and the first Panel reveal.
pub(crate) fn focus_window(handle: &tauri::AppHandle) {
    let Some(window) = handle.get_webview_window("main") else {
        return;
    };
    // Latch "the window has been revealed at least once": a relaunch / tray /
    // Reopen after this point is a legitimate bring-to-front, but any focus
    // request *before* it (a re-click during cold boot) must be dropped so the
    // window is never surfaced mid-bootstrap (the single-instance handler reads
    // this). Every real show path funnels through here.
    handle
        .state::<RevealGate>()
        .done
        .store(true, Ordering::SeqCst);
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// Re-route the shell for a freshly-chosen target: update the link allow-list,
/// then navigate (and, in the full app, (re)start supervision). Called by the
/// connection commands (`set_connection_target` / `clear_connection_target`)
/// after the new target has been persisted. In the full app the resident
/// `supervise_daemon` loop picks up the persisted switch on its next tick and
/// re-arms in the matching mode.
///
/// Panel-only shell: every re-route is probe-gated — the webview navigates
/// only when the target's Gateway answers, otherwise it lands on the connect
/// page. This is the structural guard behind `connect_to`'s own pre-probe:
/// paths that reach `set_connection_target` directly (the full-app-shaped
/// `connect.html` fallback, "Back to Local", tray/menu) cannot strand the
/// user on a dead origin either (spec §8 — no white screen).
pub(crate) fn reroute_for_target(app: &tauri::AppHandle, target: connection::ConnectionTarget) {
    match &target {
        connection::ConnectionTarget::Remote(url) => {
            external_link::set_remote_host(Some(url.clone()));
            // Full app: navigate directly — the resident supervisor surfaces
            // an unreachable remote on its next tick.
            #[cfg(feature = "embedded-core")]
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.navigate(url.clone());
            }
        }
        connection::ConnectionTarget::Local => {
            external_link::set_remote_host(None);
            // Full app: bring the local daemon up, then reveal the Panel.
            #[cfg(feature = "embedded-core")]
            {
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
    // Panel-only shell: no daemon to manage in either arm — probe the target
    // and navigate only when it answers. The probe is async, so hop onto the
    // async runtime (the sync connection commands stay non-blocking).
    #[cfg(not(feature = "embedded-core"))]
    {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            if connect_setup::target_reachable(&target).await {
                navigate_to_target(&handle, &target);
                focus_window(&handle);
            } else {
                tracing::warn!("re-route target unreachable — opening connect page");
                connect_setup::show_lite_connect_page(&handle);
            }
        });
    }
}

/// Reveal the Panel for the first time: point the webview at the Panel and show
/// the window. The window appears as soon as the daemon is `/ready` — the shell
/// does not artificially defer it (Rust boots fast; the user should see the
/// window promptly). Full-app-only — the panel-only shell reveals via
/// `bring_target_online`.
#[cfg(feature = "embedded-core")]
fn reveal_panel(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        navigate_to_target(&handle, &connection::load_target());
        focus_window(&handle);
    });
}

/// Surface a daemon-startup failure on the splash screen. Full-app-only — the
/// panel-only shell launches no daemon and has no startup failure to surface.
#[cfg(feature = "embedded-core")]
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
    /// The local daemon crash-looped past the relaunch budget — stop hammering
    /// (spawning a process every poll interval forever) and surface a persistent
    /// error. Probing continues, so a manual restart still recovers.
    GiveUp,
}

/// Give up relaunching the local daemon after this many attempts within one
/// down episode. Without a cap, a daemon that binds-then-crashes was relaunched
/// every `HEALTH_POLL_INTERVAL` forever — a process-spawn / log-flood storm.
/// Mirrors the bridge `SpawnGate` RestartWindow philosophy.
const MAX_RELAUNCH_ATTEMPTS: u32 = 5;

/// Poll-ticks to wait *after* the Nth relaunch attempt before the next one,
/// so a crash loop spaces out instead of hammering. Each tick is one
/// `HEALTH_POLL_INTERVAL` (~5s): 1→2→4→8→12 ticks ≈ 5s→10s→20s→40s→60s.
const fn relaunch_backoff_ticks(attempt: u32) -> u32 {
    match attempt {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        _ => 12,
    }
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
    /// Relaunch attempts made in the current down episode (reset on recovery).
    relaunch_attempts: u32,
    /// Poll-ticks still to wait before the next relaunch attempt (backoff).
    ticks_until_relaunch: u32,
    /// Latched once the relaunch budget is spent — stops the storm; cleared on
    /// recovery so a later, unrelated crash gets a fresh budget.
    gave_up: bool,
}

impl Supervisor {
    /// Start supervising. `daemon_up` is the outcome of the initial boot:
    /// a failed boot starts the supervisor in `Down` so it keeps retrying.
    /// Full-app-only at runtime; available in tests for both variants.
    #[cfg(any(feature = "embedded-core", test))]
    const fn new(daemon_up: bool) -> Self {
        Self {
            health: if daemon_up {
                DaemonHealth::Up
            } else {
                DaemonHealth::Down
            },
            consecutive_failures: 0,
            remote: false,
            relaunch_attempts: 0,
            ticks_until_relaunch: 0,
            gave_up: false,
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
            relaunch_attempts: 0,
            ticks_until_relaunch: 0,
            gave_up: false,
        }
    }

    /// Construct the supervisor for `target`, seeded with its live reachability.
    /// Used both at boot and when the connection target changes at runtime.
    /// Seeding from a real probe means a freshly-switched, already-reachable
    /// target reports `Up` immediately — instead of starting `Down` and then
    /// firing a spurious `ReloadPanel` (a "recovery" that never happened) on the
    /// next tick, which would needlessly reload the Panel ~one poll interval
    /// after every switch. Full-app-only at runtime; available in tests.
    #[cfg(any(feature = "embedded-core", test))]
    fn for_target(target: &connection::ConnectionTarget, reachable: bool) -> Self {
        match target {
            connection::ConnectionTarget::Local => Self::new(reachable),
            connection::ConnectionTarget::Remote(_) => Self::new_remote(reachable),
        }
    }

    /// The action to take on a Down tick. Remote mode surfaces a connection
    /// error (the remote daemon is not ours to manage). Local mode relaunches,
    /// but with backoff + a give-up budget so a crash-looping daemon is not
    /// respawned every poll interval forever.
    const fn down_action(&mut self) -> SupervisorAction {
        if self.remote {
            return SupervisorAction::ShowConnectionError;
        }
        // Local: budget spent → stop hammering (probing still recovers on a
        // manual restart via the Down→Up arm).
        if self.gave_up {
            return SupervisorAction::Idle;
        }
        // Still cooling down between attempts.
        if self.ticks_until_relaunch > 0 {
            self.ticks_until_relaunch -= 1;
            return SupervisorAction::Idle;
        }
        if self.relaunch_attempts >= MAX_RELAUNCH_ATTEMPTS {
            self.gave_up = true;
            return SupervisorAction::GiveUp;
        }
        self.ticks_until_relaunch = relaunch_backoff_ticks(self.relaunch_attempts);
        self.relaunch_attempts += 1;
        SupervisorAction::Relaunch
    }

    /// Reset the relaunch budget — called when the daemon transitions into a
    /// fresh down episode, so each episode gets its own attempts/backoff.
    const fn arm_relaunch(&mut self) {
        self.relaunch_attempts = 0;
        self.ticks_until_relaunch = 0;
        self.gave_up = false;
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
                    // Entering a new down episode: fresh relaunch budget, then
                    // make the first attempt immediately.
                    self.arm_relaunch();
                    self.down_action()
                } else {
                    SupervisorAction::Idle
                }
            }
            (DaemonHealth::Down, false) => self.down_action(),
            (DaemonHealth::Down, true) => {
                self.health = DaemonHealth::Up;
                self.consecutive_failures = 0;
                self.arm_relaunch();
                SupervisorAction::ReloadPanel
            }
        }
    }
}

/// Keep the daemon alive for the lifetime of the shell. After the initial
/// boot this is the only thing standing between a crashed daemon and a dead
/// Panel: it relaunches the daemon when it disappears and reloads the Panel
/// once it is back. Silent by design — it never shows or focuses the window
/// (R5), it just keeps the plumbing connected. Full-app-only.
#[cfg(feature = "embedded-core")]
async fn supervise_daemon(handle: tauri::AppHandle, up: bool) {
    // The supervisor adapts to a target switch made at runtime (via the
    // connection commands / tray / menu): each tick re-reads the persisted
    // target and, when it differs from the one currently supervised, rebuilds
    // the state machine in the matching mode. This keeps a single resident
    // supervisor — no second loop is ever spawned.
    let mut target = connection::load_target();
    let mut supervisor = Supervisor::for_target(&target, up);
    // Latch: show the connect page once per outage. Without it the
    // ShowConnectionError arm re-navigates every tick (~5s), wiping any address
    // the user is typing (an R5 violation — mirrors the lite loop's `relocated`
    // guard). Cleared on any healthy/recovery tick and on a runtime target
    // switch so a future outage re-shows it.
    let mut connect_page_shown = false;
    loop {
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;

        // Re-read the target; a runtime switch re-arms the supervisor for the
        // new mode.
        let current = connection::load_target();

        // Probe the live target first — loopback `/ready` for Local, bare TCP
        // for Remote. Probing before the re-arm lets a freshly-switched target
        // be seeded with its real reachability (see `Supervisor::for_target`),
        // so a reachable new target is not mistaken for a Down→Up recovery and
        // does not trigger a redundant Panel reload.
        let ready = match &current {
            connection::ConnectionTarget::Local => daemon::is_ready().await,
            connection::ConnectionTarget::Remote(url) => {
                let host = url.host_str().unwrap_or("");
                let port = url.port_or_known_default().unwrap_or(18790);
                daemon::tcp_reachable(host, port).await
            }
        };

        if current != target {
            tracing::info!("connection target changed — re-arming supervisor");
            supervisor = Supervisor::for_target(&current, ready);
            target = current;
            // A runtime switch (e.g. back to a working target) must not stay
            // latched against re-showing the connect page on a later outage.
            connect_page_shown = false;
        }

        match supervisor.tick(ready) {
            SupervisorAction::Idle => {
                connect_page_shown = false;
            }
            SupervisorAction::Relaunch => {
                tracing::warn!("daemon unreachable — attempting relaunch");
                daemon::relaunch_if_down().await;
            }
            SupervisorAction::GiveUp => {
                tracing::error!(
                    "daemon crash-looped past the relaunch budget — giving up; \
                     showing error (a manual restart will still be picked up)"
                );
                show_daemon_error(
                    &handle,
                    "The Aleph daemon keeps crashing on startup. \
                     Check the logs, then restart the app.",
                );
            }
            SupervisorAction::ReloadPanel => {
                tracing::info!("Gateway recovered — reloading the Panel");
                connect_page_shown = false;
                match &target {
                    // In-window reload after the local daemon recovers — the
                    // Panel already holds its session state from the first reveal.
                    connection::ConnectionTarget::Local => navigate_to_target(&handle, &target),
                    // Remote recovery: re-point the webview at the remote root.
                    connection::ConnectionTarget::Remote(url) => {
                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.navigate(url.clone());
                        }
                    }
                }
            }
            SupervisorAction::ShowConnectionError => {
                if !connect_page_shown {
                    tracing::warn!("remote Gateway unreachable — showing connection page");
                    show_connection_page(
                        &handle,
                        "Remote Gateway unreachable. Retry or go back to local.",
                    );
                    connect_page_shown = true;
                }
            }
        }
    }
}

/// Surface a remote-Gateway connection failure by navigating the main window
/// to the bundled connection page and forwarding the message to its
/// `window.__alephError` hook. Mirrors `show_daemon_error`, but targets the
/// connect page (where the user can retry or fall back to Local) rather than
/// the splash. Full-app-only — wired into the daemon supervisor's Remote leg.
#[cfg(feature = "embedded-core")]
fn show_connection_page(handle: &tauri::AppHandle, message: &str) {
    let Some(window) = handle.get_webview_window("main") else {
        return;
    };
    let _ = window.show();
    if let Ok(url) = tauri::Url::parse(connection::connect_page_url()) {
        if let Err(e) = window.navigate(url) {
            tracing::warn!("could not navigate to the connection page: {e}");
        }
    }
    let safe = message.replace('\\', "\\\\").replace('\'', "\\'");
    let _ = window.eval(format!(
        "window.__alephError && window.__alephError('{safe}')"
    ));
}

/// Resident health loop for the panel-only (lite) shell. The full app uses
/// `supervise_daemon`; the lite shell hosts no daemon, so it only watches the
/// remote Gateway's reachability and, when it stays down past the budget,
/// relocates the webview to the bundled connect page (mDNS re-discovery +
/// manual address). Deliberately later than the panel's own ~31s reconnect
/// budget so a transient blip is recovered in-panel without yanking the user.
///
/// Timing: `LITE_REMOTE_POLL` (5s) × `LITE_FAILURES_TO_RELOCATE` (8) = 40s,
/// safely past the ~31s panel reconnect budget. `FAILURES_TO_DECLARE_DOWN` (3)
/// is used only by the inner `Supervisor` state machine to detect `ReloadPanel`
/// recovery; the outer counter (8 ticks) gates native relocation.
#[cfg(not(feature = "embedded-core"))]
async fn supervise_remote_lite(handle: tauri::AppHandle) {
    // Seed from a live probe so a reachable target isn't mistaken for recovery.
    let reachable = connect_setup::target_reachable(&connection::load_target()).await;
    let mut supervisor = Supervisor::new_remote(reachable);
    // Outer failure counter: native relocation fires only after this many
    // consecutive `ShowConnectionError` ticks — 5s × 8 = 40s total.
    let mut relocation_ticks: u32 = 0;
    // Latch: true after `show_lite_connect_page` fires; cleared on any healthy
    // or recovery tick so a *future* outage re-arms the one-shot relocation.
    // This prevents the connect page from being re-shown (and focus stolen)
    // every 40s while the remote stays down — an R5 violation.
    let mut relocated = false;
    loop {
        tokio::time::sleep(LITE_REMOTE_POLL).await;
        let ready = connect_setup::target_reachable(&connection::load_target()).await;
        if crate::cert_trust::pending::TRUST_PENDING.load(std::sync::atomic::Ordering::SeqCst) {
            // A trust prompt owns the screen — don't relocate, and don't let a
            // recovery tick navigate the webview away from the prompt.
            continue;
        }
        match supervisor.tick(ready) {
            SupervisorAction::ShowConnectionError => {
                relocation_ticks += 1;
                if relocation_ticks >= LITE_FAILURES_TO_RELOCATE && !relocated {
                    tracing::warn!(
                        "remote Gateway unreachable for {}s — relocating to connect page",
                        LITE_REMOTE_POLL.as_secs() * u64::from(LITE_FAILURES_TO_RELOCATE)
                    );
                    connect_setup::show_lite_connect_page(&handle);
                    relocated = true;
                }
            }
            // Remote recovered while we were on the connect page → re-point at it
            // and re-arm the latch so a future outage triggers relocation again.
            SupervisorAction::ReloadPanel => {
                relocation_ticks = 0;
                relocated = false;
                if let connection::ConnectionTarget::Remote(url) = connection::load_target() {
                    if let Some(window) = tauri::Manager::get_webview_window(&handle, "main") {
                        let _ = window.navigate(url);
                    }
                }
            }
            SupervisorAction::Idle => {
                relocation_ticks = 0;
                relocated = false;
            }
            // Local-only actions — unreachable in a remote-mode supervisor.
            SupervisorAction::Relaunch | SupervisorAction::GiveUp => {}
        }
    }
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

// Tests for the daemon-supervision state machine. The pure state machine
// (`Supervisor`/`DaemonHealth`/`SupervisorAction`) is always compiled; the
// full-app-specific tests that need `embedded-core` types are guarded below.
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
    fn supervisor_backs_off_between_relaunches_then_gives_up() {
        // A failed boot starts the supervisor already down. Relaunches must be
        // spaced by backoff (Idle ticks) and stop after the budget, not fire
        // every poll interval forever.
        let mut sup = Supervisor::new(false);
        let mut relaunches = 0;
        let mut gave_up = false;
        // Simulate a daemon that never comes back over many poll ticks.
        for _ in 0..200 {
            match sup.tick(false) {
                SupervisorAction::Relaunch => relaunches += 1,
                SupervisorAction::GiveUp => {
                    gave_up = true;
                    break;
                }
                SupervisorAction::Idle => {} // backoff wait
                other => panic!("unexpected action while down: {other:?}"),
            }
        }
        assert_eq!(
            relaunches, MAX_RELAUNCH_ATTEMPTS as usize,
            "exactly the budgeted number of relaunches"
        );
        assert!(
            gave_up,
            "must give up after the budget instead of hammering"
        );
        // Once given up, further down ticks stay Idle — no more spawns.
        assert_eq!(sup.tick(false), SupervisorAction::Idle);
    }

    #[test]
    fn supervisor_spaces_relaunches_with_idle_backoff() {
        let mut sup = Supervisor::new(false);
        // First down tick relaunches immediately…
        assert_eq!(sup.tick(false), SupervisorAction::Relaunch);
        // …then backs off one tick before the second attempt.
        assert_eq!(sup.tick(false), SupervisorAction::Idle);
        assert_eq!(sup.tick(false), SupervisorAction::Relaunch);
    }

    #[test]
    fn supervisor_rearms_relaunch_budget_after_recovery() {
        // Spend the whole budget, recover, then crash again → fresh budget.
        let mut sup = Supervisor::new(false);
        for _ in 0..200 {
            if sup.tick(false) == SupervisorAction::GiveUp {
                break;
            }
        }
        assert!(sup.gave_up, "budget should be spent");
        // Recovery re-arms the budget.
        assert_eq!(sup.tick(true), SupervisorAction::ReloadPanel);
        assert!(!sup.gave_up, "recovery must clear the give-up latch");
        // A fresh crash episode relaunches again over subsequent down ticks.
        let mut relaunched_again = false;
        for _ in 0..200 {
            if sup.tick(false) == SupervisorAction::Relaunch {
                relaunched_again = true;
                break;
            }
        }
        assert!(
            relaunched_again,
            "a new down episode must get a fresh relaunch budget"
        );
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

    #[test]
    fn rearm_to_reachable_target_is_idle_not_spurious_reload() {
        // Regression for the redundant-reload bug: a freshly-switched target
        // that is already reachable must be seeded `Up` via `for_target`, so the
        // next probe tick is `Idle` — not a phantom Down→Up `ReloadPanel` that
        // would reload the Panel ~one poll interval after the switch.
        let local = connection::ConnectionTarget::Local;
        let mut sup_local = Supervisor::for_target(&local, true);
        assert_eq!(sup_local.tick(true), SupervisorAction::Idle);

        let remote = connection::ConnectionTarget::parse("box.lan:9000").unwrap();
        let mut sup_remote = Supervisor::for_target(&remote, true);
        assert_eq!(sup_remote.tick(true), SupervisorAction::Idle);
    }

    #[test]
    fn rearm_to_unreachable_target_acts_on_first_tick() {
        // Seeded `Down` from a failed probe: Local relaunches, Remote surfaces a
        // connection error — both on the first tick, no slower than before.
        let local = connection::ConnectionTarget::Local;
        let mut sup_local = Supervisor::for_target(&local, false);
        assert_eq!(sup_local.tick(false), SupervisorAction::Relaunch);

        let remote = connection::ConnectionTarget::parse("box.lan:9000").unwrap();
        let mut sup_remote = Supervisor::for_target(&remote, false);
        assert_eq!(
            sup_remote.tick(false),
            SupervisorAction::ShowConnectionError
        );
    }

    // --- lite-shell remote-supervisor tests ---
    // These exercise the same pure state machine from the lite shell's perspective.
    // `Supervisor::new_remote` / `tick` compile in both variants now that the
    // state machine cfg guard has been relaxed.

    #[test]
    fn remote_supervisor_declares_down_and_shows_connection_page() {
        let mut sup = Supervisor::new_remote(true);
        // Healthy period: idle.
        assert_eq!(sup.tick(true), SupervisorAction::Idle);
        // Consecutive failures accumulate to the threshold; remote leg must emit
        // ShowConnectionError (not Relaunch — we don't own that daemon).
        let mut last = SupervisorAction::Idle;
        for _ in 0..FAILURES_TO_DECLARE_DOWN {
            last = sup.tick(false);
        }
        assert_eq!(last, SupervisorAction::ShowConnectionError);
    }

    #[test]
    fn remote_supervisor_recovers_without_relaunch() {
        let mut sup = Supervisor::new_remote(false);
        // Down→Up recovery must emit ReloadPanel, never Relaunch.
        assert_eq!(sup.tick(true), SupervisorAction::ReloadPanel);
    }
}
