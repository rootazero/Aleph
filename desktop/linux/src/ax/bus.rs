//! Connecting to the AT-SPI accessibility bus and finding applications on it.
//!
//! AT-SPI is a D-Bus service: a per-session *accessibility* bus (separate from
//! the session bus) whose root object's children are the running applications
//! that have accessibility enabled.
//!
//! **No handle outlives its call.** Every proxy is addressed by (bus name,
//! object path) and re-resolved on each use, for the same reason the Windows UI
//! Automation limb builds a fresh element per call: a handle that outlives its
//! call goes stale, and a locator that re-resolves is worth more than a handle
//! that has to be invalidated.
//!
//! **The connection does outlive its call**, and that is not the same claim —
//! see [`Bus::open`] for what it costs and why the distinction matters.

use atspi::proxy::accessible::AccessibleProxy;
use atspi::{AccessibilityConnection, State};

use aleph_desktop::{DesktopError, Result};

/// How many application pid lookups may be in flight at once.
///
/// One per application on the bus, and they are answered by the message bus
/// daemon rather than by the applications themselves, so there is no
/// single-threaded target to overwhelm — this bound exists only to keep the
/// in-flight set proportional to a desktop rather than unbounded.
const APP_LOOKUP_CONCURRENCY: usize = 24;

/// The process-wide accessibility-bus connection, built on first use.
///
/// A `Mutex<Option<..>>` rather than a `OnceCell` because it must be
/// *replaceable*: see [`Bus::open`].
fn shared_connection() -> &'static tokio::sync::Mutex<Option<AccessibilityConnection>> {
    static SHARED: std::sync::OnceLock<tokio::sync::Mutex<Option<AccessibilityConnection>>> =
        std::sync::OnceLock::new();
    SHARED.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// How long the liveness probe waits before declaring a shared connection dead.
///
/// It is a round trip to the message-bus daemon on an already-open socket — a
/// couple of milliseconds when anything is driving the connection at all. The
/// budget is generous only so a loaded machine does not throw away a healthy
/// connection; it is not a timeout anyone should be near.
const LIVENESS_PROBE: std::time::Duration = std::time::Duration::from_millis(250);

/// Is this connection still being driven?
///
/// See [`Bus::open`] for why this is not paranoia: an undriven zbus connection
/// does not fail, it waits, and it waits forever.
async fn is_alive(conn: &AccessibilityConnection) -> bool {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(conn.connection()).await else {
        return false;
    };
    matches!(
        tokio::time::timeout(LIVENESS_PROBE, dbus.get_id()).await,
        Ok(Ok(_))
    )
}

/// Per-call handle on the accessibility bus.
pub struct Bus {
    conn: AccessibilityConnection,
}

/// One application visible on the accessibility bus.
pub struct App {
    /// The owning process, resolved from the application's unique bus name.
    pub pid: i32,
    /// The application's root accessible object.
    pub root: AccessibleProxy<'static>,
}

