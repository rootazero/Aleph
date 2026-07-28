//! One answer to "what windows and apps are on this Windows desktop".
//!
//! Three modules used to run their own `EnumWindows` with three different ideas
//! of what counts — [`crate::action::window`] (the window list), the Windows
//! crate's running-app list, and its UI Automation root resolver — and they
//! disagreed in ways that surfaced as bugs rather than as differences:
//!
//! * `IsWindowVisible` alone is not "on screen". Windows keeps `WS_VISIBLE` set
//!   on **cloaked** windows: UWP apps suspended in the background, and every
//!   window parked on another virtual desktop. Enumerating them fills the list
//!   with handles that capture as blank frames and cannot be clicked.
//! * `GetWindowRect` is not the window a person sees. On a DWM-composited
//!   desktop it includes the invisible resize border — roughly 7 px per side —
//!   so a capture cropped to it carries a strip of whatever is behind, and a
//!   click mapped through it is offset.
//! * A window title is not an app name. The running-app list reported titles as
//!   app names, which made `app: "explorer"` miss (File Explorer titles itself
//!   after the folder) and turned two windows of one app into "matches 2
//!   running apps".
//!
//! This module is the single source for all three, so the next consumer inherits
//! the answers instead of re-deriving them.

#![cfg(windows)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::BoundingBox;

/// A visible, user-facing top-level window.
#[derive(Debug, Clone)]
pub struct TopLevelWindow {
    /// The `HWND` as an integer, which is what [`crate::WindowInfo::id`] carries.
    pub id: u64,
    /// Window title. **May be empty**: an untitled top-level window is still a
    /// real window to root an accessibility query at, even though it is
    /// unidentifiable in a list shown to the model. The list consumers filter it
    /// out; [`top_window_for_pid`] does not.
    pub title: String,
    /// Owning process id.
    pub pid: u32,
    /// The frame a person sees, in global physical pixels — DWM's extended
    /// frame bounds when it answers, `GetWindowRect` otherwise. `None` for a
    /// minimized window, whose rect Windows parks at the (-32000, -32000)
    /// sentinel: a position nothing can be clicked at is not a position.
    pub bounds: Option<BoundingBox>,
    /// Whether the window is minimized.
    pub minimized: bool,
}

/// Enumerate the visible top-level windows on the current desktop.
///
/// Excludes invisible, cloaked and tool windows — the set an Alt-Tab switcher
/// shows. It deliberately keeps **untitled** windows: dropping them is right for
/// a list handed to the model (a window with no title cannot be named or chosen)
/// but wrong for resolving a pid to a window to query, which is why that filter
/// lives in the list consumers rather than here.
#[must_use]
pub fn enumerate_top_level() -> Vec<TopLevelWindow> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible,
    };

    extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `EnumWindows` guarantees `hwnd` is valid for this callback and
        // `lparam` carries the `&mut Vec` passed below, which outlives the
        // synchronous enumeration.
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || is_cloaked(hwnd) || is_tool_window(hwnd) {
                return BOOL(1);
            }
            let len = GetWindowTextLengthW(hwnd);
            let title = if len > 0 {
                // +1 for the terminating NUL `GetWindowTextW` always writes.
                let mut buf = vec![0u16; len as usize + 1];
                let written = GetWindowTextW(hwnd, &mut buf);
                String::from_utf16_lossy(&buf[..written.max(0) as usize])
            } else {
                String::new()
            };

            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(std::ptr::addr_of_mut!(pid)));

            let minimized = IsIconic(hwnd).as_bool();
            let bounds = if minimized { None } else { visible_frame(hwnd) };

            let out = &mut *(lparam.0 as *mut Vec<TopLevelWindow>);
            out.push(TopLevelWindow {
                id: hwnd.0 as usize as u64,
                title,
                pid,
                bounds,
                minimized,
            });
        }
        BOOL(1)
    }

    let mut out: Vec<TopLevelWindow> = Vec::new();
    // SAFETY: `enum_proc` matches `WNDENUMPROC`; `out` lives until `EnumWindows`
    // returns.
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(std::ptr::addr_of_mut!(out) as isize),
        );
    }
    out
}

