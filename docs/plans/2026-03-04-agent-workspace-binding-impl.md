# Agent Definition + Workspace + Binding Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add configuration-driven agent definitions, materialized workspace directories, and declarative channel binding routing to Aleph.

**Architecture:** New `[agents]` and `[[bindings]]` config sections parsed into types, resolved via `AgentDefinitionResolver` which merges config + Profile + workspace markdown files, then registered into existing `AgentRegistry` and `AgentRouter`.

**Tech Stack:** Rust, serde/toml, existing `RouteBinding`/`resolve_route()` infrastructure, `SoulManifest::from_file()`.

**Design Doc:** `docs/plans/2026-03-04-agent-workspace-binding-design.md`

---

### Task 1: Agent Definition Config Types

**Files:**
- Create: `core/src/config/types/agents_def.rs`
- Modify: `core/src/config/types/mod.rs`

**Step 1: Write the failing test**

At the bottom of `core/src/config/types/agents_def.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agents_config_deserialize_full() {
        let toml_str = r#"
            [defaults]
            model = "claude-opus-4-6"
            workspace_root = "~/.aleph/workspaces"
            skills = ["*"]

            [[list]]
            id = "main"
            default = true
            name = "Aleph"
            workspace = "~/.aleph/workspace"
            profile = "general"

            [[list]]
            id = "coding"
            name = "Code Expert"
            profile = "coding"
            model = "claude-opus-4-6"
            skills = ["git_*", "fs_*"]

            [list.subagents]
            allow = []
        "#;
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.list.len(), 2);
        assert_eq!(config.list[0].id, "main");
        assert!(config.list[0].default);
        assert_eq!(config.list[1].skills.as_ref().unwrap().len(), 2);
        assert!(config.list[1].subagents.as_ref().unwrap().allow.is_empty());
    }

    #[test]
    fn test_agents_config_empty_deserialize() {
        let config: AgentsConfig = toml::from_str("").unwrap();
        assert!(config.list.is_empty());
    }

    #[test]
    fn test_ensure_default_when_empty() {
        let mut config = AgentsConfig::default();
        config.ensure_default();
        assert_eq!(config.list.len(), 1);
        assert_eq!(config.list[0].id, "main");
        assert!(config.list[0].default);
    }

    #[test]
    fn test_ensure_default_noop_when_populated() {
        let mut config = AgentsConfig {
            list: vec![AgentDefinition {
                id: "custom".into(),
                default: true,
                ..AgentDefinition::default()
            }],
            ..Default::default()
        };
        config.ensure_default();
        assert_eq!(config.list.len(), 1);
        assert_eq!(config.list[0].id, "custom");
    }

    #[test]
    fn test_subagent_policy_wildcard() {
        let toml_str = r#"allow = ["*"]"#;
        let policy: SubagentPolicy = toml::from_str(toml_str).unwrap();
        assert_eq!(policy.allow, vec!["*"]);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::types::agents_def -- --nocapture 2>&1 | head -20`
Expected: FAIL — module doesn't exist

**Step 3: Write minimal implementation**

Create `core/src/config/types/agents_def.rs`:

```rust
//! Agent definition configuration types.
//!
//! Defines the `[agents]` section of aleph.toml for declarative agent creation.
//! Each agent entry binds to a Profile, workspace directory, and optional skill/subagent policies.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Agent system top-level configuration (`[agents]` in aleph.toml)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentsConfig {
    /// Global defaults inherited by all agents
    #[serde(default)]
    pub defaults: AgentDefaults,
    /// Agent definitions
    #[serde(default)]
    pub list: Vec<AgentDefinition>,
}

impl AgentsConfig {
    /// Ensure at least one default agent exists.
    /// Called when no `[agents]` section is present in config.
    pub fn ensure_default(&mut self) {
        if self.list.is_empty() {
            self.list.push(AgentDefinition {
                id: "main".into(),
                default: true,
                name: Some("Aleph".into()),
                skills: Some(vec!["*".into()]),
                ..Default::default()
            });
        }
    }
}

/// Global defaults inherited by all agents unless overridden
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentDefaults {
    /// Default AI model
    pub model: Option<String>,
    /// Root directory for auto-layout workspaces (non-default agents)
    pub workspace_root: Option<PathBuf>,
    /// Default skill allowlist
    pub skills: Option<Vec<String>>,
    /// Default DM isolation scope
    pub dm_scope: Option<String>,
    /// Max characters per workspace bootstrap file (default: 20000)
    pub bootstrap_max_chars: Option<usize>,
    /// Max total characters across all workspace files (default: 150000)
    pub bootstrap_total_max_chars: Option<usize>,
}

/// Single agent definition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentDefinition {
    /// Unique identifier (URL-safe slug)
    pub id: String,
    /// Whether this is the default agent for unrouted messages
    #[serde(default)]
    pub default: bool,
    /// Display name
    pub name: Option<String>,
    /// Explicit workspace directory path (auto-derived if omitted)
    pub workspace: Option<PathBuf>,
    /// Profile name to bind (references [profiles.xxx])
    pub profile: Option<String>,
    /// Override model (takes precedence over defaults and profile)
    pub model: Option<String>,
    /// Skill allowlist (glob patterns: "git_*", "*" = all)
    pub skills: Option<Vec<String>>,
    /// Sub-agent delegation policy
    pub subagents: Option<SubagentPolicy>,
}

/// Sub-agent authorization policy
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SubagentPolicy {
    /// Allowed target agent IDs (["*"] = any, [] = none)
    #[serde(default)]
    pub allow: Vec<String>,
}
```

