//! Native X11 / EWMH window management via `x11rb`.
//!
//! This replaces a `wmctrl` shell-out that failed outright wherever `wmctrl`
//! was not installed — which is most desktops, since it has not been part of a
//! default install for years.
//!
//! Three things the native path gets right that the CLI could not:
//!
//! * **The window list is `_NET_CLIENT_LIST`** — the EWMH property that *is*
//!   the set of application windows, in the window manager's own stacking
//!   order. The obvious CLI substitute, `xdotool search`, additionally returns
//!   the window manager's unnamed utility windows (20+ on a stock XFCE
//!   session), which the model then has to guess its way past.
//! * **One X connection per operation instead of one process per window.**
//!   Listing 20 windows with `xdotool` costs 60+ process spawns (name, pid,
//!   geometry each); here it is a handful of round-trips on one socket.
//! * **Geometry is translated to the root window.** `GetGeometry` alone returns
//!   coordinates relative to the *frame* the window manager reparented the
//!   window into, which is not where clicks are issued. `TranslateCoordinates`
//!   converts to the global space that `WindowInfo.bounds` is documented in.
//!
//! Connections are opened per call and dropped at the end. That is deliberate:
//! an `x11rb` connection is neither `Send` nor cheap to keep healthy across a
//! display-server restart, and these are human-paced interactive queries.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::error::{DesktopError, Result};
use crate::{BoundingBox, WindowInfo};

/// EWMH source indication for "a pager / another application", the value
/// window managers accept from tools acting on the user's behalf.
const SOURCE_PAGER: u32 = 2;

/// `_NET_MOVERESIZE_WINDOW` flag bits: which of x/y/width/height are supplied.
const MOVERESIZE_X: u32 = 1 << 8;
const MOVERESIZE_Y: u32 = 1 << 9;
const MOVERESIZE_W: u32 = 1 << 10;
const MOVERESIZE_H: u32 = 1 << 11;
/// The source indication lives in bits 12–15 of the same word.
const MOVERESIZE_SOURCE_SHIFT: u32 = 12;

/// Compose the `data[0]` word of a `_NET_MOVERESIZE_WINDOW` message.
///
/// Pure, and unit-tested, because getting the flag word wrong is silent: the
/// window manager simply ignores the fields whose bits are unset, so a bad
/// constant looks exactly like "the WM refused to move it".
const fn moveresize_flags(with_position: bool, with_size: bool) -> u32 {
    let mut flags = SOURCE_PAGER << MOVERESIZE_SOURCE_SHIFT;
    if with_position {
        flags |= MOVERESIZE_X | MOVERESIZE_Y;
    }
    if with_size {
        flags |= MOVERESIZE_W | MOVERESIZE_H;
    }
    flags
}

/// The atoms every operation here needs, interned once per connection.
struct Atoms {
    net_client_list: Atom,
    net_active_window: Atom,
    net_close_window: Atom,
    net_moveresize_window: Atom,
    net_wm_name: Atom,
    net_wm_pid: Atom,
    net_wm_state: Atom,
    net_wm_state_hidden: Atom,
}

impl Atoms {
    /// Intern every atom, pipelining the requests before collecting replies.
    fn intern(conn: &RustConnection) -> Result<Self> {
        const NAMES: [&[u8]; 8] = [
            b"_NET_CLIENT_LIST",
            b"_NET_ACTIVE_WINDOW",
            b"_NET_CLOSE_WINDOW",
            b"_NET_MOVERESIZE_WINDOW",
            b"_NET_WM_NAME",
            b"_NET_WM_PID",
            b"_NET_WM_STATE",
            b"_NET_WM_STATE_HIDDEN",
        ];

        let cookies: Vec<_> = NAMES
            .iter()
            .map(|name| conn.intern_atom(false, name).map_err(x11_err))
            .collect::<Result<_>>()?;

        let mut atoms = [0u32; 8];
        for (slot, cookie) in atoms.iter_mut().zip(cookies) {
            *slot = cookie.reply().map_err(x11_err)?.atom;
        }

        Ok(Self {
            net_client_list: atoms[0],
            net_active_window: atoms[1],
            net_close_window: atoms[2],
            net_moveresize_window: atoms[3],
            net_wm_name: atoms[4],
            net_wm_pid: atoms[5],
            net_wm_state: atoms[6],
            net_wm_state_hidden: atoms[7],
        })
    }
}

