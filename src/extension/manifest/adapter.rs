//! `ManifestAdapter` trait and `AdapterRegistry`
//!
//! Provides a trait-based system for parsing plugin directories from
//! multiple platform formats (Claude Code, Codex, Cursor, auto-discover).

use std::path::Path;

use anyhow::{anyhow, Result};

use crate::extension::capability::{CapabilityDeclaration, CapabilitySource};
use crate::extension::manifest::PluginPermission;

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
    /// Permissions required by this plugin
    pub permissions: Vec<PluginPermission>,
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

    /// Human-readable name of this adapter's format (e.g., "`claude_code`", "codex").
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

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AdapterRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self { adapters: vec![] }
    }

    /// Create a registry with default adapters.
    ///
    /// Registered adapters (by descending priority):
    /// - Claude Code TOML (100)
    /// - Claude Code JSON (90)
    /// - Aleph native TOML (85)
    /// - Codex CLI (80)
    /// - Cursor IDE (70)
    /// - Auto-discover (-100)
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(super::cc_plugin_toml::ClaudeCodeTomlAdapter));
        registry.register(Box::new(super::cc_plugin_json::ClaudeCodeJsonAdapter));
        registry.register(Box::new(super::adapters::aleph_toml::AlephTomlAdapter));
        registry.register(Box::new(super::adapters::codex::CodexAdapter));
        registry.register(Box::new(super::adapters::cursor::CursorAdapter));
        registry.register(Box::new(
            super::adapters::auto_discover::AutoDiscoverAdapter,
        ));
        registry
    }

    /// Register a new adapter. Adapters are sorted by descending priority.
    pub fn register(&mut self, adapter: Box<dyn ManifestAdapter>) {
        self.adapters.push(adapter);
        self.adapters
            .sort_by_key(|a| std::cmp::Reverse(a.priority()));
    }

    /// Try each adapter in priority order; return the first successful parse.
    ///
    /// Every adapter's output passes through
    /// [`expand_plugin_variables`](Self::expand_plugin_variables) here rather
    /// than inside each adapter: a new adapter inherits the expansion instead
    /// of having to be told about it, which is the failure mode that left
    /// `${CLAUDE_PLUGIN_ROOT}` unexpanded in every skill body ever parsed.
    pub fn parse_dir(&self, dir: &Path) -> Result<AdapterOutput> {
        for adapter in &self.adapters {
            if adapter.detect(dir) {
                tracing::debug!(
                    adapter = adapter.format_name(),
                    dir = %dir.display(),
                    "Adapter matched"
                );
                let mut output = adapter.parse(dir)?;
                Self::expand_plugin_variables(&mut output, dir);
                return Ok(output);
            }
        }
        Err(anyhow!(
            "No manifest adapter matched directory: {}",
            dir.display()
        ))
    }

    /// Expand `${*_PLUGIN_ROOT}` / `${*_PLUGIN_DATA}` in the prose a plugin
    /// contributes.
    ///
    /// `Run ${CLAUDE_PLUGIN_ROOT}/scripts/x.py` is the most common idiom in a
    /// Claude Code `SKILL.md`. Until 2026-08-19 it reached the model verbatim,
    /// so the model issued a `bash` call against a path containing a literal
    /// `${CLAUDE_PLUGIN_ROOT}`. `.mcp.json` had its own expander and hooks had
    /// a third; skill / command / agent bodies had none.
    ///
    /// Scope: only the fields whose consumer is the model — bodies and hook
    /// commands. Names and ids are identifiers, not paths, and expanding them
    /// would let a manifest smuggle an absolute path into a registry key.
    fn expand_plugin_variables(output: &mut AdapterOutput, plugin_dir: &Path) {
        use crate::extension::capability::CapabilityDeclaration;
        use crate::extension::plugin_vars::PluginVars;

        let vars = PluginVars::new(&output.plugin_id, plugin_dir);
        for cap in &mut output.capabilities {
            match cap {
                CapabilityDeclaration::Skill(skill) => {
                    vars.ensure_data_dir_if_referenced(&skill.content);
                    skill.content = vars.expand(&skill.content);
                }
                CapabilityDeclaration::Agent(agent) => {
                    vars.ensure_data_dir_if_referenced(&agent.content);
                    agent.content = vars.expand(&agent.content);
                }
                CapabilityDeclaration::Hook(hook) => {
                    vars.ensure_data_dir_if_referenced(&hook.handler);
                    hook.handler = vars.expand(&hook.handler);
                    for action in &mut hook.actions {
                        if let crate::extension::types::HookAction::Command { command } = action {
                            *command = vars.expand(command);
                        }
                    }
                }
                // Tool parameters are a JSON Schema, services and MCP servers
                // are handled by their own layers (`mcp_config.rs` expands the
                // runtime `.mcp.json`), and none of them is prose the model
                // reads.
                CapabilityDeclaration::Tool(_)
                | CapabilityDeclaration::Service(_)
                | CapabilityDeclaration::McpServer(_) => {}
            }
        }
    }

    /// Number of registered adapters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    /// Returns true if no adapters are registered.
    #[must_use]
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
                permissions: vec![],
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

    /// `Run ${CLAUDE_PLUGIN_ROOT}/scripts/x.py` is the most common idiom in a
    /// Claude Code `SKILL.md`. Until 2026-08-19 it reached the model as a
    /// literal, so the model issued a `bash` call against a path containing
    /// `${CLAUDE_PLUGIN_ROOT}`. This runs through the real adapter chain,
    /// because the expansion deliberately lives in `parse_dir` rather than in
    /// each adapter.
    #[test]
    fn plugin_variables_are_expanded_in_skill_prose() {
        use crate::extension::capability::CapabilityDeclaration;

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"name": "vars-plugin"}"#,
        )
        .unwrap();
        let skill_dir = dir.path().join("skills/runner");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: runner\ndescription: d\n---\nRun ${CLAUDE_PLUGIN_ROOT}/scripts/x.py now",
        )
        .unwrap();

        let registry = AdapterRegistry::with_defaults();
        let out = registry.parse_dir(dir.path()).unwrap();

        let body = out
            .capabilities
            .iter()
            .find_map(|c| match c {
                CapabilityDeclaration::Skill(s) if s.name == "runner" => Some(s.content.clone()),
                _ => None,
            })
            .expect("skill must be parsed");
        assert!(
            !body.contains("${CLAUDE_PLUGIN_ROOT}"),
            "the model must not be shown an unexpanded variable: {body}"
        );
        // The SKILL.md fixture spelled the tail as `/scripts/x.py` and the
        // substitution is a pure string replace — so the body holds
        // `<plugin_root><literal-slash>scripts/x.py`. On Windows the
        // canonical plugin root uses back-slashes, the literal stays
        // forward, and a join() done in the test would yield a pure
        // back-slash path that the body can never contain. Match the
        // exact spelling by composing the assertion string the same way.
        assert!(
            body.contains(&format!("{}/scripts/x.py", dir.path().to_string_lossy())),
            "expected the plugin root to be substituted: {body}"
        );
    }

    /// Identifiers are not paths. Expanding a name would let a manifest
    /// smuggle an absolute path into a registry key.
    #[test]
    fn plugin_variables_are_not_expanded_in_identifiers() {
        use crate::extension::capability::CapabilityDeclaration;

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"name": "ident-plugin"}"#,
        )
        .unwrap();
        let skill_dir = dir.path().join("skills/s");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: ${CLAUDE_PLUGIN_ROOT}\ndescription: d\n---\nbody",
        )
        .unwrap();

        let registry = AdapterRegistry::with_defaults();
        let out = registry.parse_dir(dir.path()).unwrap();
        assert!(
            out.capabilities.iter().any(|c| matches!(
                c,
                CapabilityDeclaration::Skill(s) if s.name == "${CLAUDE_PLUGIN_ROOT}"
            )),
            "a name must be left alone"
        );
    }
}