Add to `core/src/config/types/mod.rs`:

```rust
pub mod agents_def;
// ... add to re-exports:
pub use agents_def::*;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib config::types::agents_def -- --nocapture`
Expected: PASS (5 tests)

**Step 5: Commit**

```bash
git add core/src/config/types/agents_def.rs core/src/config/types/mod.rs
git commit -m "config: add AgentsConfig types for declarative agent definitions"
```

---

### Task 2: WorkspaceFileLoader

**Files:**
- Create: `core/src/gateway/workspace_loader.rs`
- Modify: `core/src/gateway/mod.rs`

**Step 1: Write the failing test**

At the bottom of `core/src/gateway/workspace_loader.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# Test Agent\nBe helpful.\n").unwrap();
        fs::write(dir.path().join("MEMORY.md"), "Remember: user likes Rust.\n").unwrap();
        fs::create_dir_all(dir.path().join("memory")).unwrap();
        fs::write(
            dir.path().join("memory/2026-03-03.md"),
            "# 2026-03-03\nDiscussed architecture.\n",
        ).unwrap();
        fs::write(
            dir.path().join("memory/2026-03-04.md"),
            "# 2026-03-04\nImplemented agent config.\n",
        ).unwrap();
        dir
    }

    #[test]
    fn test_load_agents_md() {
        let ws = setup_workspace();
        let mut loader = WorkspaceFileLoader::new();
        let content = loader.load_agents_md(ws.path());
        assert!(content.is_some());
        assert!(content.unwrap().contains("Be helpful"));
    }

    #[test]
    fn test_load_missing_file_returns_none() {
        let ws = setup_workspace();
        let mut loader = WorkspaceFileLoader::new();
        let content = loader.load(ws.path(), "NONEXISTENT.md");
        assert!(content.is_none());
    }

    #[test]
    fn test_load_memory_md_with_truncation() {
        let ws = setup_workspace();
        let mut loader = WorkspaceFileLoader::new();
        let content = loader.load_memory_md(ws.path(), 10);
        assert!(content.is_some());
        let text = content.unwrap();
        assert!(text.len() <= 10);
    }

    #[test]
    fn test_mtime_cache_hit() {
        let ws = setup_workspace();
        let mut loader = WorkspaceFileLoader::new();
        let first = loader.load_agents_md(ws.path());
        let second = loader.load_agents_md(ws.path());
        assert_eq!(first, second);
        // Cache should have one entry
        assert_eq!(loader.cache.len(), 1);
    }

    #[test]
    fn test_load_recent_memory() {
        let ws = setup_workspace();
        let mut loader = WorkspaceFileLoader::new();
        let memories = loader.load_recent_memory(ws.path(), 7);
        assert_eq!(memories.len(), 2);
        // Should be sorted by date descending (most recent first)
        assert_eq!(memories[0].date, "2026-03-04");
        assert_eq!(memories[1].date, "2026-03-03");
    }

    #[test]
    fn test_append_daily_memory() {
        let ws = setup_workspace();
        let loader = WorkspaceFileLoader::new();
        loader
            .append_daily_memory(ws.path(), "2026-03-04", "Added tests.\n")
            .unwrap();
        let content = fs::read_to_string(ws.path().join("memory/2026-03-04.md")).unwrap();
        assert!(content.contains("Implemented agent config"));
        assert!(content.contains("Added tests"));
    }

    #[test]
    fn test_load_soul() {
        let ws = setup_workspace();
        let soul_content = r#"---
relationship: mentor
voice:
  tone: professional
  verbosity: concise
expertise:
  - rust
---

## Identity
I am a Rust expert.

## Directives
- Write idiomatic code
"#;
        fs::write(ws.path().join("SOUL.md"), soul_content).unwrap();
        let mut loader = WorkspaceFileLoader::new();
        let soul = loader.load_soul(ws.path());
        assert!(soul.is_some());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::workspace_loader -- --nocapture 2>&1 | head -20`