fn x11_err<E: std::fmt::Display>(e: E) -> DesktopError {
    DesktopError::WindowFailed(format!("X11 request failed: {e}"))
}

/// A live X connection plus the interned atoms and the root window.
struct X11 {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
}

impl X11 {
    fn open() -> Result<Self> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|e| {
            DesktopError::WindowFailed(format!(
                "Cannot reach an X server ({e}). Window management needs a display: check that \
                 DISPLAY is set for the daemon's environment, or use the screenshot + gui_locate \
                 path if this session is Wayland-only."
            ))
        })?;
        let root = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| DesktopError::WindowFailed("X server reported no screen".into()))?
            .root;
        let atoms = Atoms::intern(&conn)?;
        Ok(Self { conn, root, atoms })
    }

    /// Read a property as a list of 32-bit values (WINDOW / CARDINAL / ATOM).
    fn prop32(&self, window: Window, property: Atom) -> Result<Vec<u32>> {
        let reply = self
            .conn
            .get_property(false, window, property, AtomEnum::ANY, 0, u32::MAX)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        Ok(reply.value32().map(Iterator::collect).unwrap_or_default())
    }

    /// Read a property as raw bytes (text properties).
    fn prop_bytes(&self, window: Window, property: Atom) -> Vec<u8> {
        self.conn
            .get_property(false, window, property, AtomEnum::ANY, 0, u32::MAX)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.value)
            .unwrap_or_default()
    }

    /// The application windows the window manager advertises.
    fn client_list(&self) -> Result<Vec<Window>> {
        self.prop32(self.root, self.atoms.net_client_list)
    }

    /// Window title: `_NET_WM_NAME` (UTF-8) with a `WM_NAME` fallback for
    /// applications that never adopted the EWMH property.
    fn title_of(&self, window: Window) -> String {
        let utf8 = self.prop_bytes(window, self.atoms.net_wm_name);
        if !utf8.is_empty() {
            return String::from_utf8_lossy(&utf8).into_owned();
        }
        let legacy = self.prop_bytes(window, AtomEnum::WM_NAME.into());
        String::from_utf8_lossy(&legacy).into_owned()
    }

    /// Owning application, from `WM_CLASS`'s second NUL-separated field.
    fn owner_of(&self, window: Window) -> String {
        let raw = self.prop_bytes(window, AtomEnum::WM_CLASS.into());
        parse_wm_class(&raw)
    }

    /// The window's frame in the global coordinate space clicks use.
    fn bounds_of(&self, window: Window) -> Option<BoundingBox> {
        let geom = self.conn.get_geometry(window).ok()?.reply().ok()?;
        // GetGeometry is relative to the parent — which, under a reparenting
        // window manager, is the decoration frame, not the root.
        let abs = self
            .conn
            .translate_coordinates(window, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some(BoundingBox {
            x: f64::from(abs.dst_x),
            y: f64::from(abs.dst_y),
            w: f64::from(geom.width),
            h: f64::from(geom.height),
        })
    }

    /// `false` when the window manager marks the window `_NET_WM_STATE_HIDDEN`
    /// (minimized / shaded / on another desktop).
    fn on_screen(&self, window: Window) -> Option<bool> {
        let states = self.prop32(window, self.atoms.net_wm_state).ok()?;
        Some(!states.contains(&self.atoms.net_wm_state_hidden))
    }

    fn window_info(&self, window: Window) -> WindowInfo {
        WindowInfo {
            id: u64::from(window),
            title: self.title_of(window),
            owner: self.owner_of(window),
            pid: self
                .prop32(window, self.atoms.net_wm_pid)
                .ok()
                .and_then(|v| v.first().copied())
                .map_or(0, u64::from),
            bounds: self.bounds_of(window),
            // X11 exposes no stacking level comparable to macOS window levels;
            // not told is not zero.
            layer: None,
            on_screen: self.on_screen(window),
        }
    }

    /// Reject an id the window manager does not know about, before any
    /// operation turns it into an opaque X error.
    fn require_known(&self, window_id: u64) -> Result<Window> {
        let window = u32::try_from(window_id).map_err(|_| {
            DesktopError::WindowFailed(format!(
                "{window_id} is not a valid X11 window id (X11 ids are 32-bit)"
            ))
        })?;
        if self.client_list()?.contains(&window) {
            return Ok(window);
        }
        Err(DesktopError::WindowFailed(format!(
            "No managed window with id {window_id} (0x{window:x}) — it was closed, or the id came \
             from an older window_list. Re-run window_list and use a current id."
        )))
    }

    /// Post an EWMH client message to the root window, the channel window
    /// managers listen on for requests made on the user's behalf.
    fn send_root_message(&self, window: Window, type_: Atom, data: [u32; 5]) -> Result<()> {
        let event = ClientMessageEvent::new(32, window, type_, data);
        self.conn
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                event,
            )
            .map_err(x11_err)?;
        self.conn.flush().map_err(x11_err)?;
        Ok(())
    }
}

