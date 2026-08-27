//! Linux window management: backend selection and dispatch.
//!
//! # Why there is more than one backend
//!
//! X11 has EWMH — a cross-window-manager protocol every WM implements — so a
//! single backend serves every X11 desktop. Wayland deliberately has no
//! equivalent: window management is the compositor's private business, and each
//! compositor exposes (or withholds) its own IPC. So "list the windows" needs
//! one implementation per compositor, and on the compositors that expose
//! nothing it is honestly unavailable.
//!
//! # What replaced what
//!
//! The previous implementation shelled out to `wmctrl` and nothing else. That
//! meant every window operation failed outright on the (many) machines without
//! it, and on every Wayland session regardless. It is gone:
//!
//! * **X11** is now native, over `x11rb` — the same pure-Rust X11 client
//!   already in the dependency graph via `xcap`. No external binary, no
//!   subprocess per window, and the window list comes from `_NET_CLIENT_LIST`,
//!   which is exactly the set of application windows a user sees. (`xdotool
//!   search`, the obvious CLI alternative, also returns the window manager's
//!   own unnamed utility windows — 20+ of them on a plain XFCE desktop.)
//! * **sway / Hyprland** speak documented JSON IPC (`swaymsg` / `hyprctl`),
//!   both of which ship with the compositor itself.
//!
//! # Deliberately not covered
//!
//! * **GNOME on Wayland** exposes no window-management interface to
//!   unprivileged clients at all (it needs a shell extension). Reporting that
//!   plainly is better than pretending.
//! * **KDE on Wayland** can be driven through KWin's scripting D-Bus, which is
//!   what the third-party `kdotool` wraps. It is not wired here because its
//!   window handles are opaque UUID strings, which cannot be carried by the
//!   cross-platform `WindowInfo.id: u64` contract without a stateful
//!   handle map — precisely the cross-call handle pattern the AX layer was
//!   designed to avoid. Screen capture, `gui_locate` and the AT-SPI layer all
//!   work there, so the desktop is not unreachable, only its window frames are.

pub mod hyprland;
pub mod sway;
#[cfg(target_os = "linux")]
pub mod x11;

use crate::error::{DesktopError, Result};
use crate::linux::session::{Compositor, LinuxSession, SessionKind, ToolBox};
use crate::linux::{session, tools};
use crate::WindowInfo;

/// Which mechanism drives window management in this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowBackend {
    /// Native X11 / EWMH via `x11rb`. Serves every X11 desktop environment.
    X11,
    /// sway (and other wlroots compositors speaking the i3 IPC) via `swaymsg`.
    Sway,
    /// Hyprland via `hyprctl`.
    Hyprland,
}

impl WindowBackend {
    /// Name used in error messages and in the `backend` field of results.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X11 => "x11",
            Self::Sway => "sway",
            Self::Hyprland => "hyprland",
        }
    }
}

/// Choose the window backend for a session, or explain why there is none.
///
/// Pure: it takes the session and the tool inventory as data, so the whole
/// matrix is unit-testable on any host.
///
/// # Errors
///
/// [`DesktopError::NotAvailable`] when the session has no reachable window
/// management interface, with a message that says *why* and what still works.
pub fn pick_backend(session: LinuxSession, tb: &ToolBox) -> Result<WindowBackend> {
    match session.kind {
        // X11 (and an unknown session, which is far more likely to have a
        // reachable X server via DISPLAY than a Wayland compositor): the native
        // EWMH client needs no external tool at all.
        SessionKind::X11 | SessionKind::Unknown => Ok(WindowBackend::X11),

        SessionKind::Wayland => match session.compositor {
            Compositor::Sway if tb.has("swaymsg") => Ok(WindowBackend::Sway),
            Compositor::Sway => Err(DesktopError::NotAvailable(
                "This is a sway session but `swaymsg` is not on PATH — it ships with sway; \
                 check the daemon's PATH."
                    .into(),
            )),
            Compositor::Hyprland if tb.has("hyprctl") => Ok(WindowBackend::Hyprland),
            Compositor::Hyprland => Err(DesktopError::NotAvailable(
                "This is a Hyprland session but `hyprctl` is not on PATH — it ships with \
                 Hyprland; check the daemon's PATH."
                    .into(),
            )),
            Compositor::Gnome | Compositor::Kde | Compositor::Other => {
                Err(DesktopError::NotAvailable(format!(
                    "Window management is unavailable on {} under Wayland: the compositor \
                     exposes no window-management interface to ordinary clients (Wayland has no \
                     cross-compositor equivalent of X11's EWMH). Screenshots, gui_locate and the \
                     accessibility layer all still work — locate and click by pixel or by \
                     element instead of by window.",
                    compositor_label(session.compositor)
                )))
            }
        },
    }
}

const fn compositor_label(c: Compositor) -> &'static str {
    match c {
        Compositor::Sway => "sway",
        Compositor::Hyprland => "Hyprland",
        Compositor::Kde => "KDE Plasma",
        Compositor::Gnome => "GNOME",
        Compositor::Other => "this desktop",
    }
}

