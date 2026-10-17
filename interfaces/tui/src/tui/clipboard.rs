//! Local OS clipboard transport; SSH/OSC52 forwarding is intentionally unsupported.

use std::io::{self, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

const MAX_BYTES: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(10);
type Helper = (&'static str, &'static [&'static str]);
#[cfg(any(windows, test))]
const POWERSHELL: Helper = (
    "powershell.exe",
    &[
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$ErrorActionPreference='Stop';[Console]::InputEncoding=[System.Text.UTF8Encoding]::new($false);Set-Clipboard -Value ([Console]::In.ReadToEnd())",
    ],
);

pub(super) async fn copy_text(text: String) -> Result<(), String> {
    check_size(text.len()).map_err(str::to_owned)?;
    tokio::task::spawn_blocking(move || copy_blocking(&text))
        .await
        .map_err(|_| "Clipboard worker failed".to_owned())?
}

fn check_size(bytes: usize) -> Result<(), &'static str> {
    if bytes > MAX_BYTES {
        Err("Clipboard text exceeds 1 MiB")
    } else {
        Ok(())
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_helpers(wayland: bool, display: bool) -> Vec<Helper> {
    let mut helpers = Vec::new();
    if wayland {
        helpers.push(("wl-copy", &[][..]));
    }
    if display {
        helpers.push(("xclip", &["-selection", "clipboard"][..]));
        helpers.push(("xsel", &["--clipboard", "--input"][..]));
    }
    helpers
}

fn copy_blocking(text: &str) -> Result<(), String> {
    #[cfg(windows)]
    let helpers = vec![POWERSHELL];
    #[cfg(target_os = "macos")]
    let helpers: Vec<Helper> = vec![("pbcopy", &[])];
    #[cfg(target_os = "linux")]
    let helpers = linux_helpers(
        std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
        std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty()),
    );
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let helpers: Vec<Helper> = Vec::new();

    // One shared budget also bounds retries across Linux helpers.
    let deadline = Instant::now() + TIMEOUT;
    let mut error = "No usable local clipboard helper or display";
    for helper in helpers {
        if Instant::now() >= deadline {
            return Err("Clipboard helper timed out".to_owned());
        }
        match run_helper(helper, text.as_bytes(), deadline) {
            Ok(()) => return Ok(()),
            Err(reason) => error = reason,
        }
    }
    Err(error.to_owned())
}

// Always reap the direct helper, including setup/write/timeout failure paths.
// There is no writer thread, so descendants holding stdin cannot strand one.
struct HelperChild(Child);
impl Drop for HelperChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_helper(helper: Helper, bytes: &[u8], deadline: Instant) -> Result<(), &'static str> {
    let mut command = Command::new(helper.0);
    command.args(helper.1).stdin(Stdio::piped());
    // Never collect or expose diagnostics: helpers may echo the payload.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(target_os = "macos")]
    command.env("LC_CTYPE", "en_US.UTF-8");
    let mut child = HelperChild(
        command
            .spawn()
            .map_err(|_| "Clipboard helper unavailable")?,
    );
    let mut input = child.0.stdin.take().ok_or("Clipboard stdin unavailable")?;
    nonblocking(&input).map_err(|_| "Clipboard nonblocking stdin unavailable")?;
    let mut offset = 0;
    while offset < bytes.len() {
        if Instant::now() >= deadline {
            return Err("Clipboard helper timed out");
        }
        match input.write(&bytes[offset..bytes.len().min(offset + 16 * 1024)]) {
            Ok(0) => std::thread::sleep(POLL),
            Ok(written) => offset += written,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(POLL);
            }
            Err(_) => return Err("Clipboard stdin write failed"),
        }
    }
    drop(input); // EOF, without adding a newline or transforming UTF-8.
    loop {
        if Instant::now() >= deadline {
            return Err("Clipboard helper timed out");
        }
        match child.0.try_wait() {
            Ok(Some(status)) => return exit_result(status.success()),
            Ok(None) => std::thread::sleep(POLL),
            Err(_) => return Err("Clipboard helper wait failed"),
        }
    }
}

fn exit_result(success: bool) -> Result<(), &'static str> {
    if success {
        Ok(())
    } else {
        Err("Clipboard helper exited unsuccessfully")
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn nonblocking(input: &ChildStdin) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn fcntl(fd: std::ffi::c_int, cmd: std::ffi::c_int, ...) -> std::ffi::c_int;
    }
    #[cfg(target_os = "linux")]
    const O_NONBLOCK: i32 = 0x800;
    #[cfg(target_os = "macos")]
    const O_NONBLOCK: i32 = 0x4;
    // SAFETY: stdin owns a live fd; F_GETFL=3 / F_SETFL=4 use integer flags.
    let flags = unsafe { fcntl(input.as_raw_fd(), 3) };
    if flags < 0 || unsafe { fcntl(input.as_raw_fd(), 4, flags | O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn nonblocking(input: &ChildStdin) -> io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetNamedPipeHandleState(
            pipe: *mut c_void,
            mode: *const u32,
            max_collection_count: *const u32,
            collect_data_timeout: *const u32,
        ) -> i32;
    }
    let mode = 1u32; // PIPE_NOWAIT; anonymous pipes are implemented as named pipes.
                     // SAFETY: stdin owns a live pipe handle; pointers remain valid for this call.
    if unsafe {
        SetNamedPipeHandleState(
            input.as_raw_handle(),
            &mode,
            std::ptr::null(),
            std::ptr::null(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn nonblocking(_: &ChildStdin) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_limit_is_in_bytes_and_inclusive() {
        assert_eq!(check_size(0), Ok(()));
        assert_eq!(check_size(MAX_BYTES), Ok(()));
        assert!(check_size(MAX_BYTES + 1).is_err());
        assert!(check_size("界".repeat(MAX_BYTES / 3 + 1).len()).is_err());
    }

    #[test]
    fn linux_selection_requires_the_corresponding_display() {
        assert!(linux_helpers(false, false).is_empty());
        assert_eq!(linux_helpers(true, false), vec![("wl-copy", &[][..])]);
        assert_eq!(linux_helpers(false, true).len(), 2);
        let helpers = linux_helpers(true, true);
        assert_eq!(helpers[0].0, "wl-copy");
        assert_eq!(helpers[1], ("xclip", &["-selection", "clipboard"][..]));
        assert_eq!(helpers[2], ("xsel", &["--clipboard", "--input"][..]));
    }

    #[test]
    fn powershell_uses_a_fixed_utf8_read_to_end_script() {
        assert_eq!(POWERSHELL.0, "powershell.exe");
        assert_eq!(
            &POWERSHELL.1[..3],
            &["-NoProfile", "-NonInteractive", "-Command"]
        );
        assert!(POWERSHELL.1[3].contains("InputEncoding=[System.Text.UTF8Encoding]"));
        assert!(POWERSHELL.1[3].ends_with("Set-Clipboard -Value ([Console]::In.ReadToEnd())"));
    }

    #[test]
    fn only_successful_exit_is_accepted() {
        assert_eq!(exit_result(true), Ok(()));
        assert_eq!(
            exit_result(false),
            Err("Clipboard helper exited unsuccessfully")
        );
    }
}