Expected: FAIL — module doesn't exist

**Step 3: Write minimal implementation**

Create `core/src/gateway/workspace_loader.rs`:

```rust
//! Workspace file loader with mtime caching.
//!
//! Loads markdown files from agent workspace directories (SOUL.md, AGENTS.md,
//! MEMORY.md, daily memory logs) with filesystem mtime-based cache invalidation.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::thinker::soul::SoulManifest;

/// Cached file content with modification time
struct CachedFile {
    content: String,
    mtime: SystemTime,
}

/// Daily memory log entry
#[derive(Debug, Clone)]
pub struct DailyMemory {
    /// Date string (YYYY-MM-DD)
    pub date: String,
    /// Markdown content
    pub content: String,
}

/// Workspace file loader with mtime-based caching.
///
/// Loads and caches workspace files, invalidating when file mtime changes.
pub struct WorkspaceFileLoader {
    cache: HashMap<PathBuf, CachedFile>,
}

impl WorkspaceFileLoader {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Load a file from workspace, using mtime cache.
    /// Returns None if file doesn't exist.
    pub fn load(&mut self, workspace: &Path, filename: &str) -> Option<String> {
        let path = workspace.join(filename);
        if !path.exists() {
            return None;
        }

        let mtime = fs::metadata(&path).ok()?.modified().ok()?;

        // Check cache
        if let Some(cached) = self.cache.get(&path) {
            if cached.mtime == mtime {
                return Some(cached.content.clone());
            }
        }

        // Read and cache
        let content = fs::read_to_string(&path).ok()?;
        self.cache.insert(
            path,
            CachedFile {
                content: content.clone(),
                mtime,
            },
        );
        Some(content)
    }

    /// Load SOUL.md and parse into SoulManifest
    pub fn load_soul(&mut self, workspace: &Path) -> Option<SoulManifest> {
        let soul_path = workspace.join("SOUL.md");
        if !soul_path.exists() {
            return None;
        }
        SoulManifest::from_file(&soul_path).ok()
    }

    /// Load AGENTS.md content
    pub fn load_agents_md(&mut self, workspace: &Path) -> Option<String> {
        self.load(workspace, "AGENTS.md")
    }

    /// Load MEMORY.md content, truncated to max_chars
    pub fn load_memory_md(&mut self, workspace: &Path, max_chars: usize) -> Option<String> {
        let content = self.load(workspace, "MEMORY.md")?;
        if content.len() <= max_chars {
            Some(content)
        } else {
            // Truncate at char boundary
            Some(content.chars().take(max_chars).collect())
        }
    }

    /// Load recent daily memory logs (most recent first)
    pub fn load_recent_memory(&mut self, workspace: &Path, days: u32) -> Vec<DailyMemory> {
        let memory_dir = workspace.join("memory");
        if !memory_dir.exists() {
            return Vec::new();
        }

        let mut entries: Vec<DailyMemory> = Vec::new();

        if let Ok(read_dir) = fs::read_dir(&memory_dir) {
            for entry in read_dir.flatten() {
                let filename = entry.file_name().to_string_lossy().to_string();
                if let Some(date) = filename.strip_suffix(".md") {
                    // Validate date format (YYYY-MM-DD)
                    if date.len() == 10 && date.chars().nth(4) == Some('-') {
                        if let Ok(content) = fs::read_to_string(entry.path()) {
                            entries.push(DailyMemory {
                                date: date.to_string(),
                                content,
                            });
                        }
                    }
                }
            }
        }

        // Sort by date descending (most recent first)
        entries.sort_by(|a, b| b.date.cmp(&a.date));

        // Limit to N most recent days
        entries.truncate(days as usize);
        entries
    }

    /// Append content to daily memory log
    pub fn append_daily_memory(
        &self,
        workspace: &Path,
        date: &str,
        content: &str,
    ) -> Result<(), std::io::Error> {
        let memory_dir = workspace.join("memory");
        fs::create_dir_all(&memory_dir)?;

        let path = memory_dir.join(format!("{}.md", date));
        let mut existing = fs::read_to_string(&path).unwrap_or_default();
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(content);
        fs::write(&path, existing)
    }
}
```