impl Bus {
    /// A connection to the accessibility bus.
    ///
    /// # The connection is shared; the objects on it are not
    ///
    /// Opening one is expensive in a way that is invisible from the call site:
    /// it resolves the a11y bus address over the *session* bus and then performs
    /// a second full connection handshake. **Measured on this host: 424 ms**,
    /// paid before any real work on every snapshot, locator resolution and focus
    /// query — several times the cost of the tree walk that follows it.
    ///
    /// This module used to open one per call, reasoning by analogy with the
    /// Windows limb building a fresh `IUIAutomation` each time. The analogy does
    /// not hold: what goes stale there is an *element handle*, and nothing here
    /// keeps one. Every proxy is addressed by (bus name, object path) and
    /// re-resolved per call, exactly as before — a D-Bus connection is a socket,
    /// not a handle to anything that can change underneath it.
    ///
    /// # A shared connection has to be *checked*, not assumed
    ///
    /// A zbus connection is driven by a task on the runtime that built it. Reuse
    /// it from a different runtime — or after the a11y bus has been restarted —
    /// and it is not an error: it is a socket nobody is reading, so every call
    /// on it **waits forever**. That failure mode is invisible (no error, no
    /// log, just a turn that never ends), so the reuse path pays one bounded
    /// round trip to prove the connection is alive before handing it out. At
    /// ~2 ms against the 424 ms it replaces, correctness here is nearly free.
    ///
    /// The daemon has one runtime for its whole life, so in production this
    /// probe always passes on the first try; it is the test binaries — a fresh
    /// current-thread runtime per `#[tokio::test]` — that would otherwise hang,
    /// and that is exactly the signal worth keeping.
    ///
    /// # Errors
    ///
    /// [`DesktopError::NotAvailable`] when the bus cannot be reached — most
    /// often because the desktop has accessibility support switched off, which
    /// is a thing a user can fix, so the message says how.
    pub async fn open() -> Result<Self> {
        let mut shared = shared_connection().lock().await;
        if let Some(conn) = shared.as_ref() {
            if is_alive(conn).await {
                return Ok(Self { conn: conn.clone() });
            }
            *shared = None;
        }
        let conn = AccessibilityConnection::new().await.map_err(|e| {
            DesktopError::NotAvailable(format!(
                "Cannot reach the AT-SPI accessibility bus ({e}). Install `at-spi2-core` and \
                 enable toolkit accessibility (GNOME: `gsettings set \
                 org.gnome.desktop.interface toolkit-accessibility true`; other desktops: export \
                 `GTK_MODULES=gail:atk-bridge` / `QT_ACCESSIBILITY=1` for the apps you want \
                 Aleph to see), then restart those applications."
            ))
        })?;
        *shared = Some(conn.clone());
        Ok(Self { conn })
    }

    /// Drop the shared connection so the next [`Self::open`] builds a new one.
    ///
    /// Called when a query fails at the bus level, which is the observable
    /// symptom of the accessibility bus having been restarted underneath us.
    async fn invalidate_shared() {
        *shared_connection().lock().await = None;
    }

    /// The underlying zbus connection, for building interface proxies.
    pub fn connection(&self) -> &zbus::Connection {
        self.conn.connection()
    }

