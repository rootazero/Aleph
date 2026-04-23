use std::sync::Arc;

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};

pub mod common;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "windows")]
pub mod windows;

pub fn create_platform_driver() -> Arc<dyn OsSandboxDriverTrait> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::seatbelt::SeatbeltDriver::new())
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::bwrap::BubblewrapDriver::new())
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(windows::driver::WindowsSandboxDriver::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Arc::new(UnsupportedDriver)
    }
}

pub struct UnsupportedDriver;

#[async_trait]
impl OsSandboxDriverTrait for UnsupportedDriver {
    fn platform(&self) -> &'static str {
        "unsupported"
    }

    fn is_supported(&self) -> bool {
        false
    }

    fn profile_for(
        &self,
        _capabilities: &SandboxCapabilities,
        _cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        Err(SandboxError::ExecutionFailed(
            "sandbox not supported on this platform".into(),
        ))
    }

    async fn run(
        &self,
        _program: &str,
        _args: &[String],
        _env: &HashMap<String, String>,
        _stdin: Option<&[u8]>,
        _cwd: &Path,
        _profile: &OsSandboxProfile,
        _timeout: Duration,
        _max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError> {
        Err(SandboxError::ExecutionFailed(
            "sandbox not supported on this platform".into(),
        ))
    }
}
