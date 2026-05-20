//! OsSandboxDriverTrait — the seam between WorkspaceSandbox and OS-level seatbelt.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};

/// OS-specific seatbelt / sandbox-exec profile bytes or handle.
///
/// `contents` carries the platform-native policy text (SBPL on macOS,
/// bwrap arg list on Linux, key=value lines on Windows).
///
/// `max_memory_mb`, when `Some`, is honored by the driver's `run` method:
/// - macOS / Linux: `setrlimit(RLIMIT_AS)` via `pre_exec` before spawn.
/// - Windows: `JOBOBJECT_EXTENDED_LIMIT_INFORMATION.ProcessMemoryLimit`.
///   Threaded through the profile (rather than re-parsed from `contents`)
///   so the run path does not depend on text round-tripping.
#[derive(Debug, Clone)]
pub struct OsSandboxProfile {
    pub contents: String,
    pub max_memory_mb: Option<u64>,
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
