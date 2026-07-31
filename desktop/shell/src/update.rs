//! Background auto-update.
//!
//! Checks GitHub Releases for a newer Aleph and — never stealing focus or
//! restarting under the user (R5) — surfaces an available update through
//! the tray and a desktop notification. Applying it is always the user's
//! explicit choice; doing so downloads, installs, and restarts the app
//! (and, with it, the bundled `aleph-server`).
//!
//! Best-effort: an unreachable or unconfigured update endpoint is logged
//! and the rest of the shell carries on unaffected.

use std::sync::Mutex;
use std::time::Duration;

use tauri::menu::MenuItem;
use tauri::{AppHandle, Manager, Url, Wry};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// Delay before the first check — let the daemon boot and the Panel settle.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(90);
/// Interval between background checks for a long-running shell.
const CHECK_INTERVAL: Duration = Duration::from_hours(6);

/// Where to send users whose install can't self-update (Linux package
/// installs): the GitHub releases page.
const RELEASES_URL: &str = "https://github.com/rootazero/Aleph/releases/latest";

/// Reserved shell-control paths the in-window update banner navigates to. The
/// `on_navigation` guard (`main.rs`) intercepts and cancels these, so they
/// never actually load — the Panel/daemon never serve the `/__aleph-shell/`
/// prefix. A sentinel is honoured only when it also comes from the origin the
/// Panel is actually served from (loopback for the full app, the configured
/// Gateway for Panel-lite) — path matching alone would let any rendered link
/// (e.g. an LLM-generated markdown link to `https://evil.com/__aleph-shell/
/// update/apply`) trigger a download + install + restart. Tauri IPC is
/// loopback-scoped and unavailable from a remote origin, so this navigation
/// channel remains the only one that works for both variants.
const APPLY_PATH: &str = "/__aleph-shell/update/apply";
const DISMISS_PATH: &str = "/__aleph-shell/update/dismiss";

/// A banner control signal routed from the webview back to the shell via a
/// sentinel navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateControl {
    /// Apply the staged update and restart.
    Apply,
    /// Hide the banner for this session.
    Dismiss,
}

/// Recognise a banner control link. Returns `None` for ordinary Panel routes
/// and external links, which must pass through to `external_link::route` —
/// and for sentinel paths arriving from any origin other than the Panel's own
/// (`target`), so a drive-by link on a foreign page cannot apply or dismiss
/// an update. The banner's relative `location.href` navigations resolve to
/// exactly the Panel origin, so legitimate controls are unaffected.
pub fn control_action(
    url: &Url,
    target: &crate::connection::ConnectionTarget,
) -> Option<UpdateControl> {
    let action = match url.path() {
        APPLY_PATH => UpdateControl::Apply,
        DISMISS_PATH => UpdateControl::Dismiss,
        _ => return None,
    };
    target.serves_origin(url).then_some(action)
}

/// The injected banner as a JS template. Placeholders (`__MSG__`, `__LABEL__`,
/// `__HREF__`, `__DISMISS__`, `__ISRESTART__`) are replaced with JSON-encoded
/// (JS-safe) literals by `banner_script`. Built with `createElement` +
/// `addEventListener` (never inline `onclick`) so a strict Panel CSP cannot
/// block it; the buttons navigate via `location.href`, which is unaffected by
/// script-CSP. On macOS (`data-platform="macos"`) the bar is offset below the
/// overlay-titlebar traffic lights.
const BANNER_TEMPLATE: &str = r"(function(){
var ID='__aleph-update-banner';
var old=document.getElementById(ID); if(old) old.remove();
var mac=document.documentElement.getAttribute('data-platform')==='macos';
var dark=window.matchMedia&&window.matchMedia('(prefers-color-scheme: dark)').matches;
var bar=document.createElement('div'); bar.id=ID; bar.setAttribute('role','status');
bar.style.cssText='position:fixed;left:0;right:0;top:'+(mac?'28px':'0px')+';z-index:2147483000;display:flex;align-items:center;gap:12px;padding:8px 14px;font:13px -apple-system,system-ui,sans-serif;box-shadow:0 1px 4px rgba(0,0,0,.25);'+(dark?'background:#1f2430;color:#e6e9ef;':'background:#f4f6fb;color:#1b2130;');
var msg=document.createElement('span'); msg.style.cssText='flex:1;'; msg.textContent=__MSG__;
var act=document.createElement('button'); act.textContent=__LABEL__;
act.style.cssText='cursor:pointer;border:0;border-radius:6px;padding:5px 12px;font:inherit;font-weight:600;background:#3b82f6;color:#fff;';
act.addEventListener('click',function(){ if(__ISRESTART__){ act.disabled=true; act.textContent='Updating…'; msg.textContent='Updating — Aleph will restart shortly.'; } window.location.href=__HREF__; });
var close=document.createElement('button'); close.setAttribute('aria-label','Dismiss'); close.textContent='×';
close.style.cssText='cursor:pointer;border:0;background:transparent;color:inherit;font-size:18px;line-height:1;padding:0 6px;';
close.addEventListener('click',function(){ window.location.href=__DISMISS__; });
bar.appendChild(msg); bar.appendChild(act); bar.appendChild(close);
(document.body||document.documentElement).appendChild(bar);
})();";