Add to `core/src/gateway/mod.rs`:

```rust
pub mod workspace_loader;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::workspace_loader -- --nocapture`
Expected: PASS (7 tests). Note: `test_load_soul` may fail if `SoulManifest::from_file` expects specific frontmatter — adjust test content as needed to match existing parser expectations.

**Step 5: Commit**

```bash
git add core/src/gateway/workspace_loader.rs core/src/gateway/mod.rs
git commit -m "gateway: add WorkspaceFileLoader with mtime caching"
```

---

### Task 3: AgentDefinitionResolver

**Files:**
- Create: `core/src/config/agent_resolver.rs`
- Modify: `core/src/config/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::agents_def::*;
    use crate::config::types::profile::ProfileConfig;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn default_profiles() -> HashMap<String, ProfileConfig> {
        let mut profiles = HashMap::new();
        profiles.insert(
            "general".to_string(),
            ProfileConfig {
                model: Some("claude-opus-4-6".into()),
                ..Default::default()
            },
        );
        profiles.insert(
            "coding".to_string(),
            ProfileConfig {
                model: Some("claude-opus-4-6".into()),
                tools: vec!["git_*".into(), "fs_*".into()],
                temperature: Some(0.2),
                ..Default::default()
            },
        );
        profiles
    }

    #[test]
    fn test_resolve_workspace_path_explicit() {
        let resolver = AgentDefinitionResolver::new();
        let agent = AgentDefinition {
            id: "main".into(),
            workspace: Some("/tmp/my-workspace".into()),
            ..Default::default()
        };
        let defaults = AgentDefaults::default();
        let path = resolver.resolve_workspace_path(&agent, &defaults);
        assert_eq!(path.to_str().unwrap(), "/tmp/my-workspace");
    }

    #[test]
    fn test_resolve_workspace_path_auto_layout() {
        let resolver = AgentDefinitionResolver::new();
        let agent = AgentDefinition {
            id: "coding".into(),
            ..Default::default()
        };
        let defaults = AgentDefaults {
            workspace_root: Some("/tmp/workspaces".into()),
            ..Default::default()
        };
        let path = resolver.resolve_workspace_path(&agent, &defaults);
        assert_eq!(path.to_str().unwrap(), "/tmp/workspaces/coding");
    }

    #[test]
    fn test_resolve_all_basic() {
        let tmp = TempDir::new().unwrap();
        let ws_root = tmp.path().to_path_buf();

        let agents_config = AgentsConfig {
            defaults: AgentDefaults {
                model: Some("claude-opus-4-6".into()),
                workspace_root: Some(ws_root.clone()),
                skills: Some(vec!["*".into()]),
                ..Default::default()
            },
            list: vec![
                AgentDefinition {
                    id: "main".into(),
                    default: true,
                    name: Some("Aleph".into()),
                    workspace: Some(ws_root.join("main")),
                    profile: Some("general".into()),
                    ..Default::default()
                },
                AgentDefinition {
                    id: "coding".into(),
                    name: Some("Code Expert".into()),
                    profile: Some("coding".into()),
                    model: Some("gemini-2.5-pro".into()),
                    skills: Some(vec!["git_*".into()]),
                    subagents: Some(SubagentPolicy { allow: vec![] }),
                    ..Default::default()
                },
            ],
        };

        let profiles = default_profiles();
        let mut resolver = AgentDefinitionResolver::new();
        let resolved = resolver.resolve_all(&agents_config, &profiles);

        assert_eq!(resolved.len(), 2);

        // Main agent
        assert_eq!(resolved[0].id, "main");
        assert!(resolved[0].is_default);
        assert_eq!(resolved[0].model, "claude-opus-4-6");
        assert_eq!(resolved[0].skills, vec!["*"]);

        // Coding agent — model overridden
        assert_eq!(resolved[1].id, "coding");
        assert!(!resolved[1].is_default);
        assert_eq!(resolved[1].model, "gemini-2.5-pro"); // Override
        assert_eq!(resolved[1].skills, vec!["git_*"]);

        // Workspace dirs should have been created
        assert!(ws_root.join("main").exists());
        assert!(ws_root.join("coding").exists());
    }

    #[test]
    fn test_resolve_all_empty_creates_default() {
        let tmp = TempDir::new().unwrap();
        let agents_config = AgentsConfig {
            defaults: AgentDefaults {
                workspace_root: Some(tmp.path().to_path_buf()),
                ..Default::default()
            },
            ..Default::default()
        };
        let profiles = HashMap::new();
        let mut resolver = AgentDefinitionResolver::new();
        let resolved = resolver.resolve_all(&agents_config, &profiles);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "main");
        assert!(resolved[0].is_default);
    }

    #[test]
    fn test_workspace_initialization() {
        let tmp = TempDir::new().unwrap();
        let ws_path = tmp.path().join("test-agent");

        initialize_workspace(&ws_path, "Test Agent").unwrap();

        assert!(ws_path.join("AGENTS.md").exists());
        assert!(ws_path.join("memory").exists());

        let agents_content = std::fs::read_to_string(ws_path.join("AGENTS.md")).unwrap();
        assert!(agents_content.contains("Test Agent"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::agent_resolver -- --nocapture 2>&1 | head -20`