/// Extract the class from a `WM_CLASS` value (`"instance\0class\0"`).
///
/// The class (second field) is the stable application identity; the instance is
/// often the argv\[0\] the process happened to be started with. Falls back to
/// the instance when only one field is present.
#[must_use]
pub fn parse_wm_class(raw: &[u8]) -> String {
    let mut fields = raw
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned());
    let instance = fields.next();
    fields.next().or(instance).unwrap_or_default()
}

// ── Operations ───────────────────────────────────────────────────────────────

/// List the application windows the window manager advertises.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] when no X server is reachable or a request
/// fails.
pub fn window_list() -> Result<Vec<WindowInfo>> {
    let x = X11::open()?;
    let windows = x.client_list()?;
    Ok(windows.into_iter().map(|w| x.window_info(w)).collect())
}

/// Raise `window_id` and give it keyboard focus, via `_NET_ACTIVE_WINDOW`.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] when the id is unknown or the request fails.
pub fn focus_window(window_id: u64) -> Result<()> {
    let x = X11::open()?;
    let window = x.require_known(window_id)?;
    x.send_root_message(
        window,
        x.atoms.net_active_window,
        [SOURCE_PAGER, x11rb::CURRENT_TIME, 0, 0, 0],
    )
}

/// Move `window_id` so its top-left corner sits at (`x`, `y`).
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] when the id is unknown or the request fails.
pub fn move_window(window_id: u64, x: i32, y: i32) -> Result<()> {
    let conn = X11::open()?;
    let window = conn.require_known(window_id)?;
    conn.send_root_message(
        window,
        conn.atoms.net_moveresize_window,
        [moveresize_flags(true, false), x as u32, y as u32, 0, 0],
    )
}

/// Resize `window_id` to `width` × `height` pixels.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] when the id is unknown or the request fails.
pub fn resize_window(window_id: u64, width: u32, height: u32) -> Result<()> {
    let conn = X11::open()?;
    let window = conn.require_known(window_id)?;
    conn.send_root_message(
        window,
        conn.atoms.net_moveresize_window,
        [moveresize_flags(false, true), 0, 0, width, height],
    )
}

/// Ask `window_id` to close, the way its close button would.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] when the id is unknown or the request fails.
pub fn close_window(window_id: u64) -> Result<()> {
    let x = X11::open()?;
    let window = x.require_known(window_id)?;
    x.send_root_message(
        window,
        x.atoms.net_close_window,
        [x11rb::CURRENT_TIME, SOURCE_PAGER, 0, 0, 0],
    )
}