    /// Build an `Accessible` proxy for a child reference.
    ///
    /// Names and paths are cloned into `'static` so the proxy can outlive the
    /// reference it came from — which is what lets the tree walk recurse
    /// without threading a borrow through every level.
    ///
    /// # Errors
    ///
    /// [`DesktopError::PlatformError`] if the reference is null or the proxy
    /// cannot be built.
    pub async fn proxy_for(
        &self,
        object: &atspi::ObjectRefOwned,
    ) -> Result<AccessibleProxy<'static>> {
        let name = object
            .name()
            .ok_or_else(|| DesktopError::PlatformError("AT-SPI object has no bus name".into()))?
            .to_owned();
        let path = object.path().to_owned();
        AccessibleProxy::builder(self.connection())
            .destination(zbus::names::BusName::Unique(name))
            .and_then(|b| b.path(path))
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI proxy address invalid: {e}")))?
            // Property caching would keep a stale name/description for the life
            // of the proxy; these walks want what is true right now.
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI proxy failed: {e}")))
    }

    /// Build a proxy for another object **in the same application** as `sibling`.
    ///
    /// The bulk cache identifies objects by path alone, because every object in
    /// one `GetItems` reply belongs to that one application's bus name. This is
    /// how a cached child path becomes something to read.
    ///
    /// # Errors
    ///
    /// [`DesktopError::PlatformError`] if the path is not a valid object path or
    /// the proxy cannot be built.
    pub async fn sibling_proxy(
        &self,
        sibling: &AccessibleProxy<'static>,
        path: &str,
    ) -> Result<AccessibleProxy<'static>> {
        AccessibleProxy::builder(self.connection())
            .destination(sibling.inner().destination().to_owned())
            .and_then(|b| b.path(path.to_owned()))
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI proxy address invalid: {e}")))?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI proxy failed: {e}")))
    }

    /// Every application on the accessibility bus, with its pid.
    ///
    /// Applications that cannot be addressed (exited between the listing and
    /// the query) are skipped rather than failing the whole enumeration.
    ///
    /// # Errors
    ///
    /// [`DesktopError::PlatformError`] if the bus root cannot be listed.
    pub async fn apps(&self) -> Result<Vec<App>> {
        // The first two calls of every capability invocation, and therefore
        // where a connection that died since it was cached shows itself.
        // Dropping the shared one here means the *next* call reconnects rather
        // than inheriting a socket nobody is listening on.
        let root = match self.conn.root_accessible_on_registry().await {
            Ok(root) => root,
            Err(e) => {
                Self::invalidate_shared().await;
                return Err(DesktopError::PlatformError(format!(
                    "AT-SPI registry query failed: {e}"
                )));
            }
        };
        let children = match root.get_children().await {
            Ok(children) => children,
            Err(e) => {
                Self::invalidate_shared().await;
                return Err(DesktopError::PlatformError(format!(
                    "AT-SPI application list failed: {e}"
                )));
            }
        };

        let dbus = zbus::fdo::DBusProxy::new(self.connection())
            .await
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI bus introspection: {e}")))?;

        // Concurrent, because this is pure latency and it is paid on **every**
        // capability call before any real work starts. AT-SPI has no "pid"
        // attribute — an application's identity on the bus is its unique
        // connection name, and only the bus knows which process owns that — so
        // one round trip per application is unavoidable. Issued in sequence on a
        // desktop with ~20 applications that was ~400 ms of dead time ahead of
        // every snapshot, locator resolution and focus query.
        use futures::StreamExt as _;
        let lookups = children.into_iter().filter_map(|child| {
            let name = child.name().cloned()?;
            let dbus = &dbus;
            Some(async move {
                let pid = dbus
                    .get_connection_unix_process_id(zbus::names::BusName::Unique(name))
                    .await
                    .ok()?;
                let root = self.proxy_for(&child).await.ok()?;
                Some(App {
                    pid: i32::try_from(pid).unwrap_or(0),
                    root,
                })
            })
        });

        // Applications that cannot be addressed (they exited between the listing
        // and the query) drop out rather than failing the whole enumeration.
        Ok(futures::stream::iter(lookups)
            .buffer_unordered(APP_LOOKUP_CONCURRENCY)
            .filter_map(|app| async move { app })
            .collect()
            .await)
    }

    /// The application to act on: the one owning `pid`, or the frontmost one
    /// when `pid` is `None`.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::apps`].
    pub async fn app_for(&self, pid: Option<i32>) -> Result<Option<App>> {
        let apps = self.apps().await?;
        let wanted = match pid {
            Some(pid) => Some(pid),
            None => self.frontmost_pid_among(&apps).await,
        };
        let Some(wanted) = wanted else {
            // No focused window, nothing marked active, and no pid asked for:
            // there is no defensible "current app", and picking one arbitrarily
            // would be a guess the caller could not detect.
            return Ok(None);
        };
        Ok(apps.into_iter().find(|a| a.pid == wanted))
    }

    /// Which application is frontmost, asked of the window layer first and of
    /// AT-SPI itself second.
    ///
    /// # Why there are two sources
    ///
    /// The window layer is authoritative where it exists: it is the same source
    /// `window_list` reports, so "the frontmost app" means one thing to every
    /// desktop tool. But it does not exist everywhere. Under Wayland, only sway
    /// and Hyprland expose window management; **GNOME and KDE expose none**, so
    /// `active_window()` there is a hard `NotAvailable` — and this used to be
    /// the only source, which meant that on the two most widely deployed
    /// Wayland desktops every locator-based call (`query_focused`, `set_value`
    /// and `ax_action` without an explicit pid, `desktop_som`'s semantic mode,
    /// and with them the `type_text` focus gate) answered "no frontmost
    /// application" no matter what was on screen.
    ///
    /// AT-SPI can answer it on its own: a toolkit marks the top-level frame
    /// that has keyboard focus with [`State::Active`], and that is a property
    /// of the application, not of the compositor. orca's Linux runtime picks
    /// its target window the same way, for the same reason.
    ///
    /// The window layer stays first because it is one cheap call against a
    /// compositor that already knows, whereas this walks every application's
    /// top-level children.
    async fn frontmost_pid_among(&self, apps: &[App]) -> Option<i32> {
        if let Some(pid) = frontmost_pid_via_windows() {
            return Some(pid);
        }
        self.frontmost_pid_via_atspi(apps).await
    }

    /// The pid of the application owning a top-level frame in [`State::Active`].
    ///
    /// Only an application's direct children are examined — those are its
    /// top-level windows — so this costs one round trip per application plus one
    /// per window, not a tree walk.
    async fn frontmost_pid_via_atspi(&self, apps: &[App]) -> Option<i32> {
        for app in apps {
            let Ok(children) = app.root.get_children().await else {
                continue;
            };
            for child in children {
                let Ok(window) = self.proxy_for(&child).await else {
                    continue;
                };
                if window
                    .get_state()
                    .await
                    .is_ok_and(|s| s.contains(State::Active))
                {
                    return Some(app.pid);
                }
            }
        }
        None
    }
}