/// The backend for the live session.
fn backend() -> Result<WindowBackend> {
    pick_backend(session(), tools())
}

// ── Public operations ────────────────────────────────────────────────────────

/// List the application windows currently on screen.
///
/// # Errors
///
/// Propagates [`pick_backend`], or the backend's own failure.
pub fn window_list() -> Result<Vec<WindowInfo>> {
    match backend()? {
        WindowBackend::X11 => x11_list(),
        WindowBackend::Sway => sway::window_list(),
        WindowBackend::Hyprland => hyprland::window_list(),
    }
}

/// Bring `window_id` to the front and give it keyboard focus.
///
/// A focus *request* the WM accepted is not the window focused: X11's
/// `_NET_ACTIVE_WINDOW` client message is a hint the WM may refuse or redirect,
/// and reporting success anyway is how the model ends up typing into whatever
/// the user is actually looking at. So after dispatching, the active window is
/// polled on the same 500 ms cadence the Windows and macOS limbs use
/// (`action::window`'s foreground poll), and a window that never becomes active
/// is an honest error, not a delivered request.
///
/// # Errors
///
/// Propagates [`backend`]'s selection failure and the backend's own dispatch
/// failure; returns [`DesktopError::WindowFailed`] when the window did not
/// become active within the wait, or when the outcome could not be read back.
pub fn focus_window(window_id: u64) -> Result<()> {
    match backend()? {
        WindowBackend::X11 => x11_focus(window_id),
        WindowBackend::Sway => sway::focus_window(window_id),
        WindowBackend::Hyprland => hyprland::focus_window(window_id),
    }?;

    let deadline = std::time::Instant::now() + FOCUS_WAIT;
    loop {
        match active_window() {
            Ok(Some(id)) if id == window_id => return Ok(()),
            Ok(_) => {}
            Err(e) => {
                // The dispatch succeeded but the outcome is unreadable —
                // "delivered, unconfirmed", which must not read as success.
                return Err(DesktopError::WindowFailed(format!(
                    "focus request for window {window_id} was delivered, but the focused window \
                     could not be read back ({e}) — treat focus as unconfirmed."
                )));
            }
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(FOCUS_POLL);
    }
    Err(DesktopError::WindowFailed(format!(
        "window {window_id} did not become the active window within {} ms of the focus request — \
         the window manager refused or redirected it. Address the app directly with set_value / \
         ax_action (they need no focus), or screenshot with its window_id to look at it without \
         raising it.",
        FOCUS_WAIT.as_millis()
    )))
}

/// How long to wait for the WM to actually focus a window after the request,
/// and how often to re-read the active window. The same cadence the Windows
/// and macOS limbs poll on (`action::window`'s `FOREGROUND_WAIT` /
/// `FOREGROUND_POLL`), duplicated here because those consts are `cfg`'d to
/// their platforms.
const FOCUS_WAIT: std::time::Duration = std::time::Duration::from_millis(500);
const FOCUS_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// Move `window_id` so its top-left corner sits at (`x`, `y`).
///
/// # Errors
///
/// Propagates [`pick_backend`], or the backend's own failure.
pub fn move_window(window_id: u64, x: i32, y: i32) -> Result<()> {
    match backend()? {
        WindowBackend::X11 => x11_move(window_id, x, y),
        WindowBackend::Sway => sway::move_window(window_id, x, y),
        WindowBackend::Hyprland => hyprland::move_window(window_id, x, y),
    }
}

/// Resize `window_id` to `width` × `height` pixels.
///
/// # Errors
///
/// Propagates [`pick_backend`], or the backend's own failure.
pub fn resize_window(window_id: u64, width: u32, height: u32) -> Result<()> {
    match backend()? {
        WindowBackend::X11 => x11_resize(window_id, width, height),
        WindowBackend::Sway => sway::resize_window(window_id, width, height),
        WindowBackend::Hyprland => hyprland::resize_window(window_id, width, height),
    }
}

/// Ask `window_id` to close the way clicking its close button would — the
/// application gets to run its own shutdown path (save prompts included).
///
/// This is what `quit_app` reaches for before falling back to a signal.
///
/// # Errors
///
/// Propagates [`pick_backend`], or the backend's own failure.
pub fn close_window(window_id: u64) -> Result<()> {
    match backend()? {
        WindowBackend::X11 => x11_close(window_id),
        WindowBackend::Sway => sway::close_window(window_id),
        WindowBackend::Hyprland => hyprland::close_window(window_id),
    }
}

/// The window that currently has focus, if any.
///
/// # Errors
///
/// Propagates [`pick_backend`], or the backend's own failure.
pub fn active_window() -> Result<Option<u64>> {
    match backend()? {
        WindowBackend::X11 => x11_active(),
        WindowBackend::Sway => sway::active_window(),
        WindowBackend::Hyprland => hyprland::active_window(),
    }
}

// ── X11 arm: native on Linux, unreachable stub elsewhere ─────────────────────
//
// The dispatch above is compiled on every host so the pure backend-selection
// logic and the sway/Hyprland argv builders stay testable, but `x11rb` is a
// Linux-only dependency. These thin shims keep the `cfg` in one place instead
// of sprinkling it through every operation.

#[cfg(target_os = "linux")]
mod x11_arm {
    use super::{Result, WindowInfo};

    pub fn list() -> Result<Vec<WindowInfo>> {
        super::x11::window_list()
    }
    pub fn focus(id: u64) -> Result<()> {
        super::x11::focus_window(id)
    }
    pub fn move_to(id: u64, x: i32, y: i32) -> Result<()> {
        super::x11::move_window(id, x, y)
    }
    pub fn resize(id: u64, w: u32, h: u32) -> Result<()> {
        super::x11::resize_window(id, w, h)
    }
    pub fn close(id: u64) -> Result<()> {
        super::x11::close_window(id)
    }
    pub fn active() -> Result<Option<u64>> {
        super::x11::active_window()
    }
}

#[cfg(not(target_os = "linux"))]
mod x11_arm {
    use super::{DesktopError, Result, WindowInfo};

    fn nope<T>() -> Result<T> {
        Err(DesktopError::NotImplemented(
            "X11 window management is only compiled on Linux".into(),
        ))
    }

    pub fn list() -> Result<Vec<WindowInfo>> {
        nope()
    }
    pub fn focus(_id: u64) -> Result<()> {
        nope()
    }
    pub fn move_to(_id: u64, _x: i32, _y: i32) -> Result<()> {
        nope()
    }
    pub fn resize(_id: u64, _w: u32, _h: u32) -> Result<()> {
        nope()
    }
    pub fn close(_id: u64) -> Result<()> {
        nope()
    }
    pub fn active() -> Result<Option<u64>> {
        nope()
    }
}

use x11_arm::{
    active as x11_active, close as x11_close, focus as x11_focus, list as x11_list,
    move_to as x11_move, resize as x11_resize,
};

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(kind: SessionKind, compositor: Compositor) -> LinuxSession {
        LinuxSession { kind, compositor }
    }

    #[test]
    fn x11_needs_no_external_tool() {
        // The whole point of going native: an empty toolbox still works on X11.
        let empty = ToolBox::from_names(&[]);
        for compositor in [
            Compositor::Gnome,
            Compositor::Kde,
            Compositor::Other,
            Compositor::Sway,
        ] {
            assert_eq!(
                pick_backend(sess(SessionKind::X11, compositor), &empty).unwrap(),
                WindowBackend::X11,
                "{compositor:?}"
            );
        }
    }

    #[test]
    fn unknown_session_tries_x11_rather_than_giving_up() {
        let empty = ToolBox::from_names(&[]);
        assert_eq!(
            pick_backend(sess(SessionKind::Unknown, Compositor::Other), &empty).unwrap(),
            WindowBackend::X11
        );
    }

    #[test]
    fn wayland_compositors_map_to_their_own_ipc() {
        let tb = ToolBox::from_names(&["swaymsg", "hyprctl"]);
        assert_eq!(
            pick_backend(sess(SessionKind::Wayland, Compositor::Sway), &tb).unwrap(),
            WindowBackend::Sway
        );
        assert_eq!(
            pick_backend(sess(SessionKind::Wayland, Compositor::Hyprland), &tb).unwrap(),
            WindowBackend::Hyprland
        );
    }

    #[test]
    fn wayland_compositor_without_its_cli_says_which_binary_is_missing() {
        let empty = ToolBox::from_names(&[]);
        let err = pick_backend(sess(SessionKind::Wayland, Compositor::Sway), &empty).unwrap_err();
        assert!(err.to_string().contains("swaymsg"), "{err}");

        let err =
            pick_backend(sess(SessionKind::Wayland, Compositor::Hyprland), &empty).unwrap_err();
        assert!(err.to_string().contains("hyprctl"), "{err}");
    }

    #[test]
    fn gnome_and_kde_wayland_explain_themselves_and_name_the_way_out() {
        let tb = ToolBox::from_names(&["swaymsg", "hyprctl"]);
        for compositor in [Compositor::Gnome, Compositor::Kde, Compositor::Other] {
            let err = pick_backend(sess(SessionKind::Wayland, compositor), &tb).unwrap_err();
            let msg = err.to_string();
            // Must not be a bare "unavailable": the model needs an alternative.
            assert!(msg.contains("gui_locate"), "{compositor:?}: {msg}");
            assert!(msg.contains("Wayland"), "{compositor:?}: {msg}");
        }
        // And it must name the desktop rather than say "this platform".
        let err = pick_backend(sess(SessionKind::Wayland, Compositor::Gnome), &tb).unwrap_err();
        assert!(err.to_string().contains("GNOME"), "{err}");
    }

    #[test]
    fn backend_names_are_stable() {
        assert_eq!(WindowBackend::X11.as_str(), "x11");
        assert_eq!(WindowBackend::Sway.as_str(), "sway");
        assert_eq!(WindowBackend::Hyprland.as_str(), "hyprland");
    }
}
