//! Projects-related configuration.
//!
//! Currently a single knob: the filesystem roots the cross-platform
//! `<DirectoryBrowser />` (Panel + future channels) is allowed to traverse
//! via `fs.list_dir` / `fs.create_dir`. This is the *only* safety boundary
//! between a paired client and the server's filesystem for project
//! selection; treat it like the firewall it is.
//!
//! ```toml
//! [projects]
//! allowed_roots = ["~", "/Volumes/Workspace"]
//! ```
//!
//! `~` is expanded to the server's home directory at boot. Paths that
//! don't resolve (don't exist or fail canonicalisation) are dropped with
//! a `warn!` log so a stale entry can't accidentally widen scope.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Projects subsystem configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectsConfig {
    /// Filesystem roots the directory browser may traverse. `~` is allowed
    /// and expanded at boot. Defaults to a single entry — the user's home
    /// directory — which mirrors how every Finder-style picker on macOS
    /// behaves out of the box.
    #[serde(default = "default_allowed_roots")]
    pub allowed_roots: Vec<String>,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self {
            allowed_roots: default_allowed_roots(),
        }
    }
}

fn default_allowed_roots() -> Vec<String> {
    vec!["~".to_string()]
}