Expected: FAIL — module doesn't exist

**Step 3: Write minimal implementation**

Create `core/src/config/agent_resolver.rs`:

```rust
//! Agent definition resolver.
//!
//! Merges AgentsConfig + ProfileConfig + workspace markdown files into
//! fully resolved agent definitions ready for AgentRegistry registration.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::config::types::agents_def::*;
use crate::config::types::profile::ProfileConfig;
use crate::gateway::workspace_loader::WorkspaceFileLoader;
use crate::thinker::soul::SoulManifest;

/// Resolved agent definition (output of AgentDefinitionResolver)
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub workspace_path: PathBuf,
    pub profile: ProfileConfig,
    pub soul: Option<SoulManifest>,
    pub agents_md: Option<String>,
    pub memory_md: Option<String>,
    pub model: String,
    pub skills: Vec<String>,
    pub subagent_policy: SubagentPolicy,
}

const DEFAULT_BOOTSTRAP_MAX_CHARS: usize = 20_000;
const DEFAULT_MODEL: &str = "claude-opus-4-6";

/// Resolves AgentsConfig + Profiles + workspace files into ResolvedAgents
pub struct AgentDefinitionResolver {
    workspace_loader: WorkspaceFileLoader,
}

impl AgentDefinitionResolver {
    pub fn new() -> Self {
        Self {
            workspace_loader: WorkspaceFileLoader::new(),
        }
    }

    /// Resolve all agent definitions into fully resolved agents.
    /// Creates workspace directories if they don't exist.
    pub fn resolve_all(
        &mut self,
        config: &AgentsConfig,
        profiles: &HashMap<String, ProfileConfig>,
    ) -> Vec<ResolvedAgent> {
        let mut effective_config = config.clone();
        effective_config.ensure_default();

        effective_config
            .list
            .iter()
            .map(|agent_def| self.resolve_one(agent_def, &config.defaults, profiles))
            .collect()
    }

    fn resolve_one(
        &mut self,
        agent: &AgentDefinition,
        defaults: &AgentDefaults,
        profiles: &HashMap<String, ProfileConfig>,
    ) -> ResolvedAgent {
        let workspace_path = self.resolve_workspace_path(agent, defaults);

        // Initialize workspace if needed
        let name = agent
            .name
            .clone()
            .unwrap_or_else(|| agent.id.clone());

        if let Err(e) = initialize_workspace(&workspace_path, &name) {
            tracing::warn!(
                agent_id = %agent.id,
                path = %workspace_path.display(),
                error = %e,
                "Failed to initialize workspace"
            );
        }

        // Load profile
        let profile = agent
            .profile
            .as_ref()
            .and_then(|name| profiles.get(name))
            .cloned()
            .unwrap_or_default();

        // Resolve model: agent override > defaults > profile > hardcoded
        let model = agent
            .model
            .clone()
            .or_else(|| defaults.model.clone())
            .or_else(|| profile.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        // Resolve skills: agent override > defaults > all
        let skills = agent
            .skills
            .clone()
            .or_else(|| defaults.skills.clone())
            .unwrap_or_else(|| vec!["*".into()]);

        // Load workspace files
        let soul = self.workspace_loader.load_soul(&workspace_path);
        let max_chars = defaults
            .bootstrap_max_chars
            .unwrap_or(DEFAULT_BOOTSTRAP_MAX_CHARS);
        let agents_md = self.workspace_loader.load_agents_md(&workspace_path);
        let memory_md = self.workspace_loader.load_memory_md(&workspace_path, max_chars);

        let subagent_policy = agent
            .subagents
            .clone()
            .unwrap_or_default();

        ResolvedAgent {
            id: agent.id.clone(),
            name,
            is_default: agent.default,
            workspace_path,
            profile,
            soul,
            agents_md,
            memory_md,
            model,
            skills,
            subagent_policy,
        }
    }

    /// Resolve workspace path for an agent.
    /// Explicit path > auto-layout ({workspace_root}/{agent_id})
    pub fn resolve_workspace_path(
        &self,
        agent: &AgentDefinition,
        defaults: &AgentDefaults,
    ) -> PathBuf {
        if let Some(ref ws) = agent.workspace {
            resolve_user_path(ws)
        } else {
            let root = defaults
                .workspace_root
                .as_ref()
                .map(|p| resolve_user_path(p))
                .unwrap_or_else(default_workspace_root);
            root.join(&agent.id)
        }
    }
}

/// Initialize a workspace directory with default files.
pub fn initialize_workspace(path: &Path, agent_name: &str) -> Result<(), io::Error> {
    fs::create_dir_all(path.join("memory"))?;

    let agents_md_path = path.join("AGENTS.md");
    if !agents_md_path.exists() {
        let content = format!(
            "# {} Operating Instructions\n\n\
             Customize this file to guide agent behavior.\n",
            agent_name
        );
        fs::write(&agents_md_path, content)?;
    }

    Ok(())
}

/// Expand ~ to home directory
fn resolve_user_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/") || s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.join(s.trim_start_matches("~/"));
        }
    }
    path.to_path_buf()
}

/// Default workspace root: ~/.aleph/workspaces
fn default_workspace_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph")
        .join("workspaces")
}
```

