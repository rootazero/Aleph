//! Connecting to the AT-SPI accessibility bus and finding applications on it.
//!
//! AT-SPI is a D-Bus service: a per-session *accessibility* bus (separate from
//! the session bus) whose root object's children are the running applications
//! that have accessibility enabled. Everything here is stateless — a connection
//! is opened per capability call and dropped at the end — for the same reason
//! the Windows UI Automation limb builds a fresh `IUIAutomation` per call: an
//! element handle that outlives its call goes stale, and a locator that
//! re-resolves is worth more than a handle that has to be invalidated.

use atspi::proxy::accessible::AccessibleProxy;
use atspi::AccessibilityConnection;

use aleph_desktop::{DesktopError, Result};

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
    /// Open a connection to the accessibility bus.
    ///
    /// # Errors
    ///
    /// [`DesktopError::NotAvailable`] when the bus cannot be reached — most
    /// often because the desktop has accessibility support switched off, which
    /// is a thing a user can fix, so the message says how.
    pub async fn open() -> Result<Self> {
        let conn = AccessibilityConnection::new().await.map_err(|e| {
            DesktopError::NotAvailable(format!(
                "Cannot reach the AT-SPI accessibility bus ({e}). Install `at-spi2-core` and \
                 enable toolkit accessibility (GNOME: `gsettings set \
                 org.gnome.desktop.interface toolkit-accessibility true`; other desktops: export \
                 `GTK_MODULES=gail:atk-bridge` / `QT_ACCESSIBILITY=1` for the apps you want \
                 Aleph to see), then restart those applications."
            ))
        })?;
        Ok(Self { conn })
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

    /// Every application on the accessibility bus, with its pid.
    ///
    /// Applications that cannot be addressed (exited between the listing and
    /// the query) are skipped rather than failing the whole enumeration.
    ///
    /// # Errors
    ///
    /// [`DesktopError::PlatformError`] if the bus root cannot be listed.
    pub async fn apps(&self) -> Result<Vec<App>> {
        let root = self.conn.root_accessible_on_registry().await.map_err(|e| {
            DesktopError::PlatformError(format!("AT-SPI registry query failed: {e}"))
        })?;
        let children = root.get_children().await.map_err(|e| {
            DesktopError::PlatformError(format!("AT-SPI application list failed: {e}"))
        })?;

        let dbus = zbus::fdo::DBusProxy::new(self.connection())
            .await
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI bus introspection: {e}")))?;

        let mut apps = Vec::with_capacity(children.len());
        for child in children {
            let Some(name) = child.name().cloned() else {
                continue;
            };
            // AT-SPI has no "pid" attribute; the application's identity on the
            // bus is its unique connection name, and the bus itself knows which
            // process owns that.
            let Ok(pid) = dbus
                .get_connection_unix_process_id(zbus::names::BusName::Unique(name))
                .await
            else {
                continue;
            };
            let Ok(root) = self.proxy_for(&child).await else {
                continue;
            };
            apps.push(App {
                pid: i32::try_from(pid).unwrap_or(0),
                root,
            });
        }
        Ok(apps)
    }

    /// The application to act on: the one owning `pid`, or the one owning the
    /// focused window when `pid` is `None`.
    ///
    /// "Frontmost" is not an AT-SPI concept — the accessibility bus has no
    /// notion of stacking. It comes from the window layer instead, which is the
    /// same source `window_list` reports, so "the frontmost app" means the same
    /// thing to every desktop tool.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::apps`].
    pub async fn app_for(&self, pid: Option<i32>) -> Result<Option<App>> {
        let apps = self.apps().await?;
        let wanted = match pid {
            Some(pid) => Some(pid),
            None => frontmost_pid(),
        };
        let Some(wanted) = wanted else {
            // No focused window and no pid asked for: there is no defensible
            // "current app", and picking one arbitrarily would be a guess the
            // caller could not detect.
            return Ok(None);
        };
        Ok(apps.into_iter().find(|a| a.pid == wanted))
    }
}

/// The pid owning the focused window, via the window layer.
fn frontmost_pid() -> Option<i32> {
    let active = aleph_desktop::action::window_linux::active_window().ok()??;
    let windows = aleph_desktop::action::window_linux::window_list().ok()?;
    let pid = windows.iter().find(|w| w.id == active)?.pid;
    i32::try_from(pid).ok()
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
