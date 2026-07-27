//! Launching and quitting applications on Linux — one implementation.
//!
//! There were two of each, and all four were weak:
//!
//! | | old `ScreenCapability` path | old `SystemCapability` path |
//! |---|---|---|
//! | launch | `xdg-open <app_name>` | `gtk-launch <app_name>` → `xdg-open` |
//! | quit | `pkill -x -- <name>` | `killall <name>` → **`pkill -f <name>`** |
//!
//! `xdg-open` opens *files and URLs*; handing it a bare application name is a
//! category error, so `launch_app("firefox")` could only ever work by accident.
//! `gtk-launch` needs a desktop-file **id**, not a human name, so it missed
//! anything whose `.desktop` file is not literally `<name>.desktop`. And
//! `pkill -f` matches the whole command line, so quitting one app could take
//! unrelated processes — including the agent — with it.
//!
//! # Launch, in order
//!
//! 1. **A URL or an existing path** → `xdg-open`. That is what it is for.
//! 2. **A desktop entry** resolved by id, `Name=`, or executable → `gtk-launch`
//!    (or `gio launch`). This is the path that makes human names work, and it
//!    is what a desktop's own launcher does.
//! 3. **A binary on `PATH`** → spawn it directly, detached.
//! 4. Otherwise an error that says all three were tried.
//!
//! # Quit, in order
//!
//! 1. **Ask the windows to close** through the window manager, exactly as
//!    clicking the close button would — the application runs its own shutdown
//!    path and can still prompt to save.
//! 2. **`SIGTERM` to the matching pids**, matched by *executable name*, never by
//!    command line (see [`super::proc::matches_name`]).
//!
//! Both steps address processes the same way, so "quit chrome" can never reach
//! a process that merely mentions chrome.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tracing::info;

use super::proc;
use super::session::{missing_tool_error, tools, ToolBox};
use crate::error::{DesktopError, Result};

// ── Desktop-entry resolution (pure) ──────────────────────────────────────────

/// The fields of a `.desktop` file this module cares about.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// `Name=` — the human-facing application name.
    pub name: String,
    /// The executable from `Exec=`, with its field codes (`%U`, `%f`, …) and
    /// arguments stripped.
    pub exec: String,
    /// `NoDisplay=true` marks an entry the desktop hides from its menus (MIME
    /// handlers, settings panels). Not something a user means by an app name.
    pub no_display: bool,
}

/// Parse the `[Desktop Entry]` group of a `.desktop` file.
///
/// Only the first group is read: later groups are `Desktop Action` blocks with
/// their own `Name=`/`Exec=`, and taking those would resolve "Firefox" to
/// "Open a New Private Window".
#[must_use]
pub fn parse_desktop_entry(contents: &str) -> DesktopEntry {
    let mut entry = DesktopEntry::default();
    let mut in_main_group = false;

    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // Entering any group other than the first ends the parse.
            if in_main_group {
                break;
            }
            in_main_group = line.eq_ignore_ascii_case("[Desktop Entry]");
            continue;
        }
        if !in_main_group || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            // Bare `Name`, not `Name[zh_CN]`: the localized variants would
            // otherwise overwrite the canonical one depending on file order.
            "Name" if entry.name.is_empty() => entry.name = value.trim().to_string(),
            "Exec" if entry.exec.is_empty() => entry.exec = exec_binary(value.trim()),
            "NoDisplay" => entry.no_display = value.trim().eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    entry
}

/// Reduce an `Exec=` line to the executable's file name.
///
/// `Exec=/usr/lib/firefox/firefox %u` → `firefox`;
/// `Exec=env FOO=1 /usr/bin/code --unity-launch %F` → `env` is wrong, so the
/// first token that is not an environment assignment wins.
#[must_use]
pub fn exec_binary(exec: &str) -> String {
    for token in exec.split_whitespace() {
        // Skip a leading `env` wrapper and any VAR=value assignments.
        if token == "env" || (token.contains('=') && !token.starts_with('/')) {
            continue;
        }
        if token.starts_with('%') {
            break;
        }
        return Path::new(token)
            .file_name()
            .map_or_else(|| token.to_string(), |f| f.to_string_lossy().into_owned());
    }
    String::new()
}