/// The pid owning the focused window, via the window layer.
fn frontmost_pid_via_windows() -> Option<i32> {
    let active = aleph_desktop::action::window_linux::active_window().ok()??;
    let windows = aleph_desktop::action::window_linux::window_list().ok()?;
    let pid = windows.iter().find(|w| w.id == active)?.pid;
    i32::try_from(pid).ok()
}

/// The frontmost application's pid, opening a bus connection of its own.
///
/// Exists for callers outside this module that have no [`Bus`] in hand — the
/// system limb's `list_running_apps`, whose `is_active` flag is what
/// `DesktopTool::check_blocked_app` reads to hard-block a password manager. That
/// flag came only from the window layer, so on GNOME/KDE Wayland it was
/// permanently `false` and the block was dead — the mirror image of the
/// multi-process fold bug fixed on the same flag in 2026-07.
///
/// Returns `None` rather than an error: a caller that cannot determine the
/// frontmost application should report no active app, not fail the listing.
pub async fn frontmost_pid() -> Option<i32> {
    if let Some(pid) = frontmost_pid_via_windows() {
        return Some(pid);
    }
    let bus = Bus::open().await.ok()?;
    let apps = bus.apps().await.ok()?;
    bus.frontmost_pid_via_atspi(&apps).await
}

/// Is an AT-SPI bus plausibly reachable in this session?
///
/// Called from `LinuxPlatform::new`, which is synchronous, so this is a
/// filesystem/environment probe rather than a connection attempt.
///
/// The distinction matters: `DesktopPlatform::ax()` returning `Some` is a claim
/// that the accessibility layer works. The `type_text` focus gate treats an AX
/// *error* as fail-open but logs a warning for it, so a capability that is
/// present and always failing is worse than one that is honestly absent — every
/// keystroke would carry a useless warning. Absent is the truthful answer on a
/// desktop with accessibility switched off.
#[must_use]
pub fn bus_looks_reachable() -> bool {
    // An explicitly configured bus address settles it.
    if std::env::var_os("AT_SPI_BUS_ADDRESS").is_some() {
        return true;
    }
    // Otherwise at-spi2-core leaves its socket under the runtime dir; its
    // presence means the bus launcher has run for this session.
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return false;
    };
    let dir = std::path::Path::new(&runtime).join("at-spi");
    std::fs::read_dir(dir).is_ok_and(|mut entries| {
        entries.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("bus")))
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reachability_probe_never_panics_and_answers_from_the_environment() {
        // Whatever this host looks like, the probe must return a bool without
        // touching the bus — it runs inside a synchronous constructor.
        let _ = bus_looks_reachable();
    }

    #[tokio::test]
    async fn opening_the_bus_either_works_or_says_how_to_enable_it() {
        match Bus::open().await {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("at-spi2-core") || msg.contains("toolkit-accessibility"),
                    "the failure must be actionable: {msg}"
                );
            }
        }
    }
}
