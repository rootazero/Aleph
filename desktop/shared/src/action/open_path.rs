//! Open a file or URL with the OS default handler.
//!
//! This is the universal "double-click it" primitive — `open` on macOS,
//! `xdg-open` on Linux, `start` on Windows. Unlike [`launch_app`](super::app_launch::launch_app),
//! which resolves an *application* by name/bundle-id, this opens a *document*
//! (or URL) with whatever application the OS has registered for it.
//!
//! Synchronous; call via `tokio::task::spawn_blocking` from async contexts.

use tracing::info;

use crate::error::{DesktopError, Result};

/// How long to watch `xdg-open` for an immediate failure before concluding a
/// handler took the target and letting it run unattended.
///
/// Long enough for the "nothing is registered for this type" exit (which is
/// immediate), short enough that it is not felt as latency.
#[cfg(target_os = "linux")]
const XDG_OPEN_SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

/// Open a filesystem path or URL with the system's default application.
///
/// `target` may be an absolute filesystem path (`/Users/me/report.html`) or a
/// URL (`https://…`, `file://…`, `mailto:…`). The OS picks the handler the same
/// way it would on a double-click — e.g. an `.html` file opens in the default
/// browser, a `.pdf` in the default PDF viewer.
///
/// - **macOS**: `/usr/bin/open <target>`
/// - **Linux**: `xdg-open <target>`
/// - **Windows**: `ShellExecuteW` with verb `"open"` (no cmd.exe)
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if `target` is empty or the handler fails.
/// - [`DesktopError::NotImplemented`] on unsupported platforms.
pub fn open(target: &str) -> Result<()> {
    if target.trim().is_empty() {
        return Err(DesktopError::InputFailed(
            "open: target path/URL is empty".into(),
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/bin/open")
            .arg(target)
            .status()
            .map_err(|e| {
                DesktopError::InputFailed(format!("open: failed to run /usr/bin/open: {e}"))
            })?;
        if !status.success() {
            return Err(DesktopError::InputFailed(format!(
                "open: '/usr/bin/open {target}' exited with {status}"
            )));
        }
        info!(target, "Opened with default handler (macOS)");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        use std::time::Instant;

        // `xdg-open` usually hands the target to a desktop-specific opener
        // (`exo-open`, `gio`, `kde-open`) and exits immediately. But its generic
        // fallback path runs the handler in the *foreground*, so on a desktop it
        // does not recognise it does not return until the user closes the opened
        // application. Waiting on that unbounded pins the `spawn_blocking`
        // thread — and the agent turn — for as long as the document stays open.
        //
        // So watch it only long enough to catch the failures that are reported
        // immediately (no handler registered is exit code 3, handler failed is
        // 4), and read "still running past the settle window" as success:
        // something evidently took the target.
        //
        // Deliberately **not** `output_capped_blocking`: that kills the child on
        // timeout, and in the foreground-exec case this pid *is* the application
        // the caller just asked to open.
        let mut child = std::process::Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|e| DesktopError::InputFailed(format!("open: failed to run xdg-open: {e}")))?;

        let deadline = Instant::now() + XDG_OPEN_SETTLE;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(status)) => {
                    return Err(DesktopError::InputFailed(format!(
                        "open: 'xdg-open {target}' exited with {status}"
                    )));
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(DesktopError::InputFailed(format!(
                        "open: failed to wait for xdg-open: {e}"
                    )));
                }
            }
            if Instant::now() >= deadline {
                // Left running on purpose. Reaped on a detached thread so a
                // long-lived daemon does not accumulate zombies for every
                // document it opens.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                info!(
                    target,
                    "xdg-open still running after the settle window; treating as launched (Linux)"
                );
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        info!(target, "Opened with default handler (Linux)");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        // NUL-terminated wide buffers for the Win32 W API.
        let verb: Vec<u16> = std::ffi::OsStr::new("open")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let file: Vec<u16> = std::ffi::OsStr::new(target)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // ShellExecuteW hands `target` straight to the shell's association
        // resolver — it never spawns cmd.exe, so shell metacharacters in
        // `target` cannot inject a command. Returns HINSTANCE > 32 on success.
        // SAFETY: `verb`/`file` are valid NUL-terminated wide buffers that
        // outlive the call; the remaining pointer args are null as documented.
        let hinst = unsafe {
            ShellExecuteW(
                HWND(std::ptr::null_mut()),
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if hinst.0 as isize <= 32 {
            return Err(DesktopError::InputFailed(format!(
                "open: ShellExecuteW failed for '{target}' (code {})",
                hinst.0 as isize
            )));
        }
        info!(target, "Opened with default handler (Windows)");
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(DesktopError::NotImplemented(
            "open not supported on this platform".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_target() {
        let err = open("   ").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
    }
}
