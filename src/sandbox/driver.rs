//! OsSandboxDriverTrait — the seam between WorkspaceSandbox and OS-level seatbelt.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};

/// OS-specific seatbelt / sandbox-exec profile bytes or handle.
#[derive(Debug, Clone)]
pub struct OsSandboxProfile {
    /// macOS: sandbox-exec SBPL profile text.
    pub contents: String,
}

#[async_trait]
pub trait OsSandboxDriverTrait: Send + Sync + 'static {
    /// Platform identifier in the form `"os/mechanism"` (e.g. `"macos/seatbelt"`,
    /// `"linux/bwrap"`, `"windows/token"`).
    fn platform(&self) -> &'static str;

    /// Whether the sandbox mechanism is available on the current host.
    fn is_supported(&self) -> bool;

    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError>;

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<&[u8]>,
        cwd: &Path,
        profile: &OsSandboxProfile,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError>;
}
