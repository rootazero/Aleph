//! Windows `EscapeAbort` implementation using a low-level keyboard hook.

#[cfg(windows)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::Result;

/// Windows escape listener using a `WH_KEYBOARD_LL` hook.
pub struct WindowsEscapeListener {
    aborted: AtomicBool,
    /// Installed hook handle, stored as its `HHOOK` address (`None` = not
    /// installed). Kept as an integer rather than the raw-pointer `HHOOK`
    /// newtype so the struct stays `Send + Sync` (required by `DesktopPlatform`).
    #[cfg_attr(not(windows), allow(dead_code))]
    hook: Mutex<Option<isize>>,
}

/// Address of the active listener, published so the process-global
/// `keyboard_hook_proc` can reach it. Stored as a `usize` (`0` = none) so the
/// static is `Sync` — a raw pointer would not be.
#[cfg(windows)]
static LISTENER_PTR: AtomicUsize = AtomicUsize::new(0);

impl WindowsEscapeListener {
    pub fn new() -> Self {
        Self {
            aborted: AtomicBool::new(false),
            hook: Mutex::new(None),
        }
    }
}

impl Default for WindowsEscapeListener {
    fn default() -> Self {
        Self::new()
    }
}

impl EscapeAbort for WindowsEscapeListener {
    fn start(&self) -> Result<()> {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::HINSTANCE;
            use windows::Win32::System::LibraryLoader::GetModuleHandleW;
            use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_KEYBOARD_LL};

            LISTENER_PTR.store(self as *const Self as usize, Ordering::SeqCst);

            // SAFETY: `GetModuleHandleW(None)` returns the handle of the calling
            // module; it is a documented, side-effect-free Win32 call.
            let hmod = unsafe { GetModuleHandleW(None) }.map_err(|e| {
                aleph_desktop::DesktopError::PlatformError(format!(
                    "failed to get module handle: {e}"
                ))
            })?;

            // SAFETY: installs a process-wide low-level keyboard hook bound to
            // the `'static` `keyboard_hook_proc`; removed again by `stop`.
            let hook = unsafe {
                SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(keyboard_hook_proc),
                    HINSTANCE(hmod.0),
                    0,
                )
            }
            .map_err(|e| {
                aleph_desktop::DesktopError::PlatformError(format!(
                    "failed to install keyboard hook: {e}"
                ))
            })?;

            *self.hook.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook.0 as isize);
            Ok(())
        }
        #[cfg(not(windows))]
        {
            Err(aleph_desktop::DesktopError::NotImplemented(
                "EscapeAbort requires Windows".into(),
            ))
        }
    }

    fn stop(&self) {
        #[cfg(windows)]
        {
            use windows::Win32::UI::WindowsAndMessaging::{UnhookWindowsHookEx, HHOOK};

            if let Some(addr) = self.hook.lock().unwrap_or_else(|e| e.into_inner()).take() {
                // SAFETY: `addr` is an `HHOOK` previously returned by
                // `SetWindowsHookExW` and not yet unhooked.
                let _ = unsafe { UnhookWindowsHookEx(HHOOK(addr as *mut core::ffi::c_void)) };
            }
            LISTENER_PTR.store(0, Ordering::SeqCst);
        }
    }

    fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }

    fn reset(&self) {
        self.aborted.store(false, Ordering::SeqCst);
    }
}

impl Drop for WindowsEscapeListener {
    /// Unhook the keyboard hook and clear `LISTENER_PTR`. Without this, a
    /// listener dropped while still started leaves `LISTENER_PTR` dangling —
    /// the next Escape keypress would dereference freed memory in
    /// `keyboard_hook_proc` — and leaks the `HHOOK`. Mirrors the macOS
    /// `EscapeListener` `Drop` impl.
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
extern "system" fn keyboard_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
    use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, KBDLLHOOKSTRUCT, WM_KEYDOWN};

    if code >= 0 && wparam.0 as u32 == WM_KEYDOWN {
        // SAFETY: for `WH_KEYBOARD_LL` with `code >= 0`, `lparam` points to a
        // `KBDLLHOOKSTRUCT` owned by the OS for the duration of this callback.
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if kb.vkCode == VK_ESCAPE.0 as u32 {
            let addr = LISTENER_PTR.load(Ordering::SeqCst);
            if addr != 0 {
                // SAFETY: `addr` points to a live `WindowsEscapeListener`; its
                // `Drop` runs `stop`, which clears `LISTENER_PTR` before the
                // listener's memory is freed.
                let listener = unsafe { &*(addr as *const WindowsEscapeListener) };
                listener.aborted.store(true, Ordering::SeqCst);
                return LRESULT(1);
            }
        }
    }

    // SAFETY: documented Win32 hook-chaining call.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _esc = WindowsEscapeListener::default();
    }

    #[test]
    fn lifecycle() {
        let esc = WindowsEscapeListener::new();
        assert!(!esc.is_aborted());

        let started = esc.start();
        if started.is_ok() {
            assert!(!esc.is_aborted());
            esc.reset();
            assert!(!esc.is_aborted());
            esc.stop();
        }
    }
}