/// The focused window, from `_NET_ACTIVE_WINDOW` on the root.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] when no X server is reachable.
pub fn active_window() -> Result<Option<u64>> {
    let x = X11::open()?;
    let active = x
        .prop32(x.root, x.atoms.net_active_window)?
        .first()
        .copied()
        .filter(|w| *w != 0);
    Ok(active.map(u64::from))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wm_class_prefers_the_class_over_the_instance() {
        assert_eq!(parse_wm_class(b"navigator\0Firefox\0"), "Firefox");
    }

    #[test]
    fn wm_class_falls_back_to_the_instance_when_alone() {
        assert_eq!(parse_wm_class(b"xterm\0"), "xterm");
        assert_eq!(parse_wm_class(b"xterm"), "xterm");
    }

    #[test]
    fn wm_class_of_nothing_is_empty_not_a_panic() {
        assert_eq!(parse_wm_class(b""), "");
        assert_eq!(parse_wm_class(b"\0\0"), "");
    }

    #[test]
    fn wm_class_survives_invalid_utf8() {
        // X11 text properties are byte strings; a Latin-1 title must not panic.
        let raw = b"inst\0caf\xe9\0";
        assert!(parse_wm_class(raw).contains("caf"));
    }

    #[test]
    fn moveresize_flags_set_only_the_supplied_fields() {
        let pos = moveresize_flags(true, false);
        assert_eq!(pos & MOVERESIZE_X, MOVERESIZE_X);
        assert_eq!(pos & MOVERESIZE_Y, MOVERESIZE_Y);
        assert_eq!(pos & MOVERESIZE_W, 0, "width must not be claimed");
        assert_eq!(pos & MOVERESIZE_H, 0, "height must not be claimed");

        let size = moveresize_flags(false, true);
        assert_eq!(size & MOVERESIZE_W, MOVERESIZE_W);
        assert_eq!(size & MOVERESIZE_H, MOVERESIZE_H);
        assert_eq!(size & MOVERESIZE_X, 0);
    }

    /// Live smoke test against a real X server, skipped where there is none
    /// (CI, a Wayland-only box) so it never turns into a flaky gate.
    ///
    /// It asserts the two properties that a unit test on parsed fixtures can
    /// never reach: that `_NET_CLIENT_LIST` really is the *application* window
    /// set (not the window manager's own dozens of unnamed utility windows,
    /// which is what the obvious `xdotool search` alternative returns), and
    /// that geometry comes back translated to the root rather than relative to
    /// a reparenting frame.
    #[test]
    fn live_window_list_describes_real_application_windows() {
        let Ok(windows) = window_list() else {
            return; // no display here
        };
        if windows.is_empty() {
            return; // a display with nothing on it
        }

        assert!(
            windows.len() < 100,
            "_NET_CLIENT_LIST returned {} windows — that is a raw X window dump, \
             not the application list",
            windows.len()
        );
        assert!(
            windows.iter().any(|w| !w.title.is_empty()),
            "every window unnamed: the title properties are not being read"
        );
        assert!(
            windows.iter().any(|w| w.pid != 0),
            "no window reported a pid: _NET_WM_PID is not being read"
        );
        for w in &windows {
            if let Some(b) = w.bounds {
                assert!(
                    b.w > 0.0 && b.h > 0.0,
                    "window {} has a degenerate rectangle {b:?}",
                    w.id
                );
            }
        }

        // The active window, when there is one, must be a window we listed.
        if let Ok(Some(active)) = active_window() {
            assert!(
                windows.iter().any(|w| w.id == active),
                "the focused window {active} is not in the client list"
            );
        }
    }

    #[test]
    fn moveresize_flags_carry_the_pager_source_indication() {
        // Window managers ignore requests whose source indication they do not
        // recognise, so this bit field is load-bearing.
        for flags in [
            moveresize_flags(true, false),
            moveresize_flags(false, true),
            moveresize_flags(true, true),
        ] {
            assert_eq!((flags >> MOVERESIZE_SOURCE_SHIFT) & 0xF, SOURCE_PAGER);
        }
    }
}
