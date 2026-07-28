//! Session + helper-binary detection: the single source of truth.
//!
//! See the module doc on [`super`] for why this exists.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use crate::error::DesktopError;

// ── Session classification (pure) ────────────────────────────────────────────

/// Which display server the current session runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// X11 (or XWayland presented as X11 to this process). Synthetic input via
    /// XTEST works, screen capture needs no prompt, EWMH window management works.
    X11,
    /// A native Wayland session. Global input injection and window management
    /// are compositor-mediated; screen capture goes through xdg-desktop-portal.
    Wayland,
    /// No display server detected (headless, systemd service without a seat).
    Unknown,
}

impl SessionKind {
    /// `true` for a native Wayland session — the condition under which the
    /// XTEST/`enigo` input path cannot work and `ydotool` must be used.
    #[must_use]
    pub const fn is_wayland(self) -> bool {
        matches!(self, Self::Wayland)
    }
}

/// Which Wayland compositor (or X11 desktop environment) is in charge.
///
/// This matters only for window management: X11 has EWMH, a cross-WM protocol
/// every window manager implements, so one backend serves all of X11. Wayland
/// deliberately has **no** equivalent — each compositor exposes its own IPC, so
/// "list/focus/move a window" needs a per-compositor backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compositor {
    /// sway (and other wlroots compositors speaking the i3 IPC via `swaymsg`).
    Sway,
    /// Hyprland (`hyprctl`).
    Hyprland,
    /// KDE Plasma (`kdotool` under Wayland, EWMH under X11).
    Kde,
    /// GNOME / Mutter. Under Wayland it exposes no window-management IPC to
    /// unprivileged clients without a shell extension.
    Gnome,
    /// Some other desktop (XFCE, MATE, Cinnamon, …). Under X11 these all speak
    /// EWMH, so the X11 backends serve them.
    Other,
}

/// The environment inputs the classification reads, as data.
///
/// Taking these as a struct instead of calling `std::env::var` inline is what
/// makes the whole matrix testable without mutating process-global state (env
/// mutation in tests races with every other test in the binary).
#[derive(Debug, Default, Clone, Copy)]
pub struct SessionEnv<'a> {
    pub xdg_session_type: Option<&'a str>,
    pub wayland_display: Option<&'a str>,
    pub display: Option<&'a str>,
    pub xdg_current_desktop: Option<&'a str>,
    pub swaysock: Option<&'a str>,
    pub hyprland_signature: Option<&'a str>,
    pub kde_full_session: Option<&'a str>,
}

/// The detected session: display server + compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxSession {
    pub kind: SessionKind,
    pub compositor: Compositor,
}

/// Classify a session from its environment.
///
/// Precedence for the display server mirrors the freedesktop convention:
/// `XDG_SESSION_TYPE` is authoritative when it says something we recognise;
/// otherwise the presence of `WAYLAND_DISPLAY` wins over `DISPLAY`, because a
/// Wayland session commonly also exports `DISPLAY` for XWayland clients while
/// the reverse never happens.
#[must_use]
pub fn classify(env: &SessionEnv<'_>) -> LinuxSession {
    let kind = classify_kind(env);
    LinuxSession {
        kind,
        compositor: classify_compositor(env),
    }
}

fn classify_kind(env: &SessionEnv<'_>) -> SessionKind {
    if let Some(t) = env.xdg_session_type {
        if t.eq_ignore_ascii_case("wayland") {
            return SessionKind::Wayland;
        }
        if t.eq_ignore_ascii_case("x11") {
            return SessionKind::X11;
        }
    }
    // A Wayland session usually exports DISPLAY too (for XWayland), so
    // WAYLAND_DISPLAY must be checked first or every Wayland box reads as X11.
    if env.wayland_display.is_some_and(|v| !v.is_empty()) {
        return SessionKind::Wayland;
    }
    if env.display.is_some_and(|v| !v.is_empty()) {
        return SessionKind::X11;
    }
    SessionKind::Unknown
}

