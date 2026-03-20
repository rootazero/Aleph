//! Marketplace types — core data structures for the plugin marketplace system.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplacePluginEntry {
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
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
    pub plugin_path: PathBuf,
}

// =============================================================================
// Constants
// =============================================================================

pub const BUILTIN_MARKETPLACE_NAME: &str = "aleph-official";
pub const BUILTIN_MARKETPLACE_SOURCE: &str = "rootazero/Aleph-plugins";

// =============================================================================
// Path Helpers
// =============================================================================

/// Returns the directory where marketplace repos are cached locally.
pub fn marketplace_cache_dir() -> PathBuf {
    crate::discovery::aleph_home_dir()
        .unwrap_or_else(|_| PathBuf::from(concat!(env!("HOME"), "/.aleph")))
        .join("plugins/cache")
}

/// Returns the directory where plugins are installed.
pub fn default_install_dir() -> PathBuf {
    crate::discovery::aleph_home_dir()
        .unwrap_or_else(|_| PathBuf::from(concat!(env!("HOME"), "/.aleph")))
        .join("plugins/installed")
}
