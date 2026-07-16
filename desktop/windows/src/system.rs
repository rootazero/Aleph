//! Windows `SystemCapability` implementation using Win32 APIs.

use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;

pub struct WindowsSystem {
    _private: (),
}

impl WindowsSystem {
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WindowsSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemCapability for WindowsSystem {
    async fn launch_app(&self, app_name: &str) -> Result<()> {
        #[cfg(windows)]
        {
            let app_name = app_name.to_string();
            tokio::task::spawn_blocking(move || {
                use windows::core::PCWSTR;
                use windows::Win32::UI::Shell::ShellExecuteW;
                use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

                let operation: Vec<u16> = "open\0".encode_utf16().collect();
                let file: Vec<u16> = app_name.encode_utf16().chain(std::iter::once(0)).collect();

                // SAFETY: ShellExecuteW is a well-documented Win32 API. PCWSTR points to valid null-terminated UTF-16.
                let result = unsafe {
                    ShellExecuteW(
                        None,
                        PCWSTR(operation.as_ptr()),
                        PCWSTR(file.as_ptr()),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        SW_SHOWNORMAL,
                    )
                };

                // ShellExecuteW returns an HINSTANCE > 32 on success.
                let code = result.0 as isize;
                if code > 32 {
                    Ok(())
                } else {
                    Err(DesktopError::PlatformError(format!(
                        "failed to launch '{app_name}': ShellExecute returned {code}"
                    )))
                }
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            let _ = app_name;
            Err(DesktopError::NotImplemented(
                "launch_app requires Windows".into(),
            ))
        }
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        // Delegate to the shared cross-platform implementation, which matches by
        // process executable name. The previous local implementation matched by
        // window-title substring, so `quit_app("Word")` could also close
        // unrelated apps such as "Password Manager".
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || aleph_desktop::action::quit_app(&app_name))
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }

    async fn list_running_apps(&self) -> Result<Vec<AppInfo>> {
        #[cfg(windows)]
        {
            tokio::task::spawn_blocking(|| {
                use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
                use windows::Win32::UI::WindowsAndMessaging::{
                    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
                };

                struct EnumState {
                    apps: Vec<AppInfo>,
                }

                // SAFETY: EnumWindows callback follows documented signature.
                extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
                    // SAFETY: `lparam` carries the `&mut EnumState` we pass to
                    // `EnumWindows`, which outlives this synchronous enumeration;
                    // remaining calls are documented Win32 APIs.
                    unsafe {
                        if IsWindowVisible(hwnd).as_bool() {
                            let mut buf = [0u16; 512];
                            let len = GetWindowTextW(hwnd, &mut buf);
                            if len > 0 {
                                let title = String::from_utf16_lossy(&buf[..len as usize]);
                                let mut pid: u32 = 0;
                                GetWindowThreadProcessId(hwnd, Some(std::ptr::addr_of_mut!(pid)));

                                let state = &mut *(lparam.0 as *mut EnumState);
                                state.apps.push(AppInfo {
                                    name: title,
                                    bundle_id: String::new(),
                                    pid: Some(u64::from(pid)),
                                    is_active: false,
                                });
                            }
                        }
                        BOOL(1)
                    }
                }

                let mut state = EnumState { apps: Vec::new() };
                // SAFETY: `enum_proc` matches `WNDENUMPROC`; `state` outlives the
                // synchronous enumeration.
                unsafe {
                    let _ = EnumWindows(
                        Some(enum_proc),
                        LPARAM(std::ptr::addr_of_mut!(state) as isize),
                    );
                }

                Ok(state.apps)
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            Err(DesktopError::NotImplemented(
                "list_running_apps requires Windows".into(),
            ))
        }
    }

    async fn send_notification(&self, title: &str, body: &str) -> Result<()> {
        #[cfg(windows)]
        {
            let title = title.to_string();
            let body = body.to_string();
            tokio::task::spawn_blocking(move || {
                // Use PowerShell to show a toast notification via WinRT APIs.
                let script = format!(
                    r#"Add-Type -AssemblyName System.Runtime.WindowsRuntime;
$null = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType=WindowsRuntime];
$template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02);
$template.GetElementsByTagName('text').Item(0).AppendChild($template.CreateTextNode('{}')) | Out-Null;
$template.GetElementsByTagName('text').Item(1).AppendChild($template.CreateTextNode('{}')) | Out-Null;
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Aleph').Show([Windows.UI.Notifications.ToastNotification]::new($template));"#,
                    title.replace('\'', "''"),
                    body.replace('\'', "''")
                );

                match std::process::Command::new("powershell.exe")
                    .args(["-NoProfile", "-Command", &script])
                    .output()
                {
                    Ok(output) if output.status.success() => Ok(()),
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        Err(DesktopError::PlatformError(format!(
                            "toast notification failed: {stderr}"
                        )))
                    }
                    Err(e) => Err(DesktopError::PlatformError(format!(
                        "failed to spawn powershell for notification: {e}"
                    ))),
                }
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            let _ = (title, body);
            Err(DesktopError::NotImplemented(
                "send_notification requires Windows".into(),
            ))
        }
    }

    async fn clipboard_read(&self) -> Result<ClipboardContent> {
        #[cfg(windows)]
        {
            tokio::task::spawn_blocking(|| {
                use clipboard_win::{formats, get_clipboard};

                match get_clipboard::<String, _>(formats::Unicode) {
                    Ok(text) => Ok(ClipboardContent {
                        text: Some(text),
                        has_image: false,
                        image_base64: None,
                    }),
                    Err(e) => Err(DesktopError::InputFailed(format!(
                        "clipboard read failed: {e}"
                    ))),
                }
            })
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            Err(DesktopError::NotImplemented(
                "clipboard_read requires Windows".into(),
            ))
        }
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        #[cfg(windows)]
        {
            let text = text.to_string();
            tokio::task::spawn_blocking(move || {
                use clipboard_win::{formats, set_clipboard};

                match set_clipboard(formats::Unicode, text.as_str()) {
                    Ok(()) => Ok(()),
                    Err(e) => Err(DesktopError::InputFailed(format!(
                        "clipboard write failed: {e}"
                    ))),
                }
            })
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            let _ = text;
            Err(DesktopError::NotImplemented(
                "clipboard_write requires Windows".into(),
            ))
        }
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        #[cfg(windows)]
        {
            tokio::task::spawn_blocking(|| {
                use windows::core::PWSTR;
                use windows::Win32::System::SystemInformation::{
                    ComputerNamePhysicalDnsHostname, GetComputerNameExW,
                };

                let mut hostname_buf = [0u16; 256];
                let mut size = 256u32;

                // SAFETY: GetComputerNameExW writes up to `size` UTF-16 units
                // into the buffer and updates `size` with the count written.
                let hostname = unsafe {
                    if GetComputerNameExW(
                        ComputerNamePhysicalDnsHostname,
                        PWSTR(hostname_buf.as_mut_ptr()),
                        std::ptr::addr_of_mut!(size),
                    )
                    .is_ok()
                    {
                        String::from_utf16_lossy(&hostname_buf[..size as usize])
                    } else {
                        "unknown".to_string()
                    }
                };

                let username = std::env::var("USERNAME").unwrap_or_else(|_| "unknown".to_string());

                Ok(SystemInfo {
                    os_name: "Windows".to_string(),
                    os_version: std::env::var("OS").unwrap_or_else(|_| "Windows NT".to_string()),
                    hostname,
                    arch: std::env::consts::ARCH.to_string(),
                    username,
                })
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            Err(DesktopError::NotImplemented(
                "system_info requires Windows".into(),
            ))
        }
    }

    async fn user_idle_seconds(&self) -> Result<f64> {
        #[cfg(windows)]
        {
            tokio::task::spawn_blocking(|| {
                use windows::Win32::System::SystemInformation::GetTickCount64;
                use windows::Win32::UI::Input::KeyboardAndMouse::GetLastInputInfo;

                #[repr(C)]
                #[allow(non_snake_case, clippy::upper_case_acronyms)]
                // Win32 ABI struct — name & field casing must match the Win32 header.
                struct LASTINPUTINFO {
                    cbSize: u32,
                    dwTime: u32,
                }

                let mut info = LASTINPUTINFO {
                    cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
                    dwTime: 0,
                };

                // SAFETY: GetLastInputInfo writes into the struct on success;
                // GetTickCount64 returns u64 millis since boot (no 49.7-day wrap).
                let (ok, now_ms, last_ms) = unsafe {
                    let ok = GetLastInputInfo(std::ptr::addr_of_mut!(info) as *mut _).as_bool();
                    (ok, GetTickCount64(), info.dwTime)
                };

                if !ok {
                    return Err(DesktopError::PlatformError(
                        "GetLastInputInfo failed".into(),
                    ));
                }

                // dwTime is the low 32 bits of the boot tick counter at the moment
                // of the last input event. GetTickCount64 advances past 2^32 ms
                // after ~49.7 days; reconstruct the full 64-bit timestamp by
                // combining the high 32 bits of `now_ms` with `last_ms`, then
                // rolling back one wrap if the result is in the future.
                let high = now_ms & 0xFFFF_FFFF_0000_0000;
                let mut last_full = high | (last_ms as u64);
                if last_full > now_ms {
                    last_full = last_full.wrapping_sub(1u64 << 32);
                }
                let idle_millis = now_ms.saturating_sub(last_full);

                Ok((idle_millis as f64) / 1000.0)
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            Err(DesktopError::NotImplemented(
                "user_idle_seconds requires Windows".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _sys = WindowsSystem::default();
    }
}
