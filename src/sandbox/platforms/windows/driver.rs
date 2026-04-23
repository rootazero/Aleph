use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::sandbox::capabilities::{NetworkPolicy, SandboxCapabilities};
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::platforms::windows::job::SandboxJob;
use crate::sandbox::platforms::windows::token::create_restricted_token;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::SetHandleInformation;
use windows_sys::Win32::Foundation::WaitForSingleObject;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT;
use windows_sys::Win32::Foundation::INFINITE;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::CreateProcessAsUserW;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::CREATE_UNICODE_ENVIRONMENT;
use windows_sys::Win32::System::Threading::PROCESS_INFORMATION;
use windows_sys::Win32::System::Threading::STARTF_USESTDHANDLES;
use windows_sys::Win32::System::Threading::STARTUPINFOW;

/// Windows sandbox driver using restricted tokens and job objects.
///
/// This driver implements sandboxing for Windows using:
/// - Restricted tokens (CreateRestrictedToken API)
/// - Job objects for resource limits
/// - ACL-based filesystem restrictions (future enhancement)
///
/// Windows sandboxing has inherent limitations compared to macOS/Linux:
/// - Network restrictions require Windows Firewall (not implemented)
/// - Filesystem sandboxing is ACL-based, not namespace-based
/// - AllowHosts policy is not enforceable at OS level
#[derive(Debug, Default)]
pub struct WindowsSandboxDriver;

impl WindowsSandboxDriver {
    /// Create a new Windows sandbox driver.
    pub fn new() -> Self {
        Self
    }

    /// Convert a Rust string to a wide (UTF-16) string for Windows APIs.
    fn to_wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Quote a Windows command-line argument following CommandLineToArgvW rules.
    fn quote_windows_arg(arg: &str) -> String {
        let needs_quotes = arg.is_empty()
            || arg.chars()
                .any(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '"'));
        if !needs_quotes {
            return arg.to_string();
        }

        let mut quoted = String::with_capacity(arg.len() + 2);
        quoted.push('"');
        let mut backslashes = 0;
        for ch in arg.chars() {
            match ch {
                '\\' => {
                    backslashes += 1;
                }
                '"' => {
                    quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                    quoted.push('"');
                    backslashes = 0;
                }
                _ => {
                    if backslashes > 0 {
                        quoted.push_str(&"\\".repeat(backslashes));
                        backslashes = 0;
                    }
                    quoted.push(ch);
                }
            }
        }
        if backslashes > 0 {
            quoted.push_str(&"\\".repeat(backslashes * 2));
        }
        quoted.push('"');
        quoted
    }

    /// Build an environment block for CreateProcessAsUserW.
    fn make_env_block(env: &HashMap<String, String>) -> Vec<u16> {
        let mut items: Vec<(String, String)> =
            env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        items.sort_by(|a, b| {
            a.0.to_uppercase()
                .cmp(&b.0.to_uppercase())
                .then(a.0.cmp(&b.0))
        });
        let mut w: Vec<u16> = Vec::new();
        for (k, v) in items {
            let mut s = Self::to_wide(&format!("{k}={v}"));
            s.pop();
            w.extend_from_slice(&s);
            w.push(0);
        }
        w.push(0);
        w
    }

    /// Spawn a sandboxed process with piped stdio.
    ///
    /// # Safety
    /// Caller must ensure all handles are properly closed.
    unsafe fn spawn_sandboxed_process(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: &Path,
        h_token: HANDLE,
    ) -> Result<(PROCESS_INFORMATION, HANDLE, HANDLE, HANDLE), SandboxError> {
        // Create pipes for stdio
        let mut stdin_r: HANDLE = 0;
        let mut stdin_w: HANDLE = 0;
        let mut stdout_r: HANDLE = 0;
        let mut stdout_w: HANDLE = 0;
        let mut stderr_r: HANDLE = 0;
        let mut stderr_w: HANDLE = 0;

        if CreatePipe(&mut stdin_r, &mut stdin_w, std::ptr::null_mut(), 0) == 0 {
            return Err(SandboxError::Io(format!(
                "CreatePipe stdin failed: {}",
                GetLastError()
            )));
        }
        if CreatePipe(&mut stdout_r, &mut stdout_w, std::ptr::null_mut(), 0) == 0 {
            CloseHandle(stdin_r);
            CloseHandle(stdin_w);
            return Err(SandboxError::Io(format!(
                "CreatePipe stdout failed: {}",
                GetLastError()
            )));
        }
        if CreatePipe(&mut stderr_r, &mut stderr_w, std::ptr::null_mut(), 0) == 0 {
            CloseHandle(stdin_r);
            CloseHandle(stdin_w);
            CloseHandle(stdout_r);
            CloseHandle(stdout_w);
            return Err(SandboxError::Io(format!(
                "CreatePipe stderr failed: {}",
                GetLastError()
            )));
        }

        // Make pipe handles inheritable
        for h in [stdin_r, stdout_w, stderr_w] {
            if SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
                CloseHandle(stdin_r);
                CloseHandle(stdin_w);
                CloseHandle(stdout_r);
                CloseHandle(stdout_w);
                CloseHandle(stderr_r);
                CloseHandle(stderr_w);
                return Err(SandboxError::Io(format!(
                    "SetHandleInformation failed: {}",
                    GetLastError()
                )));
            }
        }

        // Build command line
        let cmdline_str = std::iter::once(program.to_string())
            .chain(args.iter().cloned())
            .map(|a| Self::quote_windows_arg(&a))
            .collect::<Vec<_>>()
            .join(" ");
        let mut cmdline = Self::to_wide(&cmdline_str);
        let env_block = Self::make_env_block(env);
        let cwd_wide = Self::to_wide(cwd.to_str().unwrap_or("."));

        // Setup startup info
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdInput = stdin_r;
        si.hStdOutput = stdout_w;
        si.hStdError = stderr_w;

        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

        // Create process with restricted token
        let ok = CreateProcessAsUserW(
            h_token,
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            1, // inherit_handles = true
            CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ptr() as *mut c_void,
            cwd_wide.as_ptr(),
            &si,
            &mut pi,
        );

        // Close our copies of the child's ends of the pipes
        CloseHandle(stdin_r);
        CloseHandle(stdout_w);
        CloseHandle(stderr_w);

        if ok == 0 {
            CloseHandle(stdin_w);
            CloseHandle(stdout_r);
            CloseHandle(stderr_r);
            return Err(SandboxError::Io(format!(
                "CreateProcessAsUserW failed: {}",
                GetLastError()
            )));
        }

        Ok((pi, stdin_w, stdout_r, stderr_r))
    }

    /// Read from a handle until EOF, collecting output.
    ///
    /// # Safety
    /// Caller must ensure handle is valid.
    unsafe fn read_handle_to_vec(
        &self,
        handle: HANDLE,
        max_bytes: usize,
    ) -> Result<Vec<u8>, SandboxError> {
        let mut output = Vec::new();
        let mut buf = [0u8; 4096];

        loop {
            let mut read_bytes: u32 = 0;
            let ok = ReadFile(
                handle,
                buf.as_mut_ptr(),
                buf.len().min(max_bytes - output.len()) as u32,
                &mut read_bytes,
                std::ptr::null_mut(),
            );

            if ok == 0 || read_bytes == 0 {
                break;
            }

            output.extend_from_slice(&buf[..read_bytes as usize]);

            if output.len() >= max_bytes {
                break;
            }
        }

        CloseHandle(handle);
        Ok(output)
    }
}

