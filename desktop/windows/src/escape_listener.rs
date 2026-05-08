//! Windows `EscapeAbort` implementation using low-level keyboard hook.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::Result;

/// Windows escape listener using `WH_KEYBOARD_LL` hook.
pub struct WindowsEscapeListener {
    aborted: AtomicBool,
    #[cfg(windows)]
    hook: Mutex<Option<windows::Win32::UI::WindowsAndMessaging::HHOOK>>,
    #[cfg(not(windows))]
    _hook: Mutex<Option<()>>,
}

#[cfg(windows)]
static LISTENER_PTR: Mutex<Option<*const WindowsEscapeListener>> = Mutex::new(None);

impl WindowsEscapeListener {
    pub fn new() -> Self {
        Self {
            aborted: AtomicBool::new(false),
            #[cfg(windows)]
            hook: Mutex::new(None),
            #[cfg(not(windows))]
            _hook: Mutex::new(None),
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
            use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
            use windows::Win32::System::LibraryLoader::GetModuleHandleW;
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                SetWindowsHookExW, WH_KEYBOARD_LL,
            };

            {
                let mut guard = LISTENER_PTR.lock().unwrap();
                *guard = Some(self as *const _);
            }

            let hmod = unsafe {
                GetModuleHandleW(None).unwrap_or(HINSTANCE(std::ptr::null_mut()))
            };

            let hook = unsafe {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), hmod, 0)
            };

            let hook = hook.map_err(|e| {
                aleph_desktop::DesktopError::PlatformError(format!(
                    "failed to install keyboard hook: {e}"
                ))
            })?;

            {
                let mut guard = self.hook.lock().unwrap();
                *guard = Some(hook);
            }

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
            use windows::Win32::UI::Input::KeyboardAndMouse::UnhookWindowsHookEx;

            let mut guard = self.hook.lock().unwrap();
            if let Some(hook) = guard.take() {
                let _ = unsafe { UnhookWindowsHookEx(hook) };
            }

            let mut ptr_guard = LISTENER_PTR.lock().unwrap();
            *ptr_guard = None;
        }
    }

    fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }

    fn reset(&self) {
        self.aborted.store(false, Ordering::SeqCst);
    }
}

#[cfg(windows)]
extern "system" fn keyboard_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        CallNextHookEx, KBDLLHOOKSTRUCT, VK_ESCAPE, WM_KEYDOWN,
    };

    if code >= 0 && wparam.0 as u32 == WM_KEYDOWN {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if kb.vkCode == VK_ESCAPE.0 as u32 {
            if let Ok(guard) = LISTENER_PTR.lock() {
                if let Some(ptr) = *guard {
                    let listener = unsafe { &*ptr };
                    listener
                        .aborted
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    return LRESULT(1);
                }
            }
        }
    }

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
