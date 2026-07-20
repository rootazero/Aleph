//! Windows `EscapeAbort` implementation using a low-level keyboard hook.

#[cfg(windows)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::Result;

/// State shared between the listener and the global low-level keyboard hook.
/// Kept on the heap so that moving the `WindowsEscapeListener` value does not
/// invalidate the pointer published to `LISTENER_PTR`.
///
/// Compiled on every platform because the always-present `WindowsEscapeListener`
/// stores it (and `is_aborted`/`reset` read it); only its construction is
/// Windows-only, hence `allow(dead_code)` off-Windows.
#[cfg_attr(not(windows), allow(dead_code))]
struct ListenerState {
    aborted: AtomicBool,
}

#[cfg(windows)]
impl ListenerState {
    const fn new() -> Self {
        Self {
            aborted: AtomicBool::new(false),
        }
    }
}

/// Windows escape listener using a `WH_KEYBOARD_LL` hook.
pub struct WindowsEscapeListener {
    /// Installed hook handle, stored as its `HHOOK` address (`None` = not
    /// installed). Kept as an integer rather than the raw-pointer `HHOOK`
    /// newtype so the struct stays `Send + Sync` (required by `DesktopPlatform`).
    #[cfg_attr(not(windows), allow(dead_code))]
    hook: Mutex<Option<isize>>,
    /// Heap-allocated state shared with the hook callback. Moving the listener
    /// does not move this heap allocation, so `LISTENER_PTR` remains valid.
    #[cfg_attr(not(windows), allow(dead_code))]
    state: Mutex<Option<Box<ListenerState>>>,
}

/// Address of the active listener's heap state, published so the process-global
/// `keyboard_hook_proc` can reach it. Stored as a `usize` (`0` = none) so the
/// static is `Sync` — a raw pointer would not be.
#[cfg(windows)]
static LISTENER_PTR: AtomicUsize = AtomicUsize::new(0);

impl WindowsEscapeListener {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            hook: Mutex::new(None),
            state: Mutex::new(None),
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

            let mut state_guard = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state_guard.is_some() {
                return Ok(());
            }
            let state = Box::new(ListenerState::new());
            LISTENER_PTR.store(std::ptr::addr_of!(*state) as usize, Ordering::SeqCst);
            *state_guard = Some(state);
            drop(state_guard);

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
                LISTENER_PTR.store(0, Ordering::SeqCst);
                let _ = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                aleph_desktop::DesktopError::PlatformError(format!(
                    "failed to install keyboard hook: {e}"
                ))
            })?;

            *self
                .hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook.0 as isize);
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

            if let Some(addr) = self
                .hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                // SAFETY: `addr` is an `HHOOK` previously returned by
                // `SetWindowsHookExW` and not yet unhooked.
                let _ = unsafe { UnhookWindowsHookEx(HHOOK(addr as *mut core::ffi::c_void)) };
            }
            // Clear BEFORE dropping: any in-flight `keyboard_hook_proc`
            // that has already loaded LISTENER_PTR into a local would
            // otherwise dereference the freed `ListenerState` (UAF).
            // Yield to let that callback observe the zero, then drop.
            LISTENER_PTR.store(0, Ordering::SeqCst);
            std::thread::yield_now();
            let _ = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
        }
    }

    fn is_aborted(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .as_ref()
            .is_some_and(|s| s.aborted.load(Ordering::SeqCst))
    }

    fn reset(&self) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(s) = state.as_ref() {
            s.aborted.store(false, Ordering::SeqCst);
        }
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
        if kb.vkCode == u32::from(VK_ESCAPE.0) {
            let addr = LISTENER_PTR.load(Ordering::SeqCst);
            if addr != 0 {
                // SAFETY: `addr` points to a live `ListenerState` owned by
                // the started listener; its `Drop` runs via `stop`, which
                // clears `LISTENER_PTR` before the heap allocation is freed.
                let state = unsafe { &*(addr as *const ListenerState) };
                state.aborted.store(true, Ordering::SeqCst);
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
