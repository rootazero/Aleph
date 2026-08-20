//! Marketplace types — core data structures for the plugin marketplace system.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// =============================================================================
// Source Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarketplaceSourceType {
    Github,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceConfig {
    pub source: String,
    #[serde(rename = "type")]
    pub source_type: MarketplaceSourceType,
}

// =============================================================================
// Index / Manifest Types
// =============================================================================

/// Where a marketplace entry's plugin lives.
///
/// Claude Code's `PluginSourceSchema` is a six-arm union: a path string
/// relative to the marketplace root, or an object tagged by a `source`
/// discriminator (`npm` / `pip` / `url` / `github` / `git-subdir`). Aleph
/// modelled this as a bare `String` until 2026-08-19, and because serde fails
/// the *whole* struct on a type mismatch, a single object-form entry made the
/// entire `marketplace.json` unparseable — every plugin in that marketplace
/// became invisible, not just the one entry.
///
/// Aleph installs a marketplace as a directory (local, or a git clone), so the
/// path form is the one it can serve. The object forms still parse — that is
/// the point — and turn into a named, per-entry refusal at install time
/// instead of a whole-manifest rejection. `Unknown` catches arms Claude Code
/// adds later, so a newer marketplace never bricks an older Aleph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MarketplacePluginSource {
    /// A path relative to the marketplace root.
    Path(String),
    /// One of the object forms, kept verbatim. The `source` discriminator is
    /// read back out for the refusal message rather than re-modelled here:
    /// Aleph installs none of them, so modelling their fields would be five
    /// structs with no consumer (R10).
    External(serde_json::Value),
}

impl MarketplacePluginSource {
    /// The relative path inside the marketplace directory, if this entry has
    /// one. `None` means the entry points somewhere Aleph does not fetch.
    #[must_use]
    pub fn as_relative_path(&self) -> Option<&str> {
        match self {
            Self::Path(p) => Some(p.as_str()),
            Self::External(_) => None,
        }
    }

    /// The `source` discriminator of an object form (`"github"`, `"npm"`, …),
    /// for error messages. Returns `None` for the path form.
    #[must_use]
    pub fn external_kind(&self) -> Option<&str> {
        match self {
            Self::Path(_) => None,
            Self::External(v) => v.get("source").and_then(serde_json::Value::as_str),
        }
    }
}

impl std::fmt::Display for MarketplacePluginSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(p) => f.write_str(p),
            Self::External(v) => write!(f, "{v}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplacePluginEntry {
    pub name: String,
    pub source: MarketplacePluginSource,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// SHA-256 hash of the plugin archive (hex-encoded)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceOwner {
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketplaceMetadata {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default, rename = "plugin-root")]
    pub plugin_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceManifest {
    pub name: String,
    #[serde(default)]
    pub owner: Option<MarketplaceOwner>,
    #[serde(default)]
    pub metadata: MarketplaceMetadata,
    #[serde(default)]
    pub plugins: Vec<MarketplacePluginEntry>,
}

// =============================================================================
// Search Result
// =============================================================================

#[derive(Debug, Clone)]
pub struct PluginSearchResult {
    pub marketplace_name: String,
    pub plugin: MarketplacePluginEntry,
    /// Where the plugin sits inside the marketplace directory.
    ///
    /// `None` when the entry declares one of Claude Code's external source
    /// forms (`github` / `npm` / …). The entry is still returned so the
    /// operator gets "this marketplace cannot install that form" rather than
    /// "no such plugin" — the second one sends them looking for a typo.
    pub plugin_path: Option<PathBuf>,
}

impl PluginSearchResult {
    /// The directory to install this entry from, or the reason there is none.
    ///
    /// This is the **one** predicate for "can this entry be installed". Install
    /// and update both go through it, and so does the browse listing that
    /// renders an Install button — a catalogue that offers an action on a row
    /// the action refuses is worse than one that says so up front, and the only
    /// way to keep the two in step is for the renderer and the refusal to be
    /// the same code rather than two readings of the same enum.
    ///
    /// # Errors
    /// Returns the refusal text when the entry declares one of Claude Code's
    /// external source forms (`github` / `npm` / `pip` / `url` / `git-subdir`),
    /// which do not live inside the marketplace directory this host serves.
    pub fn installable_path(&self) -> Result<&Path, String> {
        match self.plugin_path.as_deref() {
            Some(p) => Ok(p),
            None => {
                let kind = self.plugin.source.external_kind().unwrap_or("object");
                let name = &self.plugin.name;
                Err(format!(
                    "Plugin '{name}' declares a '{kind}' source, which this marketplace \
                     cannot install — Aleph serves plugins from the marketplace directory itself. \
                     Add the upstream repository as its own marketplace, or install it directly \
                     with `aleph plugin install <url>`."
                ))
            }
        }
    }
}

/// A marketplace that could not be read, and why.
///
/// Carried alongside the entries rather than logged: a browse surface that
/// silently drops an unreadable marketplace reports "nothing here", which is
/// the same thing it reports for an empty query, and the operator has no way
/// to tell that a `marketplace update` is what they are missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceProblem {
    pub marketplace: String,
    pub reason: String,
}

/// What one browse call found: the entries, and every marketplace it could not
/// read on the way.
#[derive(Debug, Clone, Default)]
pub struct MarketplaceListing {
    pub entries: Vec<PluginSearchResult>,
    pub problems: Vec<MarketplaceProblem>,
}

// =============================================================================
// Constants
// =============================================================================

pub const BUILTIN_MARKETPLACE_NAME: &str = "aleph-official";
/// Builtin marketplace is extracted from bundled content, not cloned from GitHub.
pub const BUILTIN_MARKETPLACE_SOURCE: &str = "bundled";

// =============================================================================
// Path Helpers
// =============================================================================

/// Returns the directory where marketplace repos are cached locally.
#[must_use]
pub fn marketplace_cache_dir() -> PathBuf {
    crate::discovery::aleph_home_dir()
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".aleph")
        })
        .join("plugins/cache")
}

/// Returns the directory where plugins are installed.
#[must_use]
pub fn default_install_dir() -> PathBuf {
    crate::discovery::aleph_home_dir()
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".aleph")
        })
        .join("plugins/installed")
}
