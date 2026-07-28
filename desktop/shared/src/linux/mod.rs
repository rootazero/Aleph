//! Linux-specific shared plumbing: one source of truth for "what kind of
//! session are we in, and which helper binaries exist".
//!
//! Everything Linux does at the OS boundary — input injection, window
//! management, clipboard, screen capture, idle detection, permission mapping —
//! branches on the same two facts:
//!
//! 1. **Which display server / compositor is running** (X11 vs Wayland, and
//!    under Wayland *which* compositor, because window management on Wayland
//!    has no cross-compositor protocol).
//! 2. **Which helper binary is on `PATH`** (`xdotool` / `wmctrl` / `xclip` /
//!    `wl-copy` / `ydotool` / …), because a Linux desktop ships an arbitrary
//!    subset of them.
//!
//! Before this module those two facts were re-derived in four places, each
//! with a slightly different rule, and the clipboard derived them *implicitly*
//! from the order in which it tried to spawn processes — which is why a box
//! with `wl-clipboard` installed under X11 silently wrote to nothing.
//!
//! Both detections are cached in a `OnceLock`: they describe the session the
//! daemon is attached to, which does not change while the process lives.
//!
//! The classification logic is pure (it takes the environment as data) so the
//! full matrix is unit-testable on any host, mirroring the way
//! `action::wayland_input` keeps its argv builders host-testable.

pub mod app;
pub mod clipboard;
pub mod proc;
pub mod session;

pub use session::{
    find_on_path, missing_tool_error, session, tools, Compositor, LinuxSession, SessionEnv,
    SessionKind, ToolBox,
};