/// How well a desktop entry answers to a requested application name.
///
/// Ordered worst-to-best so `max_by_key` picks the strongest match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchRank {
    /// The `Name=` merely starts with the query — "Firefox Web Browser" for
    /// "firefox". Real, but the weakest signal.
    NamePrefix,
    /// The entry's executable is exactly the query.
    Exec,
    /// The `Name=` is exactly the query, ignoring case.
    Name,
    /// The desktop-file id is exactly the query — `firefox` ↔ `firefox.desktop`.
    Id,
}

/// Rank one entry against a query, or `None` if it does not answer to it.
///
/// Hidden (`NoDisplay=true`) entries never match: they are MIME handlers and
/// settings panels, not things a user asks to launch by name.
#[must_use]
pub fn rank_entry(id: &str, entry: &DesktopEntry, query: &str) -> Option<MatchRank> {
    if entry.no_display || query.is_empty() {
        return None;
    }
    if id.eq_ignore_ascii_case(query) {
        return Some(MatchRank::Id);
    }
    if entry.name.eq_ignore_ascii_case(query) {
        return Some(MatchRank::Name);
    }
    if !entry.exec.is_empty() && entry.exec.eq_ignore_ascii_case(query) {
        return Some(MatchRank::Exec);
    }
    // Prefix match on a word boundary only: "Files" must not match "File
    // Roller"'s neighbours by accident, but "firefox" should find "Firefox Web
    // Browser".
    let name = entry.name.to_lowercase();
    let q = query.to_lowercase();
    if name.starts_with(&q) && name.as_bytes().get(q.len()).is_none_or(|b| *b == b' ') {
        return Some(MatchRank::NamePrefix);
    }
    None
}

/// `true` when the string is something `xdg-open` should handle: a URL.
#[must_use]
pub fn looks_like_url(s: &str) -> bool {
    if let Some((scheme, rest)) = s.split_once(':') {
        if scheme.is_empty() || rest.is_empty() {
            return false;
        }
        // RFC 3986 scheme: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
        return scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    }
    false
}

/// The directories `.desktop` files live in, per the XDG base-directory spec.
#[must_use]
pub fn desktop_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':').filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(dir).join("applications"));
    }
    dirs
}

/// Every desktop entry on the system, keyed by desktop-file id.
///
/// Earlier directories win, which is the XDG precedence rule: a user's own
/// `~/.local/share/applications/firefox.desktop` overrides the system one.
#[must_use]
fn all_entries() -> BTreeMap<String, DesktopEntry> {
    let mut map: BTreeMap<String, DesktopEntry> = BTreeMap::new();
    for dir in desktop_dirs() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in read.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if map.contains_key(id) {
                continue; // earlier directory already claimed this id
            }
            if let Ok(contents) = std::fs::read_to_string(&path) {
                map.insert(id.to_string(), parse_desktop_entry(&contents));
            }
        }
    }
    map
}

/// Resolve an application name to a desktop-file id.
#[must_use]
pub fn find_desktop_id(query: &str) -> Option<String> {
    best_match(&all_entries(), query)
}

/// Pure half of [`find_desktop_id`]: pick the best-ranked id from a map.
#[must_use]
pub fn best_match(entries: &BTreeMap<String, DesktopEntry>, query: &str) -> Option<String> {
    entries
        .iter()
        .filter_map(|(id, entry)| rank_entry(id, entry, query).map(|rank| (rank, id)))
        // Ties break on the id, so the choice is deterministic run to run
        // rather than dependent on directory-read order.
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(a.1)))
        .map(|(_, id)| id.clone())
}

// ── Launch ───────────────────────────────────────────────────────────────────