Add to `core/src/config/mod.rs`:

```rust
pub mod agent_resolver;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib config::agent_resolver -- --nocapture`
Expected: PASS (5 tests)

**Step 5: Commit**

```bash
git add core/src/config/agent_resolver.rs core/src/config/mod.rs
git commit -m "config: add AgentDefinitionResolver to merge config + workspace + profile"
```

---

### Task 4: Config Struct Integration

**Files:**
- Modify: `core/src/config/structs.rs`
- Modify: `core/src/config/types/mod.rs` (if not already done)

**Step 1: Write the failing test**

Add to `core/src/config/tests/` (or inline at bottom of test module):

```rust
#[test]
fn test_config_with_agents_and_bindings() {
    let toml_str = r#"
        [agents.defaults]
        model = "claude-opus-4-6"

        [[agents.list]]
        id = "main"
        default = true
        name = "Aleph"

        [[agents.list]]
        id = "coding"
        name = "Code Expert"
        profile = "coding"

        [[bindings]]
        agent_id = "coding"
        [bindings.match]
        channel = "slack"
        team_id = "T12345"

        [[bindings]]
        agent_id = "main"
        [bindings.match]
        channel = "*"
        account_id = "*"
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.agents.list.len(), 2);
    assert_eq!(config.bindings.len(), 2);
    assert_eq!(config.bindings[0].agent_id, "coding");
}

#[test]
fn test_config_without_agents_backward_compat() {
    let toml_str = r#"
        [general]
        language = "en"
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(config.agents.list.is_empty()); // Default: empty
    assert!(config.bindings.is_empty());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::tests -- test_config_with_agents 2>&1 | head -20`
Expected: FAIL — `agents` field doesn't exist on Config

**Step 3: Modify Config struct**

In `core/src/config/structs.rs`, add two new fields to the `Config` struct:

```rust
// After the `channels` field, before `presets_override`:

    /// Agent definitions for multi-agent configuration
    /// Defines available agents, their workspaces, profiles, and capabilities
    #[serde(default)]
    pub agents: AgentsConfig,
    /// Channel → Agent routing bindings
    /// Maps channel/peer patterns to specific agents
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<crate::routing::config::RouteBinding>,
```

Add to `Config::default()`:

```rust
    agents: AgentsConfig::default(),
    bindings: Vec::new(),
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib config::tests -- test_config_with_agents --nocapture`
Expected: PASS

**Step 5: Run full config tests**