/// Build the banner-injection JS for a staged `version`. `self_install`
/// distinguishes platforms that can self-update (macOS / Windows / Linux
/// AppImage — restart-to-apply) from package-manager installs (Linux
/// .deb/.rpm — point the user at the releases page instead).
fn banner_script(version: &str, self_install: bool) -> String {
    let msg = serde_json::to_string(&format!("Aleph v{version} is ready"))
        .unwrap_or_else(|_| "\"Aleph update is ready\"".to_string());
    let (label, href, is_restart) = if self_install {
        ("Restart to update", APPLY_PATH, "true")
    } else {
        ("How to update", RELEASES_URL, "false")
    };
    let json = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
    // Apply `__MSG__` LAST: it carries the attacker-influenceable `version`, and
    // the other replacement values are fixed constants with no placeholder
    // tokens, so substituting the message body last means no later pass can
    // rewrite a placeholder token that appeared inside the version string.
    BANNER_TEMPLATE
        .replace("__LABEL__", &json(label))
        .replace("__HREF__", &json(href))
        .replace("__DISMISS__", &json(DISMISS_PATH))
        .replace("__ISRESTART__", is_restart)
        .replace("__MSG__", &msg)
}

/// Inject (or replace) the update banner in the main window's current
/// document. No-op when nothing is staged or the main window is gone.
///
/// `stage()` runs on a background async task, but `window.eval` touches the
/// webview, which is UI-thread-affine (WebView2 on Windows especially). Marshal
/// the injection to the main thread, mirroring `relabel_update_items`.
pub fn show_update_banner(app: &AppHandle) {
    let version = app
        .state::<Updater>()
        .staged
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(version) = version else {
        return;
    };
    let script = banner_script(&version, updater_can_self_install());
    let app = app.clone();
    let dispatch = app.run_on_main_thread({
        let app = app.clone();
        move || {
            let Some(window) = app.get_webview_window("main") else {
                return;
            };
            if let Err(e) = window.eval(script) {
                tracing::debug!("could not inject the update banner: {e}");
            }
        }
    });
    if let Err(e) = dispatch {
        tracing::debug!("could not dispatch the update banner injection: {e}");
    }
}

/// Re-inject the banner after a Panel reload wiped the injected DOM — but only
/// if an update is staged and the user has not dismissed it this session.
/// Wired into `main.rs`'s `on_page_load(Finished)` handler.
pub fn reinject_banner_if_staged(app: &AppHandle) {
    let dismissed = *app
        .state::<Updater>()
        .dismissed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if dismissed {
        return;
    }
    show_update_banner(app);
}

/// Perform a banner control action routed from the `on_navigation` guard.
pub fn handle_control(app: &AppHandle, action: UpdateControl) {
    match action {
        UpdateControl::Apply => apply_staged_update(app),
        UpdateControl::Dismiss => {
            *app.state::<Updater>()
                .dismissed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            remove_banner(app);
        }
    }
}

/// Remove the injected banner element from the current document.
fn remove_banner(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ =
            window.eval("var b=document.getElementById('__aleph-update-banner');if(b)b.remove();");
    }
}

/// Shared update state, managed by Tauri so the background checker, the
/// tray, and the macOS menu agree on whether an update is waiting.
#[derive(Default)]
pub struct Updater {
    /// The version of a found-but-not-yet-applied update, if any.
    staged: Mutex<Option<String>>,
    /// The update menu items (tray, and the macOS app menu) registered by
    /// their builders so the checker can relabel them once an update is
    /// staged. Both surfaces stay in sync.
    update_items: Mutex<Vec<MenuItem<Wry>>>,
    /// Session latch: set when the user dismisses the in-window banner (`×`).
    /// In-memory only, so a fresh launch re-shows the banner (spec §5).
    dismissed: Mutex<bool>,
    /// Session latch: set while an Apply is in-flight (download + install +
    /// restart). Guards against a second concurrent apply — the tray menu,
    /// the in-window banner, and a stray navigation can all trigger Apply,
    /// and without this latch a rapid double-click would start two downloads
    /// and two installer invocations and have the second race past the first
    /// restart.
    applying: Mutex<bool>,
}

