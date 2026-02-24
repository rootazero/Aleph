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
mod manifest; // kept for legacy migration
pub mod probe;

// Re-exports
pub use capability::{format_entries_for_prompt, RuntimeCapability};
pub use ensure::ensure_capability;
pub use ledger::{CapabilityEntry, CapabilityLedger, CapabilitySource, CapabilityStatus};
pub use probe::ProbeResult;

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