fn classify_compositor(env: &SessionEnv<'_>) -> Compositor {
    // Compositor-specific IPC handles are the strongest signal: they exist only
    // when that compositor is actually running and reachable.
    if env.hyprland_signature.is_some_and(|v| !v.is_empty()) {
        return Compositor::Hyprland;
    }
    if env.swaysock.is_some_and(|v| !v.is_empty()) {
        return Compositor::Sway;
    }
    if env.kde_full_session.is_some_and(|v| !v.is_empty()) {
        return Compositor::Kde;
    }
    // XDG_CURRENT_DESKTOP is a colon-separated list ("ubuntu:GNOME").
    if let Some(desktops) = env.xdg_current_desktop {
        for entry in desktops.split(':') {
            let entry = entry.trim();
            if entry.eq_ignore_ascii_case("sway") {
                return Compositor::Sway;
            }
            if entry.eq_ignore_ascii_case("hyprland") {
                return Compositor::Hyprland;
            }
            if entry.eq_ignore_ascii_case("kde") || entry.eq_ignore_ascii_case("plasma") {
                return Compositor::Kde;
            }
            if entry.eq_ignore_ascii_case("gnome") || entry.eq_ignore_ascii_case("unity") {
                return Compositor::Gnome;
            }
        }
    }
    Compositor::Other
}

/// Read the real process environment into a [`SessionEnv`]-shaped owned buffer.
fn env_snapshot() -> [Option<String>; 7] {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    [
        get("XDG_SESSION_TYPE"),
        get("WAYLAND_DISPLAY"),
        get("DISPLAY"),
        get("XDG_CURRENT_DESKTOP"),
        get("SWAYSOCK"),
        get("HYPRLAND_INSTANCE_SIGNATURE"),
        get("KDE_FULL_SESSION"),
    ]
}

/// The session this process is attached to, detected once.
///
/// Cached because it cannot change while the daemon lives: a user switching
/// from X11 to Wayland logs out, which takes the daemon's session with it.
#[must_use]
pub fn session() -> LinuxSession {
    static CACHE: OnceLock<LinuxSession> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let vars = env_snapshot();
        classify(&SessionEnv {
            xdg_session_type: vars[0].as_deref(),
            wayland_display: vars[1].as_deref(),
            display: vars[2].as_deref(),
            xdg_current_desktop: vars[3].as_deref(),
            swaysock: vars[4].as_deref(),
            hyprland_signature: vars[5].as_deref(),
            kde_full_session: vars[6].as_deref(),
        })
    })
}

// ── Helper-binary probing ────────────────────────────────────────────────────

/// Every external binary any Linux limb may shell out to.
///
/// Kept as one list so the `PATH` walk happens once instead of once per query,
/// and so "what does Aleph need installed on Linux" has a single answer.
pub const KNOWN_TOOLS: &[&str] = &[
    // input
    "xdotool",
    "ydotool",
    // window management
    "wmctrl",
    "swaymsg",
    "hyprctl",
    "kdotool",
    // clipboard
    "wl-copy",
    "wl-paste",
    "xclip",
    "xsel",
    // misc desktop integration
    "notify-send",
    "gtk-launch",
    "gio",
    "xdg-open",
    "xprintidle",
    "gdbus",
    "tesseract",
    "ffmpeg",
    "pactl",
    // Screen recording on wlroots compositors (sway / Hyprland): x11grab can
    // only see an XWayland root, so a Wayland session needs its own recorder.
    "wf-recorder",
];

/// Which of [`KNOWN_TOOLS`] exist on `PATH`.
#[derive(Debug, Clone, Default)]
pub struct ToolBox {
    present: BTreeSet<&'static str>,
}

impl ToolBox {
    /// Probe `PATH` for every known tool.
    #[must_use]
    pub fn probe() -> Self {
        let Some(path) = std::env::var_os("PATH") else {
            return Self::default();
        };
        let dirs: Vec<_> = std::env::split_paths(&path).collect();
        let present = KNOWN_TOOLS
            .iter()
            .copied()
            .filter(|tool| dirs.iter().any(|dir| is_executable(&dir.join(tool))))
            .collect();
        Self { present }
    }