/// Whether DWM is hiding this window: a suspended UWP app (`DWM_CLOAKED_APP`),
/// or a window parked on another virtual desktop (`DWM_CLOAKED_SHELL`).
///
/// `IsWindowVisible` returns `true` for both, so enumerating on it alone fills
/// the list with handles that are not on any screen the user is looking at.
///
/// Excluding both is the same call the macOS arm already made, for the same
/// stated reason: `macos_window_list` passes `kCGWindowListOptionOnScreenOnly`
/// and its comment spells out that listing other-Space windows "would hand the
/// model window ids that silently do not work, which is worse than not offering
/// them". A shell-cloaked window is precisely the Windows spelling of an
/// other-Space window — its pixels are not where the coordinates say they are,
/// and reaching it means yanking the user to a different desktop. Listing them
/// is a separate feature (it needs a switch-then-act step), not a filter change.
#[must_use]
pub fn is_cloaked(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};

    let mut cloaked: u32 = 0;
    // SAFETY: writes a `u32` into `cloaked`; the size argument matches. A
    // pre-composition or unsupported attribute surfaces as `Err`, in which case
    // "not cloaked" is the answer that keeps the window listed.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            std::ptr::addr_of_mut!(cloaked).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    }
    .is_ok();
    ok && cloaked != 0
}

/// Whether this is a floating palette / tool window — present on screen but not
/// something a person switches to, and not a capture target.
fn is_tool_window(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
    };

    // SAFETY: documented read of a window's extended style bits.
    let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    ex_style & WS_EX_TOOLWINDOW.0 != 0
}

/// The window frame a person sees, in global physical pixels.
///
/// Prefers DWM's extended frame bounds: `GetWindowRect` on a composited desktop
/// includes the invisible resize border, so a crop taken from it leaks a strip
/// of the desktop behind and a click mapped through it is offset by the border
/// width.
fn visible_frame(hwnd: windows::Win32::Foundation::HWND) -> Option<BoundingBox> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let mut rect = RECT::default();
    // SAFETY: writes a `RECT` into `rect`; the size argument matches.
    let dwm_ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            std::ptr::addr_of_mut!(rect).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .is_ok();

    if !dwm_ok {
        // SAFETY: documented Win32 geometry read.
        unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    }

    // A zero-area rectangle is not a frame; reporting it would hand a consumer
    // a crop it cannot take.
    let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
    (w > 0 && h > 0).then(|| BoundingBox {
        x: f64::from(rect.left),
        y: f64::from(rect.top),
        w: f64::from(w),
        h: f64::from(h),
    })
}

/// The process id owning the foreground window, if there is one.
#[must_use]
pub fn foreground_pid() -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    // SAFETY: documented Win32 calls; a desktop with nothing focused returns a
    // null handle, which is filtered before the pid read.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(std::ptr::addr_of_mut!(pid)));
        (pid != 0).then_some(pid)
    }
}

/// The handle of a visible top-level window owned by `pid`, for rooting an
/// accessibility query.
///
/// A titled window wins — it is the one a person would call "the app's window" —
/// but an untitled one is accepted rather than refusing the query: some apps
/// (Electron mid-load, borderless players) have exactly one top-level window and
/// no title, and their AX tree is perfectly readable.
#[must_use]
pub fn top_window_for_pid(pid: u32) -> Option<u64> {
    let windows = enumerate_top_level();
    windows
        .iter()
        .find(|w| w.pid == pid && !w.title.is_empty())
        .or_else(|| windows.iter().find(|w| w.pid == pid))
        .map(|w| w.id)
}

/// Full path of a process's executable.
///
/// `PROCESS_QUERY_LIMITED_INFORMATION` is the minimum right that answers this
/// and, unlike `PROCESS_QUERY_INFORMATION`, it is granted for processes running
/// at a higher integrity level — so an elevated editor still gets a name instead
/// of silently dropping out of the app list.
#[must_use]
pub fn process_image_path(pid: u32) -> Option<PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: the handle is closed on every path below; `QueryFullProcessImageNameW`
    // writes at most `size` UTF-16 units into `buf` and updates `size`.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            std::ptr::addr_of_mut!(size),
        )
        .is_ok();
        let _ = CloseHandle(handle);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..size as usize])))
    }
}