impl Updater {
    /// Register an update menu item to be relabeled once an update is staged.
    /// Called by both the tray and the macOS app menu.
    pub fn attach_update_item(&self, item: MenuItem<Wry>) {
        self.update_items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(item);
    }

    /// Take the in-flight apply latch. `true` if this caller now owns the
    /// apply slot; `false` if an apply is already running.
    fn try_begin_apply(&self) -> bool {
        let mut applying = self
            .applying
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *applying {
            return false;
        }
        *applying = true;
        true
    }

    /// Release the apply latch so a later attempt can run. Called when the
    /// apply task ends — without it a single failed Apply would latch the
    /// flag forever and brick "Restart to update" for the rest of the session.
    fn end_apply(&self) {
        *self
            .applying
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
    }
}

/// Whether Tauri's bundled updater can self-install on this platform/install.
///
/// Tauri's Linux updater only supports the AppImage format — it replaces the
/// running AppImage in place, keyed off the `APPIMAGE` env var the AppImage
/// runtime sets. A package-manager install (.deb / .rpm) has no `APPIMAGE`
/// and must be updated through the package manager instead. macOS and Windows
/// always self-install.
fn updater_can_self_install() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("APPIMAGE").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// True once an update has been found and is waiting to be applied.
pub fn has_staged_update(app: &AppHandle) -> bool {
    app.state::<Updater>()
        .staged
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some()
}

/// The background update checker: one delayed first check, then a slow
/// poll for the lifetime of the shell. Silent unless it finds an update.
pub async fn run_update_checker(app: AppHandle) {
    tokio::time::sleep(FIRST_CHECK_DELAY).await;
    loop {
        if !has_staged_update(&app) {
            check(&app, Announce::OnlyOnUpdate).await;
        }
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

/// Run a user-initiated check now (from the tray or the macOS menu). Unlike
/// the background checker it always reports the outcome.
pub fn check_now(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        check(&app, Announce::Always).await;
    });
}

/// Download, install, and restart into the staged update — the user has
/// explicitly asked for it, so the restart is expected.
pub fn apply_staged_update(app: &AppHandle) {
    // Latch against a second concurrent apply. A user double-clicks "Restart
    // to update" in the tray, the in-window banner also navigates here, and
    // the macOS menu does the same — without this latch two parallel
    // downloaders race and a second installer races past the first restart.
    {
        let updater = app.state::<Updater>();
        if !updater.try_begin_apply() {
            tracing::debug!("apply_staged_update already in flight — ignoring duplicate");
            return;
        }
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // The latch is released when the task ends, whatever happens inside —
        // every early return, a failed download, a failed install — so one
        // failed Apply does not brick the action for the rest of the session.
        // On success `app.restart()` exits the process before this line runs,
        // which is fine: a fresh process starts unlatched.
        apply_inner(&app).await;
        app.state::<Updater>().end_apply();
    });
}

/// The body of an Apply, run as a background task by [`apply_staged_update`].
async fn apply_inner(app: &AppHandle) {
    // Package-manager installs (Linux .deb / .rpm) can't be self-installed
    // by Tauri's updater — point the user at the right path instead of
    // attempting a download_and_install that would fail.
    if !updater_can_self_install() {
        notify(
            app,
            "Update via your package manager",
            &format!(
                "Aleph was installed with your system package manager. Update \
                 with apt / dnf, or download the latest release from {RELEASES_URL}."
            ),
        );
        return;
    }
    notify(
        app,
        "Updating Aleph",
        "Downloading the update — Aleph will restart shortly.",
    );
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("updater unavailable: {e}");
            return;
        }
    };
    let update = match updater.check().await {
        Ok(Some(u)) => u,
        Ok(None) => {
            tracing::info!("nothing to apply — already up to date");
            return;
        }
        Err(e) => {
            tracing::error!("update re-check failed: {e}");
            notify(app, "Update failed", "Could not reach the update server.");
            return;
        }
    };
    // Download first while the daemon is still running; the installer will
    // try to overwrite aleph-server.exe, which Windows locks while it is
    // executing. We stop the daemon only once the package is local so the
    // service interruption is as short as possible.
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("update download failed: {e}");
            notify(app, "Update failed", "Could not download the update.");
            return;
        }
    };
    stop_daemon_for_update().await;
    match update.install(&bytes) {
        Ok(()) => {
            tracing::info!("update installed — restarting");
            app.restart();
        }
        Err(e) => {
            tracing::error!("update install failed: {e}");
            notify(app, "Update failed", "Could not install the update.");
        }
    }
}