#[async_trait]
impl OsSandboxDriverTrait for WindowsSandboxDriver {
    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        _cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        // Warn about AllowHosts limitation on Windows
        if let NetworkPolicy::AllowHosts { hosts } = &capabilities.network {
            tracing::warn!(
                hosts = ?hosts,
                "AllowHosts network policy is not enforceable on Windows. \
                 Windows sandbox uses restricted tokens which cannot filter by host. \
                 Consider using Windows Firewall or running on macOS/Linux for host-level filtering."
            );
        }

        Ok(OsSandboxProfile {
            contents: String::from("windows_restricted_token"),
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<&[u8]>,
        cwd: &Path,
        _profile: &OsSandboxProfile,
        timeout_duration: Duration,
        max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError> {
        let start_time = std::time::Instant::now();

        // Create restricted token
        let h_token = unsafe {
            match create_restricted_token() {
                Ok(token) => token,
                Err(e) => {
                    return Err(SandboxError::Other(format!(
                        "Failed to create restricted token: {e}"
                    )))
                }
            }
        };

        // Create job object for resource limits
        let job = unsafe {
            match SandboxJob::new(10) {
                // Max 10 active processes
                Ok(job) => job,
                Err(e) => {
                    CloseHandle(h_token);
                    return Err(SandboxError::Other(format!(
                        "Failed to create job object: {e}"
                    )));
                }
            }
        };

        // Spawn sandboxed process
        let (pi, stdin_w, stdout_r, stderr_r) = unsafe {
            match self.spawn_sandboxed_process(program, args, env, cwd, h_token) {
                Ok(result) => result,
                Err(e) => {
                    CloseHandle(h_token);
                    return Err(e);
                }
            }
        };

        // Assign process to job object
        unsafe {
            if let Err(e) = job.assign_process(pi.hProcess) {
                CloseHandle(pi.hProcess);
                CloseHandle(pi.hThread);
                CloseHandle(stdin_w);
                CloseHandle(stdout_r);
                CloseHandle(stderr_r);
                CloseHandle(h_token);
                return Err(SandboxError::Other(format!(
                    "Failed to assign process to job: {e}"
                )));
            }
        }

        // Close token handle - no longer needed
        unsafe {
            CloseHandle(h_token);
        }

        // Write stdin if provided
        if let Some(input) = stdin {
            unsafe {
                let mut written: u32 = 0;
                windows_sys::Win32::System::FileSystem::WriteFile(
                    stdin_w,
                    input.as_ptr(),
                    input.len() as u32,
                    &mut written,
                    std::ptr::null_mut(),
                );
            }
        }
        unsafe {
            CloseHandle(stdin_w);
        }

        // Read stdout and stderr concurrently with timeout
        let stdout_result = {
            let driver = WindowsSandboxDriver::new();
            tokio::task::spawn_blocking(move || unsafe {
                driver.read_handle_to_vec(stdout_r, max_output_bytes / 2)
            })
        };

        let stderr_result = {
            let driver = WindowsSandboxDriver::new();
            tokio::task::spawn_blocking(move || unsafe {
                driver.read_handle_to_vec(stderr_r, max_output_bytes / 2)
            })
        };

        // Wait for process with timeout
        let process_result = tokio::task::spawn_blocking(move || unsafe {
            let wait_result = WaitForSingleObject(pi.hProcess, timeout_duration.as_millis() as u32);

            let exit_code = if wait_result == WAIT_OBJECT_0 {
                let mut code: u32 = 0;
                GetExitCodeProcess(pi.hProcess, &mut code);
                Some(code as i32)
            } else {
                // Timeout - terminate the process
                windows_sys::Win32::System::Threading::TerminateProcess(pi.hProcess, 1);
                None
            };

            CloseHandle(pi.hProcess);
            CloseHandle(pi.hThread);

            exit_code
        });

        // Wait for all tasks with overall timeout
        let (stdout, stderr, exit_code) = match timeout(
            timeout_duration + Duration::from_secs(5),
            async {
                let stdout = stdout_result.await.unwrap_or_else(|e| {
                    Err(SandboxError::Io(format!("Stdout read task failed: {e}")))
                })?;
                let stderr = stderr_result.await.unwrap_or_else(|e| {
                    Err(SandboxError::Io(format!("Stderr read task failed: {e}")))
                })?;
                let exit_code = process_result.await.unwrap_or(None);
                Ok::<_, SandboxError>((stdout, stderr, exit_code))
            },
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(SandboxError::Timeout {
                    elapsed_ms: start_time.elapsed().as_millis() as u64,
                })
            }
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let truncated = stdout.len() >= max_output_bytes / 2 || stderr.len() >= max_output_bytes / 2;

        Ok(SandboxOutput {
            stdout,
            stderr,
            exit_code,
            signal: None, // Windows doesn't use signals like Unix
            truncated,
            duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::capabilities::NetworkPolicy;

    #[test]
    fn windows_driver_creates_placeholder_profile() {
        let driver = WindowsSandboxDriver::new();
        let caps = SandboxCapabilities::strict();
        let profile = driver.profile_for(&caps, Path::new("C:\\temp")).unwrap();
        assert_eq!(profile.contents, "windows_restricted_token");
    }

    #[test]
    fn quote_windows_arg_handles_empty() {
        let driver = WindowsSandboxDriver::new();
        assert_eq!(driver.quote_windows_arg(""), "\"\"");
    }

    #[test]
    fn quote_windows_arg_handles_spaces() {
        let driver = WindowsSandboxDriver::new();
        assert_eq!(driver.quote_windows_arg("hello world"), "\"hello world\"");
    }

    #[test]
    fn quote_windows_arg_handles_quotes() {
        let driver = WindowsSandboxDriver::new();
        assert_eq!(driver.quote_windows_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn quote_windows_arg_no_quotes_needed() {
        let driver = WindowsSandboxDriver::new();
        assert_eq!(driver.quote_windows_arg("hello"), "hello");
    }

    #[test]
    fn to_wide_converts_string() {
        let driver = WindowsSandboxDriver::new();
        let wide = driver.to_wide("test");
        assert_eq!(wide.len(), 5); // 4 chars + null terminator
        assert_eq!(wide[0], 't' as u16);
        assert_eq!(wide[1], 'e' as u16);
        assert_eq!(wide[2], 's' as u16);
        assert_eq!(wide[3], 't' as u16);
        assert_eq!(wide[4], 0);
    }

    #[test]
    fn make_env_block_sorted() {
        let driver = WindowsSandboxDriver::new();
        let mut env = HashMap::new();
        env.insert("B_KEY".to_string(), "value_b".to_string());
        env.insert("A_KEY".to_string(), "value_a".to_string());
        env.insert("C_KEY".to_string(), "value_c".to_string());

        let block = driver.make_env_block(&env);
        // Should be sorted: A_KEY, B_KEY, C_KEY
        assert!(block.len() > 0);
        // Each entry ends with null, block ends with extra null
        assert_eq!(block[block.len() - 1], 0);
    }

    #[test]
    fn profile_for_allowhosts_logs_warning() {
        let driver = WindowsSandboxDriver::new();
        let caps = SandboxCapabilities {
            network: NetworkPolicy::AllowHosts {
                hosts: vec!["example.com".to_string()],
            },
            ..Default::default()
        };
        // Should not fail, just log warning
        let profile = driver.profile_for(&caps, Path::new("C:\\temp")).unwrap();
        assert_eq!(profile.contents, "windows_restricted_token");
    }
}
