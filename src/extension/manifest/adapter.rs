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
    /// Scope: only the prose whose consumer is the model — skill, command and
    /// agent bodies. Never shell source: a hook's `command` and a body's
    /// `` !`cmd` `` ([`inline_commands`](crate::extension::template::inline_commands))
    /// stay as written, and the variables reach those through the child's
    /// environment (on Windows, substituted at spawn —
    /// `hooks::plugin_shell_line`). The install path is a directory name, and
    /// `r $(touch M) "` spliced into source is parsed as code. Names and ids
    /// are identifiers, not paths, and expanding them would let a manifest
    /// smuggle an absolute path into a registry key.
    fn expand_plugin_variables(output: &mut AdapterOutput, plugin_dir: &Path) {
        use crate::extension::plugin_vars::PluginVars;

        let vars = PluginVars::new(&output.plugin_id, plugin_dir);
        for cap in &mut output.capabilities {
            match cap {
                CapabilityDeclaration::Skill(skill) => {
                    vars.ensure_data_dir_if_referenced(&skill.content);
                    skill.content = expand_outside_inline_commands(&vars, &skill.content);
                }
                CapabilityDeclaration::Agent(agent) => {
                    vars.ensure_data_dir_if_referenced(&agent.content);
                    agent.content = vars.expand(&agent.content);
                }
                // A hook's text is shell source (a `command`) or an
                // identifier (`handler`): neither is expanded. The executor
                // hands the command its path variables when it spawns it
                // (`command_hook_invocation`), and creates the data
                // directory then if the command names it.
                //
                // Tool parameters are a JSON Schema, services and MCP servers
                // are handled by their own layers (an MCP server arrives here
                // already expanded and resolved by `mcp_config.rs`, the one
                // reader — expanding it twice could only differ from what its
                // containment check saw), and none of them is prose the model
                // reads.
                CapabilityDeclaration::Hook(_)
                | CapabilityDeclaration::Tool(_)
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

/// `content` with the plugin variables expanded everywhere except inside its
/// `` !`cmd` `` spans, which stay exactly as written.
///
/// If the expanded paths would change which spans the body has — an install
/// path holding a backtick can close a span early or, with a `!`, open a new
/// one — the body is returned unexpanded and a warning says why: prose that
/// shows `${CLAUDE_PLUGIN_ROOT}` is a nuisance, a path that turns into a
/// command is not.
fn expand_outside_inline_commands(
    vars: &crate::extension::plugin_vars::PluginVars,
    content: &str,
) -> String {
    use crate::extension::template::inline_commands;

    let spans = inline_commands(content);
    let text = |from: usize, to: usize| content.get(from..to).unwrap_or_default();
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for (span, _) in &spans {
        out.push_str(&vars.expand(text(last, span.start)));
        out.push_str(text(span.start, span.end));
        last = span.end;
    }
    out.push_str(&vars.expand(text(last, content.len())));

    let before: Vec<&str> = spans.iter().map(|(_, cmd)| *cmd).collect();
    let after: Vec<&str> = inline_commands(&out)
        .into_iter()
        .map(|(_, cmd)| cmd)
        .collect();
    if before != after {
        tracing::warn!(
            root = %vars.root_dir().display(),
            "plugin path variables left unexpanded in a body: the install path would \
             change its inline commands"
        );
        return content.to_string();
    }
    out
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

    /// Writes a Claude Code plugin at `root` with one `hooks.json` command
    /// hook and one command whose body has a prose reference and an inline
    /// command, each reading `${CLAUDE_PLUGIN_ROOT}/x`. The hook writes what
    /// it read to `out`.
    #[cfg(unix)]
    fn write_root_reading_plugin(root: &Path, out: &Path) {
        std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        std::fs::create_dir_all(root.join("commands")).unwrap();
        std::fs::write(
            root.join(".claude-plugin/plugin.json"),
            r#"{"name": "f1probe"}"#,
        )
        .unwrap();
        std::fs::write(root.join("x"), "from-x").unwrap();
        let hook = format!(r#"cat "${{CLAUDE_PLUGIN_ROOT}}/x" > '{}'"#, out.display());
        std::fs::write(
            root.join("hooks/hooks.json"),
            serde_json::json!({
                "hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": hook}]}]}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("commands/c.md"),
            "---\ndescription: d\n---\nSee ${CLAUDE_PLUGIN_ROOT}/x.\nOut: !`cat \"${CLAUDE_PLUGIN_ROOT}/x\"`\n",
        )
        .unwrap();
    }

    /// Runs a command body's inline commands through the production process
    /// builder, as the plugin that ships it (`InlineSite::plugin`), in `cwd`.
    #[cfg(unix)]
    struct PluginSh<'a> {
        cwd: &'a Path,
        root: &'a Path,
    }

    #[cfg(unix)]
    #[async_trait::async_trait]
    impl crate::extension::template::InlineShell for PluginSh<'_> {
        async fn run(
            &self,
            cmd: &str,
            args: &crate::extension::template::InlineArgs<'_>,
        ) -> Result<String, String> {
            let site = crate::extension::template::InlineSite {
                cwd: self.cwd,
                plugin: Some(("f1probe", self.root)),
                skill_dir: None,
            };
            let out = crate::extension::template::inline_shell_command(cmd, args, &site)
                .output()
                .await
                .map_err(|e| e.to_string())?;
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        }
    }

    /// P4.14 F-1. An install path is a directory name, and a directory name
    /// can hold `$(…)` and `"`. Parsed by the production adapter chain, run by
    /// the production hook executor and inline-command builder: nothing runs,
    /// and both read the intended file. The P4.4d guard
    /// (`a_plugin_root_named_with_a_command_substitution_is_one_word_of_data`)
    /// builds its hook by hand and never crosses `parse_dir`, which is where
    /// the root used to be spliced in; this is its sibling that does.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_plugin_root_is_data_to_its_hooks_and_inline_commands() {
        use crate::extension::capability::CapabilityDeclaration;
        use crate::extension::hooks::{HookContext, HookExecutor};
        use crate::extension::template::{SkillTemplate, TemplateCtx};
        use crate::extension::types::HookEvent;
        use crate::extension::visibility::ScopeKey;

        let base = tempfile::tempdir().unwrap();
        let root = base.path().join(r#"r $(touch M) ""#);
        let out = base.path().join("out");
        write_root_reading_plugin(&root, &out);

        let parsed = AdapterRegistry::with_defaults().parse_dir(&root).unwrap();
        let mut hooks = Vec::new();
        let mut body = None;
        for cap in parsed.capabilities {
            match cap {
                CapabilityDeclaration::Hook(h) => hooks.push(
                    crate::extension::hook_config_from_registration(h, ScopeKey::Global),
                ),
                CapabilityDeclaration::Skill(s) if s.name == "c" => body = Some(s.content),
                _ => {}
            }
        }
        assert_eq!(hooks.len(), 1, "the plugin's one hook");
        let body = body.expect("the command is parsed");

        // The hook, through the executor: its directory is the root.
        HookExecutor::new(hooks)
            .execute_interceptors(
                HookEvent::BeforeToolCall,
                HookContext::new("s").with_tool_name("bash"),
            )
            .await
            .expect("the hook runs");
        assert!(
            !root.join("M").exists(),
            "the root's `$(…)` ran in the hook"
        );
        assert_eq!(
            std::fs::read_to_string(&out).expect("the hook ran"),
            "from-x"
        );

        // The inline command, through the production builder, in `base`.
        let rendered = SkillTemplate::with_base_dir(&body, root.join("commands"))
            .render(
                "",
                &TemplateCtx {
                    shell: Some(&PluginSh {
                        cwd: base.path(),
                        root: &root,
                    }),
                },
            )
            .await
            .unwrap();
        assert!(
            !base.path().join("M").exists() && !root.join("M").exists(),
            "the root's `$(…)` ran in the inline command"
        );
        assert!(rendered.contains("Out: from-x"), "{rendered}");
        // The prose around it is still expanded for the model.
        assert!(
            rendered.contains(&format!("See {}/x.", root.display())),
            "{rendered}"
        );
    }

    /// An install path that would open or close an inline command — a
    /// backtick, here after a `!` — leaves the whole body unexpanded, so the
    /// body's commands are exactly the ones its author wrote.
    #[test]
    fn a_root_that_would_change_a_bodys_inline_commands_is_not_expanded() {
        use crate::extension::capability::CapabilityDeclaration;
        use crate::extension::template::inline_commands;

        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("r!`touch M`");
        std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        std::fs::create_dir_all(root.join("commands")).unwrap();
        std::fs::write(
            root.join(".claude-plugin/plugin.json"),
            r#"{"name": "tickroot"}"#,
        )
        .unwrap();
        let written = "---\ndescription: d\n---\nSee ${CLAUDE_PLUGIN_ROOT}/x.\nNow: !`date`\n";
        std::fs::write(root.join("commands/c.md"), written).unwrap();

        let parsed = AdapterRegistry::with_defaults().parse_dir(&root).unwrap();
        let body = parsed
            .capabilities
            .into_iter()
            .find_map(|c| match c {
                CapabilityDeclaration::Skill(s) if s.name == "c" => Some(s.content),
                _ => None,
            })
            .expect("the command is parsed");
        let commands: Vec<&str> = inline_commands(&body).into_iter().map(|(_, c)| c).collect();
        assert_eq!(commands, vec!["date"], "{body}");
        assert!(body.contains("See ${CLAUDE_PLUGIN_ROOT}/x."), "{body}");
    }
}