/// Stop the bundled `aleph-server` so the installer can replace its binary.
#[cfg(feature = "embedded-core")]
async fn stop_daemon_for_update() {
    crate::daemon::stop_daemon();
    crate::daemon::wait_until_port_closed().await;
}

/// Panel-only shells have no bundled daemon to stop.
#[cfg(not(feature = "embedded-core"))]
async fn stop_daemon_for_update() {}

/// Whether a check announces a no-update / failure outcome to the user.
#[derive(Clone, Copy)]
enum Announce {
    /// Stay silent unless an update is found (background checks).
    OnlyOnUpdate,
    /// Always report the outcome (user-initiated checks).
    Always,
}

/// Probe the update endpoint once and fold the result into shared state.
async fn check(app: &AppHandle, announce: Announce) {
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            tracing::debug!("updater unavailable: {e}");
            if matches!(announce, Announce::Always) {
                notify(app, "Update check failed", "The updater is not configured.");
            }
            return;
        }
    };
    match updater.check().await {
        Ok(Some(update)) => {
            tracing::info!("update available: v{}", update.version);
            stage(app, &update.version);
            if updater_can_self_install() {
                notify(
                    app,
                    "Update available",
                    &format!(
                        "Aleph v{} is ready — choose \"Restart to update\" from the tray \
                         or the Aleph menu.",
                        update.version
                    ),
                );
            } else {
                notify(
                    app,
                    "Update available",
                    &format!(
                        "Aleph v{} is available. You installed Aleph via your package \
                         manager — update with apt / dnf, or download it from {RELEASES_URL}.",
                        update.version
                    ),
                );
            }
        }
        Ok(None) => {
            tracing::debug!("no update available");
            if matches!(announce, Announce::Always) {
                notify(
                    app,
                    "Aleph is up to date",
                    "You are running the latest version.",
                );
            }
        }
        Err(e) => {
            tracing::debug!("update check failed: {e}");
            if matches!(announce, Announce::Always) {
                notify(
                    app,
                    "Update check failed",
                    "Could not reach the update server.",
                );
            }
        }
    }
}

/// Record a staged update and relabel every registered update item, then
/// surface the in-window banner.
fn stage(app: &AppHandle, version: &str) {
    *app.state::<Updater>()
        .staged
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(version.to_string());
    relabel_update_items(app, staged_label(version));
    show_update_banner(app);
}

/// Relabel the registered update items (tray + macOS menu) on the main
/// thread — menu mutation must not happen off the UI thread.
fn relabel_update_items(app: &AppHandle, label: String) {
    let app = app.clone();
    let dispatch = app.run_on_main_thread({
        let app = app.clone();
        move || {
            let updater = app.state::<Updater>();
            let items = updater
                .update_items
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for item in items.iter() {
                let _ = item.set_text(&label);
            }
        }
    });
    if let Err(e) = dispatch {
        tracing::debug!("could not relabel the update menu items: {e}");
    }
}

/// The update item's label once an update is staged. Installs that can
/// self-update offer the restart-to-apply action; package-manager installs
/// (which can't) are pointed at how to update instead.
fn staged_label(version: &str) -> String {
    if updater_can_self_install() {
        restart_label(version)
    } else {
        manual_update_label(version)
    }
}

/// Label for the apply-and-restart action (self-installing platforms).
fn restart_label(version: &str) -> String {
    format!("Restart to update to v{version}")
}

/// Label for installs that must update via their package manager.
fn manual_update_label(version: &str) -> String {
    format!("Update v{version} available — how to update")
}

