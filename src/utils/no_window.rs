//! Keep child processes windowless on Windows.
//!
//! On Windows 11 the default terminal host (Windows Terminal) pops a visible
//! window for *every* console child spawned without the `CREATE_NO_WINDOW`
//! creation flag. A background daemon that shells out — runtime probes
//! (`where` / `<bin> --version`), MCP/ACP stdio servers, skill scripts, hooks,
//! the sandbox, `op`, cluster bash — would otherwise flash a cascade of
//! terminal windows across the screen (notably during cold-start runtime
//! detection). Routing every spawn through `.no_window()` suppresses that.
//!
//! No-op on non-Windows, where shelling out never spawns a window.

/// `CREATE_NO_WINDOW` — create the process without a console window.
/// <https://learn.microsoft.com/windows/win32/procthread/process-creation-flags>
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Builder extension: call `.no_window()` on a `Command` before spawning to
/// keep the child windowless on Windows. Chains like the other builder methods
/// (`&mut Self`); a no-op off Windows.
///
/// Implemented for both `std::process::Command` and `tokio::process::Command`.
///
/// Note: this sets the process creation flags. If a call site also needs other
/// flags (e.g. `CREATE_NEW_PROCESS_GROUP`), set them together with a single
/// `creation_flags(..)` call rather than combining with this trait — a later
/// `creation_flags` call *replaces* the value, it does not OR into it.
pub trait NoWindow {
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for std::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

impl NoWindow for tokio::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}