/// The name a person calls this app.
///
/// Reads the executable's `FileDescription` version-resource string — the value
/// Task Manager shows and the one a model is most likely to be asked for
/// ("Google Chrome", "Visual Studio Code"). Falls back to the file stem, because
/// "chrome" is still a better handle than a window title that changes with the
/// open tab.
#[must_use]
pub fn friendly_app_name(exe: &Path) -> String {
    file_description(exe).unwrap_or_else(|| {
        exe.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

/// Read `StringFileInfo\<lang><codepage>\FileDescription` from an executable's
/// version resource. `None` whenever the resource is absent or malformed — many
/// perfectly good executables ship without one.
fn file_description(exe: &Path) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    /// One entry of the version resource's translation table.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LangCodepage {
        language: u16,
        codepage: u16,
    }

    let path = HSTRING::from(exe);

    // SAFETY: every pointer handed to the version APIs points into `block`,
    // which outlives the reads; `len` is what the API itself reported. The
    // pointers returned by `VerQueryValueW` borrow from `block` and are only
    // read while it is alive.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(path.as_ptr()), 0, size, block.as_mut_ptr().cast()).ok()?;

        // The description is filed under the executable's own language and code
        // page, which the translation table names. Guessing a fixed one (the
        // usual 040904b0) misses every non-English build.
        let mut translations: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut translations_len: u32 = 0;
        let query = HSTRING::from("\\VarFileInfo\\Translation");
        if !VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(query.as_ptr()),
            std::ptr::addr_of_mut!(translations),
            std::ptr::addr_of_mut!(translations_len),
        )
        .as_bool()
            || translations.is_null()
            || (translations_len as usize) < std::mem::size_of::<LangCodepage>()
        {
            return None;
        }

        let count = translations_len as usize / std::mem::size_of::<LangCodepage>();
        let entries = std::slice::from_raw_parts(translations.cast::<LangCodepage>(), count);

        for entry in entries {
            let sub_block = HSTRING::from(format!(
                "\\StringFileInfo\\{:04x}{:04x}\\FileDescription",
                entry.language, entry.codepage
            ));
            let mut value: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut value_len: u32 = 0;
            if VerQueryValueW(
                block.as_ptr().cast(),
                PCWSTR(sub_block.as_ptr()),
                std::ptr::addr_of_mut!(value),
                std::ptr::addr_of_mut!(value_len),
            )
            .as_bool()
                && !value.is_null()
                && value_len > 0
            {
                let wide = std::slice::from_raw_parts(value.cast::<u16>(), value_len as usize);
                // The reported length includes the terminating NUL.
                let text = String::from_utf16_lossy(wide)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
}

/// One record per process that owns at least one visible top-level window.
#[derive(Debug, Clone)]
pub struct RunningApp {
    /// Friendly name (`FileDescription`, else the executable stem).
    pub name: String,
    /// Full executable path — Windows' closest analogue to a bundle id, and
    /// what makes `app: "chrome.exe"` resolvable.
    pub executable: String,
    pub pid: u32,
    /// Whether this process owns the foreground window.
    pub is_active: bool,
}

/// Group the visible top-level windows into one entry per owning process.
///
/// Deduplication is the point: enumerating windows produced one "app" per
/// window, so two open browser windows made `app: "chrome"` ambiguous and the
/// tool refused rather than targeting the single process behind both.
#[must_use]
pub fn running_apps() -> Vec<RunningApp> {
    let foreground = foreground_pid();
    let mut by_pid: HashMap<u32, RunningApp> = HashMap::new();

    for window in enumerate_top_level() {
        // A process whose only window has no title is not something a person
        // would name; it is a helper surface, not an app in the list.
        if window.title.is_empty() || by_pid.contains_key(&window.pid) {
            continue;
        }
        let (name, executable) = match process_image_path(window.pid) {
            Some(exe) => (friendly_app_name(&exe), exe.to_string_lossy().into_owned()),
            // No image path (a protected process, or one that exited between the
            // enumeration and the query): the title is the only handle left, and
            // a worse one — but dropping the app entirely is worse still.
            None => (window.title.clone(), String::new()),
        };
        by_pid.insert(
            window.pid,
            RunningApp {
                name,
                executable,
                pid: window.pid,
                is_active: foreground == Some(window.pid),
            },
        );
    }

    by_pid.into_values().collect()
}
