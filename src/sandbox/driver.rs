//! `OsSandboxDriverTrait` — the seam between `WorkspaceSandbox` and OS-level seatbelt.

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
    /// SP-2 (Linux only): when `Some(json)`, the driver wraps the target
    /// program with `aleph-server sandbox-init --policy <json> --`
    /// inside the bwrap namespace, applying landlock + seccomp before
    /// the target executes. macOS / Windows leave as `None`.
    #[doc(hidden)]
    pub linux_init_policy: Option<String>,
    /// SP-3a (Windows only): when `Some(json)`, the driver wraps the
    /// target program with `aleph-server sandbox-init-windows --policy
    /// <json> --`, which applies a Chrome-pattern restricted token + Low
    /// integrity level before launching the target via
    /// `CreateProcessAsUserW`. Linux / macOS leave as `None`.
    #[doc(hidden)]
    pub windows_init_policy: Option<String>,
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

    /// Substrings **this backend and only this backend** writes into a
    /// sandboxed process's stderr when *it* refused an effect: EPERM's
    /// "Operation not permitted" under seatbelt, EACCES / EROFS under
    /// bwrap + landlock, "Access is denied" under a restricted token.
    ///
    /// Never a union across backends: a union lets a consumer report a denial
    /// in a dialect the running backend cannot produce. Declare entries
    /// lowercase — [`crate::sandbox::command::SandboxDenialHint::detect`]
    /// matches case-insensitively and quotes the entry verbatim.
    ///
    /// Defaulted to `&[]` so a driver with no dialect to declare — every test
    /// double, and any future backend before its dialect is observed — stays
    /// silent instead of guessing.
    fn denial_signatures(&self) -> &'static [&'static str] {
        &[]
    }

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