    /// Build a fixed toolbox — for tests, which must not depend on what the
    /// host happens to have installed.
    #[must_use]
    pub fn from_names(names: &[&'static str]) -> Self {
        Self {
            present: names.iter().copied().collect(),
        }
    }

    /// Is `tool` available?
    #[must_use]
    pub fn has(&self, tool: &str) -> bool {
        self.present.contains(tool)
    }

    /// The first available tool from `candidates`, in caller-given preference
    /// order.
    #[must_use]
    pub fn first_of(&self, candidates: &[&'static str]) -> Option<&'static str> {
        candidates.iter().copied().find(|t| self.has(t))
    }
}

/// Resolve an executable on `PATH`, applying the same "is it actually
/// runnable" test [`ToolBox::probe`] uses.
///
/// Shared rather than re-implemented per caller: a lookup that checks only
/// `is_file` happily "finds" a data file or a directory entry with no execute
/// bit, and then the spawn fails for a reason the caller reports as something
/// else entirely.
///
/// A name containing a separator is a path, not a `PATH` lookup, and returns
/// `None` so callers cannot accidentally turn `./foo` into a search.
#[must_use]
pub fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// `true` if `path` is a file the current user may execute.
fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// The tools available in this process, probed once.
///
/// Cached deliberately: `PATH` is inherited at exec and Aleph never mutates it
/// mid-run, so re-walking it per action would be pure overhead. A user who
/// installs `xdotool` while the daemon runs restarts the daemon — the same
/// contract every other `PATH`-probing daemon has.
#[must_use]
pub fn tools() -> &'static ToolBox {
    static CACHE: OnceLock<ToolBox> = OnceLock::new();
    CACHE.get_or_init(ToolBox::probe)
}

/// Build the error for "none of the tools that could do this are installed".
///
/// Every Linux limb needs to say this, and saying it badly is the difference
/// between an actionable message and a bare `No such file or directory` that
/// tells the model nothing. Naming the candidates is the whole point.
#[must_use]
pub fn missing_tool_error(action: &str, candidates: &[&str]) -> DesktopError {
    DesktopError::NotAvailable(format!(
        "{action} needs one of: {}. None is installed — install one (e.g. `sudo apt install {}`) and retry.",
        candidates.join(", "),
        candidates.first().copied().unwrap_or("the tool")
    ))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>() -> SessionEnv<'a> {
        SessionEnv::default()
    }

    #[test]
    fn xdg_session_type_is_authoritative() {
        let mut e = env();
        e.xdg_session_type = Some("wayland");
        // DISPLAY is exported by XWayland; it must not override the declaration.
        e.display = Some(":0");
        assert_eq!(classify_kind(&e), SessionKind::Wayland);

        let mut e = env();
        e.xdg_session_type = Some("X11"); // case-insensitive
        e.wayland_display = Some("wayland-0");
        assert_eq!(classify_kind(&e), SessionKind::X11);
    }

    #[test]
    fn wayland_display_beats_display_when_type_is_unset() {
        let mut e = env();
        e.wayland_display = Some("wayland-0");
        e.display = Some(":0");
        assert_eq!(classify_kind(&e), SessionKind::Wayland);
    }

    #[test]
    fn display_alone_is_x11() {
        let mut e = env();
        e.display = Some(":10.0");
        assert_eq!(classify_kind(&e), SessionKind::X11);
    }

    #[test]
    fn nothing_set_is_unknown() {
        assert_eq!(classify_kind(&env()), SessionKind::Unknown);
    }

    #[test]
    fn empty_values_do_not_count_as_present() {
        let mut e = env();
        e.wayland_display = Some("");
        e.display = Some("");
        assert_eq!(classify_kind(&e), SessionKind::Unknown);
    }

    #[test]
    fn unrecognised_session_type_falls_through_to_the_display_vars() {
        let mut e = env();
        e.xdg_session_type = Some("tty");
        e.display = Some(":0");
        assert_eq!(classify_kind(&e), SessionKind::X11);
    }

    #[test]
    fn compositor_ipc_handles_win_over_desktop_name() {
        let mut e = env();
        e.hyprland_signature = Some("abc123");
        e.xdg_current_desktop = Some("GNOME");
        assert_eq!(classify_compositor(&e), Compositor::Hyprland);

        let mut e = env();
        e.swaysock = Some("/run/user/1000/sway-ipc.sock");
        e.xdg_current_desktop = Some("KDE");
        assert_eq!(classify_compositor(&e), Compositor::Sway);
    }

    #[test]
    fn compositor_from_colon_separated_desktop_list() {
        let mut e = env();
        e.xdg_current_desktop = Some("ubuntu:GNOME");
        assert_eq!(classify_compositor(&e), Compositor::Gnome);

        let mut e = env();
        e.xdg_current_desktop = Some("KDE");
        assert_eq!(classify_compositor(&e), Compositor::Kde);

        let mut e = env();
        e.xdg_current_desktop = Some("sway");
        assert_eq!(classify_compositor(&e), Compositor::Sway);
    }

    #[test]
    fn xfce_and_friends_are_other() {
        let mut e = env();
        e.xdg_current_desktop = Some("XFCE");
        assert_eq!(classify_compositor(&e), Compositor::Other);
        assert_eq!(classify_compositor(&env()), Compositor::Other);
    }

    #[test]
    fn kde_full_session_marks_plasma() {
        let mut e = env();
        e.kde_full_session = Some("true");
        assert_eq!(classify_compositor(&e), Compositor::Kde);
    }

    #[test]
    fn is_wayland_only_for_wayland() {
        assert!(SessionKind::Wayland.is_wayland());
        assert!(!SessionKind::X11.is_wayland());
        assert!(!SessionKind::Unknown.is_wayland());
    }

    #[test]
    fn toolbox_first_of_respects_preference_order() {
        let tb = ToolBox::from_names(&["xsel", "xclip"]);
        assert_eq!(tb.first_of(&["xclip", "xsel"]), Some("xclip"));
        assert_eq!(tb.first_of(&["xsel", "xclip"]), Some("xsel"));
        assert_eq!(tb.first_of(&["wl-copy"]), None);
        assert!(tb.has("xclip"));
        assert!(!tb.has("wmctrl"));
    }

    #[test]
    fn empty_toolbox_finds_nothing() {
        let tb = ToolBox::from_names(&[]);
        assert_eq!(tb.first_of(&["xclip", "xsel"]), None);
    }

    #[test]
    fn probe_returns_only_known_tools() {
        // Whatever the host has, the result is always a subset of KNOWN_TOOLS —
        // the probe must never invent a name a caller cannot ask about.
        let tb = ToolBox::probe();
        for name in &tb.present {
            assert!(KNOWN_TOOLS.contains(name), "unexpected tool {name}");
        }
    }

    #[test]
    fn path_lookup_rejects_paths_and_empty_names() {
        assert_eq!(find_on_path(""), None);
        assert_eq!(find_on_path("./local"), None);
        assert_eq!(find_on_path("/usr/bin/env"), None, "a path is not a lookup");
    }

    #[test]
    #[cfg(unix)]
    fn path_lookup_finds_a_real_executable_and_not_a_plain_file() {
        // `sh` exists and is executable on every unix host.
        assert!(find_on_path("sh").is_some());
        assert_eq!(find_on_path("definitely-not-a-binary-xyz"), None);
    }

    #[test]
    fn missing_tool_error_names_every_candidate() {
        let err = missing_tool_error("Window listing", &["xdotool", "wmctrl"]);
        let msg = err.to_string();
        assert!(msg.contains("xdotool"), "{msg}");
        assert!(msg.contains("wmctrl"), "{msg}");
        assert!(msg.contains("Window listing"), "{msg}");
    }
}