Run: `cargo test -p alephcore --lib config -- --nocapture 2>&1 | tail -10`
Expected: All existing tests still pass

**Step 6: Commit**

```bash
git add core/src/config/structs.rs
git commit -m "config: add agents and bindings fields to Config struct"
```

---

### Task 5: Binding Integration with AgentRouter

**Files:**
- Modify: `core/src/gateway/router.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_agent_router_from_route_bindings() {
    use crate::routing::config::{MatchRule, RouteBinding};

    let bindings = vec![
        RouteBinding {
            agent_id: "coding".to_string(),
            match_rule: MatchRule {
                channel: Some("slack".to_string()),
                account_id: Some("*".to_string()),
                team_id: Some("T12345".to_string()),
                ..Default::default()
            },
        },
        RouteBinding {
            agent_id: "main".to_string(),
            match_rule: MatchRule {
                channel: Some("telegram".to_string()),
                account_id: Some("*".to_string()),
                ..Default::default()
            },
        },
    ];

    let router = AgentRouter::from_bindings(bindings, "main");
    // Verify agents are registered
    let rt = tokio::runtime::Runtime::new().unwrap();
    let agents = rt.block_on(router.list_agents());
    assert!(agents.contains(&"coding".to_string()));
    assert!(agents.contains(&"main".to_string()));
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL — `from_bindings` method doesn't exist

**Step 3: Add `from_bindings` to AgentRouter**

In `core/src/gateway/router.rs`, add:

```rust
use crate::routing::config::RouteBinding;