/// Launch an application, a file, or a URL.
///
/// # Errors
///
/// [`DesktopError::InputFailed`] when nothing could be launched, naming every
/// strategy that was tried.
pub fn launch(app_name: &str) -> Result<()> {
    let app_name = app_name.trim();
    if app_name.is_empty() {
        return Err(DesktopError::InputFailed(
            "launch_app needs an application name, a file path, or a URL".into(),
        ));
    }
    let tb = tools();

    // 1. URLs and real paths are xdg-open's actual job.
    if looks_like_url(app_name) || Path::new(app_name).exists() {
        return open_with(tb, app_name);
    }

    // 2. A desktop entry — the path that makes human-facing names work.
    if let Some(id) = find_desktop_id(app_name) {
        if let Some(launcher) = tb.first_of(&["gtk-launch", "gio"]) {
            let args = launch_entry_args(launcher, &id);
            if spawn_detached(launcher, &args).is_ok() {
                info!(app_name, id, launcher, "App launched via desktop entry");
                return Ok(());
            }
        }
    }

    // 3. A plain executable on PATH.
    if which(app_name).is_some() {
        spawn_detached(app_name, &[]).map_err(|e| {
            DesktopError::InputFailed(format!("Failed to start '{app_name}': {e}"))
        })?;
        info!(app_name, "App launched from PATH");
        return Ok(());
    }

    Err(DesktopError::InputFailed(format!(
        "Could not launch '{app_name}': it is not a URL or an existing path, no .desktop entry \
         matches it (by id, Name, or Exec), and it is not an executable on PATH. Check the name \
         with the system tool's app list, or pass a full path."
    )))
}

/// Argv for launching a resolved desktop entry with `launcher`.
#[must_use]
pub fn launch_entry_args(launcher: &str, desktop_id: &str) -> Vec<String> {
    match launcher {
        // `gio launch` wants the file, `gtk-launch` wants the id.
        "gio" => vec!["launch".into(), format!("{desktop_id}.desktop")],
        _ => vec![desktop_id.to_string()],
    }
}

fn open_with(tb: &ToolBox, target: &str) -> Result<()> {
    let Some(opener) = tb.first_of(&["xdg-open", "gio"]) else {
        return Err(missing_tool_error("Opening a file or URL", &["xdg-open"]));
    };
    let args: Vec<String> = if opener == "gio" {
        vec!["open".into(), target.to_string()]
    } else {
        vec![target.to_string()]
    };
    spawn_detached(opener, &args)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to open '{target}': {e}")))?;
    info!(target, opener, "Opened via the desktop opener");
    Ok(())
}

/// Start a process without waiting for it and without inheriting our stdio.
///
/// A launched application outlives the tool call by definition, so waiting on
/// it would hang the turn; and letting it inherit the daemon's stdout would
/// route its chatter into Aleph's log.
fn spawn_detached(program: &str, args: &[String]) -> std::io::Result<()> {
    use std::process::Stdio;
    std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_child| ())
}

