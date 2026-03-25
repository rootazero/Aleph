//! ManifestAdapter trait and AdapterRegistry
//!
//! Provides a trait-based system for parsing plugin directories from
//! multiple platform formats (Claude Code, Codex, Cursor, auto-discover).

use std::path::Path;

use anyhow::{anyhow, Result};

use crate::extension::capability::{CapabilityDeclaration, CapabilitySource};

/// Output from a manifest adapter parse operation.
#[derive(Debug)]
pub struct AdapterOutput {
    /// Unique plugin identifier
    pub plugin_id: String,
    /// Human-readable plugin name
    pub name: Option<String>,
    /// Semantic version string
    pub version: Option<String>,
    /// Plugin description
    pub description: Option<String>,
    /// Capabilities declared by this plugin
    pub capabilities: Vec<CapabilityDeclaration>,
    /// Where this plugin was discovered
    pub source: CapabilitySource,
}

/// Trait for parsing plugin directories into capability declarations.
///
/// Each adapter understands one manifest format (e.g., Claude Code, Codex, Cursor).
/// The `detect` method checks whether a directory matches the format, and `parse`
/// extracts the full capability set.
pub trait ManifestAdapter: Send + Sync {
    /// Returns true if `plugin_dir` contains a manifest this adapter can parse.
    fn detect(&self, plugin_dir: &Path) -> bool;

    /// Parse the plugin directory and return all declared capabilities.
    fn parse(&self, plugin_dir: &Path) -> Result<AdapterOutput>;

    /// Human-readable name of this adapter's format (e.g., "claude_code", "codex").
    fn format_name(&self) -> &str;

    /// Priority for ordering. Higher values are tried first.
    fn priority(&self) -> i32 {
        0
    }
}

/// Registry of manifest adapters, tried in priority order.
pub struct AdapterRegistry {
    adapters: Vec<Box<dyn ManifestAdapter>>,
}

impl AdapterRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { adapters: vec![] }
    }

    /// Create a registry with default adapters.
    ///
    /// Registered adapters (by descending priority):
    /// - Claude Code TOML (100)
    /// - Claude Code JSON (90)
    /// - Codex CLI (80)
    /// - Cursor IDE (70)
    /// - Auto-discover (-100)
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(super::cc_plugin_toml::ClaudeCodeTomlAdapter));
        registry.register(Box::new(super::cc_plugin_json::ClaudeCodeJsonAdapter));
        registry.register(Box::new(super::adapters::codex::CodexAdapter));
        registry.register(Box::new(super::adapters::cursor::CursorAdapter));
        registry.register(Box::new(super::adapters::auto_discover::AutoDiscoverAdapter));
        registry
    }

    /// Register a new adapter. Adapters are sorted by descending priority.
    pub fn register(&mut self, adapter: Box<dyn ManifestAdapter>) {
        self.adapters.push(adapter);
        self.adapters.sort_by(|a, b| b.priority().cmp(&a.priority()));
    }

    /// Try each adapter in priority order; return the first successful parse.
    pub fn parse_dir(&self, dir: &Path) -> Result<AdapterOutput> {
        for adapter in &self.adapters {
            if adapter.detect(dir) {
                tracing::debug!(
                    adapter = adapter.format_name(),
                    dir = %dir.display(),
                    "Adapter matched"
                );
                return adapter.parse(dir);
            }
        }
        Err(anyhow!(
            "No manifest adapter matched directory: {}",
            dir.display()
        ))
    }

    /// Number of registered adapters.
    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    /// Returns true if no adapters are registered.
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::capability::{CapabilitySource, SourceFormat};
    use crate::extension::types::PluginOrigin;

    /// A test adapter that always detects and returns a fixed plugin_id.
    struct StubAdapter {
        name: &'static str,
        prio: i32,
        plugin_id: &'static str,
    }

    impl ManifestAdapter for StubAdapter {
        fn detect(&self, _plugin_dir: &Path) -> bool {
            true
        }

        fn parse(&self, _plugin_dir: &Path) -> Result<AdapterOutput> {
            Ok(AdapterOutput {
                plugin_id: self.plugin_id.to_string(),
                name: Some(self.name.to_string()),
                version: None,
                description: None,
                capabilities: vec![],
                source: CapabilitySource {
                    plugin_id: self.plugin_id.to_string(),
                    origin: PluginOrigin::Global,
                    format: SourceFormat::AlephToml,
                },
            })
        }

        fn format_name(&self) -> &str {
            self.name
        }

        fn priority(&self) -> i32 {
            self.prio
        }
    }

    /// Adapter that never matches.
    struct NeverMatchAdapter;

    impl ManifestAdapter for NeverMatchAdapter {
        fn detect(&self, _plugin_dir: &Path) -> bool {
            false
        }

        fn parse(&self, _plugin_dir: &Path) -> Result<AdapterOutput> {
            Err(anyhow!("should not be called"))
        }

        fn format_name(&self) -> &str {
            "never_match"
        }
    }

    #[test]
    fn test_priority_ordering() {
        let mut registry = AdapterRegistry::new();

        registry.register(Box::new(StubAdapter {
            name: "low",
            prio: 1,
            plugin_id: "low-plugin",
        }));
        registry.register(Box::new(StubAdapter {
            name: "high",
            prio: 100,
            plugin_id: "high-plugin",
        }));
        registry.register(Box::new(StubAdapter {
            name: "mid",
            prio: 50,
            plugin_id: "mid-plugin",
        }));

        let output = registry.parse_dir(Path::new("/tmp/test")).unwrap();
        assert_eq!(output.plugin_id, "high-plugin");
    }

    #[test]
    fn test_empty_registry_returns_error() {
        let registry = AdapterRegistry::new();
        let result = registry.parse_dir(Path::new("/tmp/test"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("No manifest adapter matched"));
    }

    #[test]
    fn test_first_match_wins() {
        let mut registry = AdapterRegistry::new();

        // Both have same priority — insertion order preserved by stable sort
        registry.register(Box::new(StubAdapter {
            name: "first",
            prio: 10,
            plugin_id: "first-plugin",
        }));
        registry.register(Box::new(StubAdapter {
            name: "second",
            prio: 10,
            plugin_id: "second-plugin",
        }));

        let output = registry.parse_dir(Path::new("/tmp/test")).unwrap();
        assert_eq!(output.plugin_id, "first-plugin");
    }

    #[test]
    fn test_skips_non_matching_adapters() {
        let mut registry = AdapterRegistry::new();

        registry.register(Box::new(NeverMatchAdapter));
        registry.register(Box::new(StubAdapter {
            name: "fallback",
            prio: -1,
            plugin_id: "fallback-plugin",
        }));

        let output = registry.parse_dir(Path::new("/tmp/test")).unwrap();
        assert_eq!(output.plugin_id, "fallback-plugin");
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut registry = AdapterRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        registry.register(Box::new(NeverMatchAdapter));
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
    }
}
