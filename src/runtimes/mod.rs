//! Runtime capability management — lightweight ledger + shell bootstrap
//!
//! Manages external tool capabilities (python, node, uv, ffmpeg, yt-dlp, etc.)
//! using a three-phase approach:
//!
//! 1. **Probe** — detect what's already installed (system PATH + Aleph-managed)
//! 2. **Bootstrap** — install missing tools via shell scripts
//! 3. **Ledger** — persist capability status to `~/.aleph/runtimes/ledger.json`
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::runtimes::{ensure_capability, CapabilityLedger};
//!
//! let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(path)));
//! let bin_path = ensure_capability("python", &ledger).await?;
//! ```

pub mod bootstrap;
mod capability;
pub mod ensure;
pub mod ledger;
pub mod npm_global;
pub mod os;
pub mod post_install;
pub mod probe;
pub mod specs;

// Re-exports
pub use bootstrap::{dependencies, has_spec, install, BootstrapError, BootstrapResult};
pub use capability::format_entries_for_prompt;
pub use ensure::ensure_capability;
pub use ledger::{CapabilityEntry, CapabilityLedger, CapabilitySource, CapabilityStatus};
pub use os::TargetOs;
pub use post_install::PostInstallError;
pub use probe::ProbeResult;
pub use specs::{
    find_spec, select_install, supported_on_current_os, InstallStrategy, OsInstall,
    PostInstallAction, RuntimeSpec, SPECS,
};

use crate::error::Result;
use std::path::PathBuf;

/// Get the runtimes directory path
///
/// Returns platform-specific path:
/// - Unix: `~/.aleph/runtimes/`
/// - Windows: `%USERPROFILE%\.aleph\runtimes\`
pub fn get_runtimes_dir() -> Result<PathBuf> {
    crate::utils::paths::get_runtimes_dir()
}