/// Raise a desktop notification, best-effort.
fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!("failed to show update notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_label_names_the_version() {
        assert_eq!(restart_label("26.5.30"), "Restart to update to v26.5.30");
    }

    #[test]
    fn manual_update_label_names_the_version() {
        assert_eq!(
            manual_update_label("26.5.30"),
            "Update v26.5.30 available — how to update"
        );
    }

    #[test]
    fn apply_latch_refuses_a_concurrent_apply_and_releases() {
        let updater = Updater::default();
        assert!(updater.try_begin_apply());
        assert!(
            !updater.try_begin_apply(),
            "a second concurrent apply must be refused"
        );
        updater.end_apply();
        assert!(
            updater.try_begin_apply(),
            "the latch must be re-takable once the failed apply ends"
        );
    }

    #[test]
    fn control_action_recognises_the_apply_sentinel_on_the_panel_origin() {
        let target = crate::connection::ConnectionTarget::Local;
        let url = Url::parse("http://127.0.0.1:18790/__aleph-shell/update/apply").unwrap();
        assert_eq!(control_action(&url, &target), Some(UpdateControl::Apply));
    }

    #[test]
    fn control_action_recognises_the_dismiss_sentinel_on_the_remote_origin() {
        // Panel-lite pointed at a LAN Gateway: the Panel (and the banner) is
        // served from that origin, so the sentinel must match there.
        let target = crate::connection::ConnectionTarget::parse("http://box.lan:9000").unwrap();
        let url = Url::parse("http://box.lan:9000/__aleph-shell/update/dismiss").unwrap();
        assert_eq!(control_action(&url, &target), Some(UpdateControl::Dismiss));
    }

    #[test]
    fn control_action_rejects_the_sentinel_on_a_foreign_origin() {
        // The drive-by case: a rendered link (e.g. LLM markdown) pointing the
        // webview at the sentinel path on an attacker's origin must NOT act.
        let local = crate::connection::ConnectionTarget::Local;
        let evil_apply = Url::parse("http://evil.com/__aleph-shell/update/apply").unwrap();
        assert_eq!(control_action(&evil_apply, &local), None);
        let remote = crate::connection::ConnectionTarget::parse("http://box.lan:9000").unwrap();
        assert_eq!(control_action(&evil_apply, &remote), None);
        // A different port on the same host is a different origin — no piggy-back.
        let other_port = Url::parse("http://box.lan:9001/__aleph-shell/update/apply").unwrap();
        assert_eq!(control_action(&other_port, &remote), None);
    }

    #[test]
    fn control_action_ignores_ordinary_urls() {
        let target = crate::connection::ConnectionTarget::Local;
        for u in [
            "http://127.0.0.1:18790/",
            "http://127.0.0.1:18790/chat",
            "https://github.com/rootazero/Aleph/releases/latest",
            "tauri://localhost/index.html",
        ] {
            assert_eq!(
                control_action(&Url::parse(u).unwrap(), &target),
                None,
                "{u}"
            );
        }
    }

    #[test]
    fn control_action_matches_apply_even_with_query() {
        let target = crate::connection::ConnectionTarget::Local;
        let url = Url::parse("http://127.0.0.1:18790/__aleph-shell/update/apply?v=1").unwrap();
        assert_eq!(control_action(&url, &target), Some(UpdateControl::Apply));
    }

    #[test]
    fn banner_script_self_install_offers_restart_and_sentinels() {
        let js = banner_script("26.7.14", true);
        assert!(js.contains("Aleph v26.7.14 is ready"));
        assert!(js.contains("Restart to update"));
        assert!(js.contains("/__aleph-shell/update/apply"));
        assert!(js.contains("/__aleph-shell/update/dismiss"));
        // Idempotent injection: removes any prior banner by id first.
        assert!(js.contains("__aleph-update-banner"));
    }

    #[test]
    fn banner_script_package_manager_offers_howto_not_restart() {
        let js = banner_script("26.7.14", false);
        assert!(js.contains("How to update"));
        assert!(js.contains(RELEASES_URL));
        // The restart apply-sentinel must NOT be the primary action here.
        assert!(!js.contains("/__aleph-shell/update/apply"));
        // Dismiss still works.
        assert!(js.contains("/__aleph-shell/update/dismiss"));
    }

    #[test]
    fn banner_script_escapes_a_hostile_version() {
        let js = banner_script("1\"; alert(1);//", true);
        // The embedded quote is escaped by serde_json, so it cannot break out
        // of the JS string literal.
        assert!(!js.contains("1\"; alert"));
        assert!(js.contains("1\\\"; alert"));
    }

    #[test]
    fn banner_script_does_not_launder_placeholder_tokens_in_version() {
        // A hostile version embedding a later placeholder token must survive
        // verbatim in the message, not be rewritten by a subsequent pass.
        let js = banner_script("__HREF__", true);
        assert!(js.contains("Aleph v__HREF__ is ready"));
    }
}