impl AgentRouter {
    /// Create router from config-driven RouteBinding list.
    /// Extracts unique agent IDs and converts to internal RoutingBinding format.
    pub fn from_bindings(bindings: Vec<RouteBinding>, default_agent: impl Into<String>) -> Self {
        let default = default_agent.into();

        // Extract unique agent IDs
        let mut agent_ids: Vec<String> = vec![default.clone()];
        for b in &bindings {
            if !agent_ids.contains(&b.agent_id) {
                agent_ids.push(b.agent_id.clone());
            }
        }

        // Convert to internal format: use "channel:*" or "channel:team_id" patterns
        let internal_bindings: Vec<RoutingBinding> = bindings
            .iter()
            .filter_map(|b| {
                let channel = b.match_rule.channel.as_deref()?;
                let pattern = if channel == "*" {
                    "*".to_string()
                } else if let Some(ref team_id) = b.match_rule.team_id {
                    format!("{}:team:{}", channel, team_id)
                } else if let Some(ref guild_id) = b.match_rule.guild_id {
                    format!("{}:guild:{}", channel, guild_id)
                } else {
                    format!("{}:*", channel)
                };
                Some(RoutingBinding {
                    pattern,
                    agent_id: b.agent_id.clone(),
                })
            })
            .collect();

        Self {
            bindings: Arc::new(RwLock::new(internal_bindings)),
            default_agent: default,
            agents: Arc::new(RwLock::new(agent_ids)),
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::router -- test_agent_router_from_route 2>&1 | tail -5`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/router.rs
git commit -m "gateway: add AgentRouter::from_bindings for config-driven routing"
```

---

### Task 6: Startup Wiring

**Files:**
- Modify: Server startup code (the main function or server builder that creates AgentRegistry and AgentRouter)

**Note:** This task connects the pieces. The exact file depends on the server initialization path. Look for where `AgentRegistry::new()` and `AgentRouter::new()` are called.

**Step 1: Find the startup code**

Search for: `AgentRegistry` construction, `AgentRouter` construction, and where `Config::load()` is called in the server startup path.

**Step 2: Add resolver integration**

After `Config::load()`, add:

```rust
// Resolve agent definitions from config
let mut resolver = AgentDefinitionResolver::new();
let resolved_agents = resolver.resolve_all(&config.agents, &config.profiles);

// Find default agent
let default_agent_id = resolved_agents
    .iter()
    .find(|a| a.is_default)
    .map(|a| a.id.clone())
    .unwrap_or_else(|| "main".to_string());

// Register agents
let agent_registry = AgentRegistry::new();
for agent in &resolved_agents {
    let instance = AgentInstance::new(AgentInstanceConfig {
        agent_id: agent.id.clone(),
        workspace: agent.workspace_path.clone(),
        model: agent.model.clone(),
        fallback_models: vec![],
        max_loops: 10,
        system_prompt: agent.agents_md.clone(),
        tool_whitelist: agent.skills.clone(),
        tool_blacklist: vec![],
    });
    agent_registry.register(instance).await;
}

// Build router from bindings
let agent_router = AgentRouter::from_bindings(config.bindings.clone(), &default_agent_id);
// Register all agents in router
for agent in &resolved_agents {
    agent_router.register_agent(&agent.id).await;
}
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add <modified-startup-file>
git commit -m "server: wire AgentDefinitionResolver into startup"
```

---

### Task 7: Integration Test

**Files:**
- Create: `core/src/config/tests/agents_integration.rs` (or add to existing test module)

**Step 1: Write integration test**

```rust
//! Integration test: config → resolver → registry → routing

#[test]
fn test_full_agent_pipeline() {
    use crate::config::agent_resolver::AgentDefinitionResolver;
    use crate::config::types::agents_def::*;
    use crate::config::types::profile::ProfileConfig;
    use crate::config::Config;
    use crate::routing::config::{MatchRule, RouteBinding};
    use crate::routing::resolve::{resolve_route, RouteInput, RoutePeer, RoutePeerKind};
    use crate::routing::config::SessionConfig;
    use std::collections::HashMap;

    // 1. Parse config with agents and bindings
    let toml_str = r#"
        [agents.defaults]
        model = "claude-opus-4-6"

        [[agents.list]]
        id = "main"
        default = true
        name = "Aleph"

        [[agents.list]]
        id = "coding"
        name = "Code Expert"

        [[bindings]]
        agent_id = "coding"
        [bindings.match]
        channel = "slack"
        account_id = "*"
        team_id = "T12345"

        [[bindings]]
        agent_id = "main"
        [bindings.match]
        channel = "telegram"
        account_id = "*"
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();

    // 2. Verify agents parsed
    assert_eq!(config.agents.list.len(), 2);
    assert_eq!(config.bindings.len(), 2);

    // 3. Resolve agents
    let profiles = HashMap::new();
    let mut resolver = AgentDefinitionResolver::new();
    let resolved = resolver.resolve_all(&config.agents, &profiles);
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].model, "claude-opus-4-6");

    // 4. Test route resolution with bindings
    let session_cfg = SessionConfig::default();
    let route = resolve_route(
        &config.bindings,
        &session_cfg,
        "main",
        &RouteInput {
            channel: "slack".to_string(),
            account_id: None,
            peer: None,
            guild_id: None,
            team_id: Some("T12345".to_string()),
        },
    );
    assert_eq!(route.agent_id, "coding");

    // 5. Telegram routes to main
    let route2 = resolve_route(
        &config.bindings,
        &session_cfg,
        "main",
        &RouteInput {
            channel: "telegram".to_string(),
            account_id: None,
            peer: None,
            guild_id: None,
            team_id: None,
        },
    );
    assert_eq!(route2.agent_id, "main");
}
```

**Step 2: Run test**

Run: `cargo test -p alephcore --lib config::tests::agents_integration -- --nocapture`
Expected: PASS

**Step 3: Run all tests to verify no regressions**

Run: `cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: All tests pass (pre-existing failures in `tools::markdown_skill::loader::tests` are expected)

**Step 4: Final commit**

```bash
git add core/src/config/tests/agents_integration.rs
git commit -m "test: add integration test for agent definition pipeline"
```

---

### Summary

| Task | Files | Tests | Estimated Steps |
|------|-------|-------|-----------------|
| 1. Agent Definition Config Types | 1 new, 1 modify | 5 tests | 5 |
| 2. WorkspaceFileLoader | 1 new, 1 modify | 7 tests | 5 |
| 3. AgentDefinitionResolver | 1 new, 1 modify | 5 tests | 5 |
| 4. Config Struct Integration | 1 modify | 2 tests | 6 |
| 5. Binding → AgentRouter | 1 modify | 1 test | 5 |
| 6. Startup Wiring | 1 modify | compile check | 4 |
| 7. Integration Test | 1 new | 1 test | 4 |

**Total: 5 new files, 5 modified files, 21 tests, 7 commits**

### Key Decision: Reuse Existing RouteBinding

The design doc proposed new `AgentBinding` / `BindingMatchRule` types, but the existing `RouteBinding` / `MatchRule` in `core/src/routing/config.rs` already has identical structure. The implementation **reuses** `RouteBinding` directly to avoid duplication. The `[[bindings]]` TOML section deserializes into `Vec<RouteBinding>`, and the existing `resolve_route()` function handles priority matching out of the box.