/// Resolve an executable on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    // A name with a separator is a path, not a PATH lookup.
    if name.contains('/') {
        return None;
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

// ── Quit ─────────────────────────────────────────────────────────────────────

/// Quit an application: ask its windows to close, then fall back to `SIGTERM`.
///
/// # Errors
///
/// [`DesktopError::InputFailed`] when no process answers to the name.
pub fn quit(app_name: &str) -> Result<()> {
    let app_name = app_name.trim();
    if app_name.is_empty() {
        return Err(DesktopError::InputFailed(
            "quit_app needs an application name".into(),
        ));
    }

    let pids = proc::pids_named(app_name);
    if pids.is_empty() {
        return Err(DesktopError::InputFailed(format!(
            "No running process is named '{app_name}'. The name must match the executable \
             exactly (Aleph never matches on a command line, so an approximate name finds \
             nothing rather than killing the wrong process)."
        )));
    }

    // 1. The polite path: close the windows, let the app shut itself down.
    let closed = close_windows_of(&pids);
    if closed > 0 {
        info!(app_name, closed, "App asked to close via its windows");
        return Ok(());
    }

    // 2. No windows (or no window backend) — signal the processes themselves.
    let signalled = terminate(&pids);
    if signalled > 0 {
        info!(app_name, signalled, "App terminated via SIGTERM");
        return Ok(());
    }

    Err(DesktopError::InputFailed(format!(
        "Found {} process(es) named '{app_name}' but could not close or signal any of them \
         (they may belong to another user).",
        pids.len()
    )))
}

/// Ask the window manager to close every window owned by one of `pids`.
/// Returns how many close requests were sent.
fn close_windows_of(pids: &[u32]) -> usize {
    let Ok(windows) = crate::action::window_linux::window_list() else {
        // No window backend in this session (headless, GNOME Wayland). Not an
        // error — the signal path below is the answer there.
        return 0;
    };
    windows
        .iter()
        .filter(|w| u32::try_from(w.pid).is_ok_and(|p| pids.contains(&p)))
        .filter(|w| crate::action::window_linux::close_window(w.id).is_ok())
        .count()
}

/// Send `SIGTERM` to each pid. Returns how many were signalled.
#[cfg(unix)]
fn terminate(pids: &[u32]) -> usize {
    pids.iter()
        .filter(|pid| {
            let Ok(raw) = i32::try_from(**pid) else {
                return false;
            };
            // SAFETY: `kill` with a positive pid and SIGTERM has no memory
            // effects; a failure (ESRCH/EPERM) is reported by the return value.
            unsafe { libc::kill(raw, libc::SIGTERM) == 0 }
        })
        .count()
}

#[cfg(not(unix))]
fn terminate(_pids: &[u32]) -> usize {
    0
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, exec: &str) -> DesktopEntry {
        DesktopEntry {
            name: name.to_string(),
            exec: exec.to_string(),
            no_display: false,
        }
    }

    #[test]
    fn parses_the_main_group_only() {
        let file = "\
[Desktop Entry]
Type=Application
Name=Firefox Web Browser
Name[de]=Firefox Webbrowser
Exec=/usr/lib/firefox/firefox %u
Icon=firefox

[Desktop Action new-private-window]
Name=Open a New Private Window
Exec=/usr/lib/firefox/firefox --private-window %u
";
        let e = parse_desktop_entry(file);
        assert_eq!(e.name, "Firefox Web Browser", "action group must not win");
        assert_eq!(e.exec, "firefox");
        assert!(!e.no_display);
    }

    #[test]
    fn localized_names_do_not_overwrite_the_canonical_one() {
        let file = "[Desktop Entry]\nName[zh_CN]=文件\nName=Files\nExec=nautilus\n";
        assert_eq!(parse_desktop_entry(file).name, "Files");
    }

    #[test]
    fn no_display_is_recognised() {
        let file = "[Desktop Entry]\nName=MIME Handler\nExec=handler\nNoDisplay=true\n";
        assert!(parse_desktop_entry(file).no_display);
    }

    #[test]
    fn exec_binary_strips_paths_field_codes_and_env_wrappers() {
        assert_eq!(exec_binary("/usr/lib/firefox/firefox %u"), "firefox");
        assert_eq!(exec_binary("code --unity-launch %F"), "code");
        assert_eq!(exec_binary("env GDK_BACKEND=x11 /usr/bin/code %F"), "code");
        assert_eq!(exec_binary("%f"), "");
        assert_eq!(exec_binary(""), "");
    }

    #[test]
    fn rank_prefers_id_then_name_then_exec_then_prefix() {
        let e = entry("Firefox Web Browser", "firefox");
        assert_eq!(rank_entry("firefox", &e, "firefox"), Some(MatchRank::Id));
        assert_eq!(
            rank_entry("org.mozilla.firefox", &e, "Firefox Web Browser"),
            Some(MatchRank::Name)
        );
        assert_eq!(
            rank_entry("org.mozilla.firefox", &e, "firefox"),
            Some(MatchRank::Exec)
        );
        let no_exec = entry("Firefox Web Browser", "");
        assert_eq!(
            rank_entry("org.mozilla.ff", &no_exec, "Firefox"),
            Some(MatchRank::NamePrefix)
        );
        assert!(MatchRank::Id > MatchRank::Name);
        assert!(MatchRank::Name > MatchRank::Exec);
        assert!(MatchRank::Exec > MatchRank::NamePrefix);
    }

    #[test]
    fn hidden_entries_never_match() {
        let mut e = entry("Firefox", "firefox");
        e.no_display = true;
        assert_eq!(rank_entry("firefox", &e, "firefox"), None);
    }

    #[test]
    fn prefix_match_respects_word_boundaries() {
        let e = entry("File Roller", "file-roller");
        // "File" is a whole leading word — a real prefix match.
        assert_eq!(rank_entry("x", &e, "File"), Some(MatchRank::NamePrefix));
        // "Fil" is mid-word; matching it would make every query fuzzy.
        assert_eq!(rank_entry("x", &e, "Fil"), None);
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert_eq!(rank_entry("firefox", &entry("Firefox", "firefox"), ""), None);
    }

    #[test]
    fn best_match_picks_the_strongest_rank_deterministically() {
        let mut entries = BTreeMap::new();
        entries.insert("zzz-alias".to_string(), entry("Firefox", "firefox"));
        entries.insert("firefox".to_string(), entry("Firefox Web Browser", "firefox"));
        // The exact-id entry must win over an exact-Name one.
        assert_eq!(best_match(&entries, "firefox").as_deref(), Some("firefox"));

        // Two equally ranked entries resolve the same way every run.
        let mut ties = BTreeMap::new();
        ties.insert("aaa".to_string(), entry("Editor", "ed"));
        ties.insert("bbb".to_string(), entry("Editor", "ed"));
        assert_eq!(best_match(&ties, "Editor"), best_match(&ties, "Editor"));
    }

    #[test]
    fn best_match_of_nothing_is_none() {
        assert_eq!(best_match(&BTreeMap::new(), "firefox"), None);
    }

    #[test]
    fn url_detection_accepts_schemes_and_rejects_names_and_paths() {
        assert!(looks_like_url("https://example.com"));
        assert!(looks_like_url("mailto:a@b.c"));
        assert!(looks_like_url("aleph://open"));
        assert!(!looks_like_url("firefox"));
        assert!(!looks_like_url("/usr/bin/firefox"));
        assert!(!looks_like_url("./relative"));
        assert!(!looks_like_url("C:"), "empty remainder is not a URL");
        assert!(!looks_like_url("9gag:x"), "scheme must start with a letter");
    }

    #[test]
    fn launcher_argv_matches_each_tools_contract() {
        assert_eq!(launch_entry_args("gtk-launch", "firefox"), vec!["firefox"]);
        assert_eq!(
            launch_entry_args("gio", "firefox"),
            vec!["launch", "firefox.desktop"]
        );
    }

    #[test]
    fn quitting_nothing_is_an_error_that_explains_the_matching_rule() {
        let err = quit("definitely-not-a-real-binary-name-xyz").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exactly"), "{msg}");
        assert!(!msg.contains("pkill"), "the old tool must not resurface");
    }

    #[test]
    fn empty_names_are_rejected_before_anything_runs() {
        assert!(quit("   ").is_err());
        assert!(launch("").is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn desktop_dirs_always_include_the_system_default() {
        let dirs = desktop_dirs();
        assert!(
            dirs.iter().any(|d| d.starts_with("/usr/share")
                || d.starts_with("/usr/local/share")
                || d.to_string_lossy().contains(".local/share")),
            "{dirs:?}"
        );
    }
}
