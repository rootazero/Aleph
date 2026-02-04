# Skill Requirements & CLI Wrapper Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extend Aleph Skills system with dependency declarations, health checking, and CLI Wrapper execution.

**Architecture:** Add new types to skills module (types.rs, health.rs, cli_wrapper.rs), extend SkillFrontmatter with optional fields, integrate health checking into registry, and create CLI Wrapper executor that leverages existing exec security system.

**Tech Stack:** Rust, serde, thiserror, existing exec module

---

## Task 1: Add Core Types (types.rs)

**Files:**
- Create: `core/src/skills/types.rs`
- Modify: `core/src/skills/mod.rs:31-32`

**Step 1: Write the failing test**

Add to `core/src/skills/types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_manager_serde() {
        let brew: PackageManager = serde_yaml::from_str("brew").unwrap();
        assert_eq!(brew, PackageManager::Brew);

        let apt: PackageManager = serde_yaml::from_str("apt").unwrap();
        assert_eq!(apt, PackageManager::Apt);
    }

    #[test]
    fn test_install_command_parse() {
        let yaml = r#"
manager: brew
package: gh
args: "--cask"
"#;
        let cmd: InstallCommand = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cmd.manager, PackageManager::Brew);
        assert_eq!(cmd.package, "gh");
        assert_eq!(cmd.args, Some("--cask".to_string()));
    }

    #[test]
    fn test_skill_requirements_defaults() {
        let yaml = r#"
binaries:
  - gh
"#;
        let req: SkillRequirements = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(req.binaries, vec!["gh"]);
        assert!(req.platforms.is_none());
        assert!(req.install.is_empty());
    }

    #[test]
    fn test_skill_health_equality() {
        assert_eq!(SkillHealth::Healthy, SkillHealth::Healthy);
        assert_ne!(
            SkillHealth::Healthy,
            SkillHealth::Degraded { missing: vec!["gh".into()] }
        );
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::types --lib`
Expected: FAIL with "can't find crate for `types`"

**Step 3: Write the implementation**

Create `core/src/skills/types.rs`:

```rust
//! Skill types for requirements and health checking.

use serde::{Deserialize, Serialize};

/// Package manager type for installation commands
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageManager {
    /// macOS Homebrew
    Brew,
    /// Debian/Ubuntu apt
    Apt,
    /// Windows winget
    Winget,
    /// Rust cargo (optional extension)
    Cargo,
    /// Python pip (optional extension)
    Pip,
}

/// Single installation command for a package manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallCommand {
    /// Package manager to use
    pub manager: PackageManager,
    /// Package name to install
    pub package: String,
    /// Optional additional arguments (e.g., "--cask" for brew)
    #[serde(default)]
    pub args: Option<String>,
}

/// Skill dependency requirements
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRequirements {
    /// Required binary executables (e.g., ["gh", "git"])
    #[serde(default)]
    pub binaries: Vec<String>,
    /// Supported platforms (e.g., ["macos", "linux"])
    /// None means all platforms supported
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
    /// Installation commands for each package manager
    #[serde(default)]
    pub install: Vec<InstallCommand>,
}

/// Skill health status after dependency check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillHealth {
    /// All dependencies are satisfied
    Healthy,
    /// Some dependencies are missing
    Degraded {
        /// List of missing binary names
        missing: Vec<String>,
    },
    /// Current platform is not supported
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_manager_serde() {
        let brew: PackageManager = serde_yaml::from_str("brew").unwrap();
        assert_eq!(brew, PackageManager::Brew);

        let apt: PackageManager = serde_yaml::from_str("apt").unwrap();
        assert_eq!(apt, PackageManager::Apt);
    }

    #[test]
    fn test_install_command_parse() {
        let yaml = r#"
manager: brew
package: gh
args: "--cask"
"#;
        let cmd: InstallCommand = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cmd.manager, PackageManager::Brew);
        assert_eq!(cmd.package, "gh");
        assert_eq!(cmd.args, Some("--cask".to_string()));
    }

    #[test]
    fn test_skill_requirements_defaults() {
        let yaml = r#"
binaries:
  - gh
"#;
        let req: SkillRequirements = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(req.binaries, vec!["gh"]);
        assert!(req.platforms.is_none());
        assert!(req.install.is_empty());
    }

    #[test]
    fn test_skill_health_equality() {
        assert_eq!(SkillHealth::Healthy, SkillHealth::Healthy);
        assert_ne!(
            SkillHealth::Healthy,
            SkillHealth::Degraded { missing: vec!["gh".into()] }
        );
    }
}
```

**Step 4: Export the module**

Modify `core/src/skills/mod.rs` line 31, add after `pub mod registry;`:

```rust
pub mod types;
```

And add re-exports after line 175 (`pub use registry::SkillsRegistry;`):

```rust
pub use types::{InstallCommand, PackageManager, SkillHealth, SkillRequirements};
```

**Step 5: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::types --lib`
Expected: PASS (4 tests)

**Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements
git add core/src/skills/types.rs core/src/skills/mod.rs
git commit -m "feat(skills): add types for requirements and health checking"
```

---

## Task 2: Extend SkillFrontmatter

**Files:**
- Modify: `core/src/skills/mod.rs:37-54`

**Step 1: Write the failing test**

Add to `core/src/skills/mod.rs` tests section:

```rust
#[test]
fn test_parse_skill_with_requirements() {
    let content = r#"---
name: github
description: GitHub CLI operations
emoji: "🐙"
category: developer
cli-wrapper: true
requirements:
  binaries:
    - gh
  platforms:
    - macos
    - linux
  install:
    - manager: brew
      package: gh
---

# GitHub Skill
"#;
    let skill = Skill::parse("github", content).unwrap();

    assert_eq!(skill.frontmatter.emoji, Some("🐙".to_string()));
    assert_eq!(skill.frontmatter.category, Some("developer".to_string()));
    assert!(skill.frontmatter.cli_wrapper);

    let req = skill.frontmatter.requirements.unwrap();
    assert_eq!(req.binaries, vec!["gh"]);
    assert_eq!(req.platforms, Some(vec!["macos".to_string(), "linux".to_string()]));
    assert_eq!(req.install.len(), 1);
    assert_eq!(req.install[0].package, "gh");
}

#[test]
fn test_parse_skill_without_requirements_backwards_compat() {
    // Existing skills without new fields should still parse
    let skill = Skill::parse("refine-text", VALID_SKILL_MD).unwrap();

    assert!(skill.frontmatter.emoji.is_none());
    assert!(skill.frontmatter.category.is_none());
    assert!(!skill.frontmatter.cli_wrapper);
    assert!(skill.frontmatter.requirements.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::tests::test_parse_skill_with_requirements --lib`
Expected: FAIL with "unknown field `emoji`"

**Step 3: Update SkillFrontmatter**

Modify `core/src/skills/mod.rs` SkillFrontmatter struct (lines 37-54):

```rust
use crate::skills::types::SkillRequirements;

/// SKILL.md frontmatter structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Skill name (used as identifier)
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// Allowed tools for this skill (reserved for MCP integration)
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Vec<String>,

    /// Trigger keywords for natural language command detection
    #[serde(default)]
    pub triggers: Vec<String>,

    // === New fields (all optional for backwards compatibility) ===

    /// UI icon emoji (e.g., "🐙")
    #[serde(default)]
    pub emoji: Option<String>,

    /// Category tag (e.g., "developer", "media", "productivity")
    #[serde(default)]
    pub category: Option<String>,

    /// Whether this skill is a CLI wrapper
    #[serde(default, rename = "cli-wrapper")]
    pub cli_wrapper: bool,

    /// Dependency requirements
    #[serde(default)]
    pub requirements: Option<SkillRequirements>,
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::tests --lib`
Expected: PASS (all existing + 2 new tests)

**Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements
git add core/src/skills/mod.rs
git commit -m "feat(skills): extend SkillFrontmatter with requirements and metadata"
```

---

## Task 3: Implement HealthChecker

**Files:**
- Create: `core/src/skills/health.rs`
- Modify: `core/src/skills/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{Skill, SkillRequirements};

    fn mock_skill_with_requirements(binaries: Vec<&str>, platforms: Option<Vec<&str>>) -> Skill {
        use crate::skills::SkillFrontmatter;

        Skill {
            id: "test-skill".to_string(),
            frontmatter: SkillFrontmatter {
                name: "test".to_string(),
                description: "test".to_string(),
                allowed_tools: vec![],
                triggers: vec![],
                emoji: None,
                category: None,
                cli_wrapper: false,
                requirements: Some(SkillRequirements {
                    binaries: binaries.into_iter().map(String::from).collect(),
                    platforms: platforms.map(|p| p.into_iter().map(String::from).collect()),
                    install: vec![],
                }),
            },
            instructions: String::new(),
        }
    }

    #[test]
    fn test_check_binary_exists() {
        // 'ls' should exist on all Unix systems
        assert!(HealthChecker::check_binary("ls"));
        // This should not exist
        assert!(!HealthChecker::check_binary("nonexistent_binary_12345"));
    }

    #[test]
    fn test_check_platform() {
        // No platforms specified = all supported
        assert!(HealthChecker::check_platform(&None));

        // Current platform should be supported
        let current = std::env::consts::OS;
        assert!(HealthChecker::check_platform(&Some(vec![current.to_string()])));

        // Nonexistent platform
        assert!(!HealthChecker::check_platform(&Some(vec!["nonexistent_os".to_string()])));
    }

    #[test]
    fn test_check_skill_healthy() {
        let skill = mock_skill_with_requirements(vec!["ls"], None);
        assert_eq!(HealthChecker::check_skill(&skill), SkillHealth::Healthy);
    }

    #[test]
    fn test_check_skill_degraded() {
        let skill = mock_skill_with_requirements(vec!["nonexistent_binary_12345"], None);
        match HealthChecker::check_skill(&skill) {
            SkillHealth::Degraded { missing } => {
                assert_eq!(missing, vec!["nonexistent_binary_12345"]);
            }
            _ => panic!("Expected Degraded"),
        }
    }

    #[test]
    fn test_check_skill_no_requirements() {
        use crate::skills::SkillFrontmatter;

        let skill = Skill {
            id: "simple".to_string(),
            frontmatter: SkillFrontmatter {
                name: "simple".to_string(),
                description: "simple".to_string(),
                allowed_tools: vec![],
                triggers: vec![],
                emoji: None,
                category: None,
                cli_wrapper: false,
                requirements: None,
            },
            instructions: String::new(),
        };
        assert_eq!(HealthChecker::check_skill(&skill), SkillHealth::Healthy);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::health --lib`
Expected: FAIL with "can't find crate for `health`"

**Step 3: Write the implementation**

Create `core/src/skills/health.rs`:

```rust
//! Health checking for skill dependencies.

use crate::skills::{Skill, SkillHealth, SkillRequirements};
use std::process::Command;

/// Health checker for skill dependencies
pub struct HealthChecker;

impl HealthChecker {
    /// Check if a binary exists in PATH
    pub fn check_binary(name: &str) -> bool {
        #[cfg(unix)]
        {
            Command::new("which")
                .arg(name)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        #[cfg(windows)]
        {
            Command::new("where")
                .arg(name)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
    }

    /// Check if current platform is in the supported list
    pub fn check_platform(platforms: &Option<Vec<String>>) -> bool {
        let current = std::env::consts::OS;
        platforms
            .as_ref()
            .map(|p| p.iter().any(|s| s == current))
            .unwrap_or(true) // No platforms specified = all supported
    }

    /// Check skill health status
    pub fn check_skill(skill: &Skill) -> SkillHealth {
        let Some(req) = &skill.frontmatter.requirements else {
            return SkillHealth::Healthy; // No requirements = healthy
        };

        // Platform check
        if !Self::check_platform(&req.platforms) {
            return SkillHealth::Unsupported;
        }

        // Binary check
        let missing: Vec<String> = req
            .binaries
            .iter()
            .filter(|bin| !Self::check_binary(bin))
            .cloned()
            .collect();

        if missing.is_empty() {
            SkillHealth::Healthy
        } else {
            SkillHealth::Degraded { missing }
        }
    }

    /// Batch check multiple skills
    pub fn check_skills(skills: &[Skill]) -> Vec<(String, SkillHealth)> {
        skills
            .iter()
            .map(|s| (s.id.clone(), Self::check_skill(s)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{Skill, SkillFrontmatter, SkillRequirements};

    fn mock_skill_with_requirements(binaries: Vec<&str>, platforms: Option<Vec<&str>>) -> Skill {
        Skill {
            id: "test-skill".to_string(),
            frontmatter: SkillFrontmatter {
                name: "test".to_string(),
                description: "test".to_string(),
                allowed_tools: vec![],
                triggers: vec![],
                emoji: None,
                category: None,
                cli_wrapper: false,
                requirements: Some(SkillRequirements {
                    binaries: binaries.into_iter().map(String::from).collect(),
                    platforms: platforms.map(|p| p.into_iter().map(String::from).collect()),
                    install: vec![],
                }),
            },
            instructions: String::new(),
        }
    }

    #[test]
    fn test_check_binary_exists() {
        assert!(HealthChecker::check_binary("ls"));
        assert!(!HealthChecker::check_binary("nonexistent_binary_12345"));
    }

    #[test]
    fn test_check_platform() {
        assert!(HealthChecker::check_platform(&None));

        let current = std::env::consts::OS;
        assert!(HealthChecker::check_platform(&Some(vec![current.to_string()])));
        assert!(!HealthChecker::check_platform(&Some(vec!["nonexistent_os".to_string()])));
    }

    #[test]
    fn test_check_skill_healthy() {
        let skill = mock_skill_with_requirements(vec!["ls"], None);
        assert_eq!(HealthChecker::check_skill(&skill), SkillHealth::Healthy);
    }

    #[test]
    fn test_check_skill_degraded() {
        let skill = mock_skill_with_requirements(vec!["nonexistent_binary_12345"], None);
        match HealthChecker::check_skill(&skill) {
            SkillHealth::Degraded { missing } => {
                assert_eq!(missing, vec!["nonexistent_binary_12345"]);
            }
            _ => panic!("Expected Degraded"),
        }
    }

    #[test]
    fn test_check_skill_no_requirements() {
        let skill = Skill {
            id: "simple".to_string(),
            frontmatter: SkillFrontmatter {
                name: "simple".to_string(),
                description: "simple".to_string(),
                allowed_tools: vec![],
                triggers: vec![],
                emoji: None,
                category: None,
                cli_wrapper: false,
                requirements: None,
            },
            instructions: String::new(),
        };
        assert_eq!(HealthChecker::check_skill(&skill), SkillHealth::Healthy);
    }

    #[test]
    fn test_check_skill_unsupported_platform() {
        let skill = mock_skill_with_requirements(vec!["ls"], Some(vec!["nonexistent_os"]));
        assert_eq!(HealthChecker::check_skill(&skill), SkillHealth::Unsupported);
    }
}
```

**Step 4: Export the module**

Modify `core/src/skills/mod.rs`, add after `pub mod types;`:

```rust
pub mod health;
```

And add re-export:

```rust
pub use health::HealthChecker;
```

**Step 5: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::health --lib`
Expected: PASS (6 tests)

**Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements
git add core/src/skills/health.rs core/src/skills/mod.rs
git commit -m "feat(skills): implement HealthChecker for dependency validation"
```

---

## Task 4: Add Install Suggestion to Installer

**Files:**
- Modify: `core/src/skills/installer.rs`

**Step 1: Write the failing test**

Add to `core/src/skills/installer.rs` tests:

```rust
#[test]
fn test_suggest_install_command_brew() {
    use crate::skills::types::{InstallCommand, PackageManager, SkillRequirements};

    let req = SkillRequirements {
        binaries: vec!["gh".to_string()],
        platforms: None,
        install: vec![InstallCommand {
            manager: PackageManager::Brew,
            package: "gh".to_string(),
            args: None,
        }],
    };

    #[cfg(target_os = "macos")]
    {
        let cmd = SkillsInstaller::suggest_install_command(&req);
        assert_eq!(cmd, Some("brew install gh".to_string()));
    }
}

#[test]
fn test_suggest_install_command_with_args() {
    use crate::skills::types::{InstallCommand, PackageManager, SkillRequirements};

    let req = SkillRequirements {
        binaries: vec!["docker".to_string()],
        platforms: None,
        install: vec![InstallCommand {
            manager: PackageManager::Brew,
            package: "docker".to_string(),
            args: Some("--cask".to_string()),
        }],
    };

    #[cfg(target_os = "macos")]
    {
        let cmd = SkillsInstaller::suggest_install_command(&req);
        assert_eq!(cmd, Some("brew install --cask docker".to_string()));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::installer::tests::test_suggest_install --lib`
Expected: FAIL with "no function or associated item named `suggest_install_command`"

**Step 3: Add the implementation**

Add to `core/src/skills/installer.rs` after the `skills_dir` method:

```rust
use crate::skills::types::{PackageManager, SkillRequirements};

impl SkillsInstaller {
    // ... existing methods ...

    /// Suggest an install command for the current platform
    ///
    /// Returns the appropriate install command based on the current OS.
    pub fn suggest_install_command(req: &SkillRequirements) -> Option<String> {
        let os = std::env::consts::OS;

        req.install
            .iter()
            .find(|cmd| match (&cmd.manager, os) {
                (PackageManager::Brew, "macos") => true,
                (PackageManager::Apt, "linux") => true,
                (PackageManager::Winget, "windows") => true,
                _ => false,
            })
            .map(|cmd| {
                let base = match cmd.manager {
                    PackageManager::Brew => "brew install",
                    PackageManager::Apt => "sudo apt install -y",
                    PackageManager::Winget => "winget install",
                    PackageManager::Cargo => "cargo install",
                    PackageManager::Pip => "pip install",
                };
                match &cmd.args {
                    Some(args) => format!("{} {} {}", base, args, cmd.package),
                    None => format!("{} {}", base, cmd.package),
                }
            })
    }

    /// Generate install commands for all missing binaries
    pub fn suggest_install_plan(req: &SkillRequirements, missing: &[String]) -> Vec<String> {
        missing
            .iter()
            .filter_map(|bin| {
                // Find install command for this binary
                req.install
                    .iter()
                    .find(|cmd| cmd.package == *bin)
                    .and_then(|_| Self::suggest_install_command(req))
            })
            .collect()
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::installer --lib`
Expected: PASS

**Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements
git add core/src/skills/installer.rs
git commit -m "feat(skills): add install suggestion methods to SkillsInstaller"
```

---

## Task 5: Add Health Methods to Registry

**Files:**
- Modify: `core/src/skills/registry.rs`

**Step 1: Write the failing test**

Add to `core/src/skills/registry.rs` tests:

```rust
#[test]
fn test_load_all_with_health() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();

    // Create a skill with requirements for 'ls' (should exist)
    let skill_dir = skills_dir.join("test-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: test-skill
description: Test skill
requirements:
  binaries:
    - ls
---
Instructions
"#,
    )
    .unwrap();

    let registry = SkillsRegistry::new(skills_dir);
    registry.load_all().unwrap();

    let with_health = registry.load_all_with_health();
    assert_eq!(with_health.len(), 1);
    assert_eq!(with_health[0].health, SkillHealth::Healthy);
}

#[test]
fn test_list_degraded_skills() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();

    // Create a skill with nonexistent binary
    let skill_dir = skills_dir.join("broken-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: broken-skill
description: Broken skill
requirements:
  binaries:
    - nonexistent_binary_12345
---
Instructions
"#,
    )
    .unwrap();

    let registry = SkillsRegistry::new(skills_dir);
    registry.load_all().unwrap();

    let degraded = registry.list_degraded_skills();
    assert_eq!(degraded.len(), 1);
    assert_eq!(degraded[0].0.id, "broken-skill");
    assert_eq!(degraded[0].1, vec!["nonexistent_binary_12345"]);
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::registry::tests::test_load_all_with_health --lib`
Expected: FAIL with "no method named `load_all_with_health`"

**Step 3: Add the implementation**

Add to `core/src/skills/registry.rs`:

```rust
use crate::skills::health::HealthChecker;
use crate::skills::types::SkillHealth;

/// Skill with its health status
#[derive(Debug, Clone)]
pub struct SkillWithHealth {
    pub skill: Skill,
    pub health: SkillHealth,
}

impl SkillsRegistry {
    // ... existing methods ...

    /// Load all skills and check their health
    pub fn load_all_with_health(&self) -> Vec<SkillWithHealth> {
        let skills = self.list_skills();
        skills
            .into_iter()
            .map(|skill| {
                let health = HealthChecker::check_skill(&skill);
                SkillWithHealth { skill, health }
            })
            .collect()
    }

    /// List only healthy skills
    pub fn list_healthy_skills(&self) -> Vec<Skill> {
        self.load_all_with_health()
            .into_iter()
            .filter(|s| s.health == SkillHealth::Healthy)
            .map(|s| s.skill)
            .collect()
    }

    /// List degraded skills with their missing dependencies
    pub fn list_degraded_skills(&self) -> Vec<(Skill, Vec<String>)> {
        self.load_all_with_health()
            .into_iter()
            .filter_map(|s| match s.health {
                SkillHealth::Degraded { missing } => Some((s.skill, missing)),
                _ => None,
            })
            .collect()
    }
}
```

**Step 4: Export SkillWithHealth**

Add to re-exports in `core/src/skills/mod.rs`:

```rust
pub use registry::SkillWithHealth;
```

**Step 5: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::registry --lib`
Expected: PASS

**Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements
git add core/src/skills/registry.rs core/src/skills/mod.rs
git commit -m "feat(skills): add health checking methods to SkillsRegistry"
```

---

## Task 6: Implement CLI Wrapper Executor

**Files:**
- Create: `core/src/skills/cli_wrapper.rs`
- Modify: `core/src/skills/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{Skill, SkillFrontmatter, SkillRequirements};

    fn mock_cli_wrapper_skill() -> Skill {
        Skill {
            id: "github".to_string(),
            frontmatter: SkillFrontmatter {
                name: "github".to_string(),
                description: "GitHub CLI".to_string(),
                allowed_tools: vec![],
                triggers: vec![],
                emoji: Some("🐙".to_string()),
                category: Some("developer".to_string()),
                cli_wrapper: true,
                requirements: Some(SkillRequirements {
                    binaries: vec!["gh".to_string()],
                    platforms: None,
                    install: vec![],
                }),
            },
            instructions: String::new(),
        }
    }

    fn mock_non_cli_skill() -> Skill {
        Skill {
            id: "simple".to_string(),
            frontmatter: SkillFrontmatter {
                name: "simple".to_string(),
                description: "Simple".to_string(),
                allowed_tools: vec![],
                triggers: vec![],
                emoji: None,
                category: None,
                cli_wrapper: false,
                requirements: None,
            },
            instructions: String::new(),
        }
    }

    #[test]
    fn test_validate_command_allowed() {
        let skill = mock_cli_wrapper_skill();
        let result = CliWrapperValidator::validate_command(&skill, "gh pr list");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_command_unauthorized_binary() {
        let skill = mock_cli_wrapper_skill();
        let result = CliWrapperValidator::validate_command(&skill, "rm -rf /");
        assert!(matches!(result, Err(CliWrapperError::UnauthorizedBinary { .. })));
    }

    #[test]
    fn test_validate_command_not_cli_wrapper() {
        let skill = mock_non_cli_skill();
        let result = CliWrapperValidator::validate_command(&skill, "echo hello");
        assert!(matches!(result, Err(CliWrapperError::NotCliWrapper)));
    }

    #[test]
    fn test_validate_command_empty() {
        let skill = mock_cli_wrapper_skill();
        let result = CliWrapperValidator::validate_command(&skill, "");
        assert!(matches!(result, Err(CliWrapperError::EmptyCommand)));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::cli_wrapper --lib`
Expected: FAIL

**Step 3: Write the implementation**

Create `core/src/skills/cli_wrapper.rs`:

```rust
//! CLI Wrapper skill execution validation.
//!
//! CLI Wrapper skills can execute shell commands, but only for binaries
//! declared in their requirements. All commands go through the exec
//! approval system.

use crate::skills::Skill;
use thiserror::Error;

/// Errors from CLI Wrapper validation
#[derive(Debug, Error)]
pub enum CliWrapperError {
    #[error("Skill is not a CLI wrapper")]
    NotCliWrapper,

    #[error("Empty command")]
    EmptyCommand,

    #[error("Unauthorized binary '{attempted}', allowed: {allowed:?}")]
    UnauthorizedBinary {
        attempted: String,
        allowed: Vec<String>,
    },

    #[error("No requirements defined for CLI wrapper skill")]
    NoRequirements,
}

/// Validator for CLI Wrapper commands
pub struct CliWrapperValidator;

impl CliWrapperValidator {
    /// Validate that a command is allowed by the skill's requirements
    ///
    /// Checks:
    /// 1. Skill has cli_wrapper = true
    /// 2. Skill has requirements with binaries defined
    /// 3. Command's binary is in the allowed binaries list
    pub fn validate_command(skill: &Skill, command: &str) -> Result<(), CliWrapperError> {
        // Check if skill is a CLI wrapper
        if !skill.frontmatter.cli_wrapper {
            return Err(CliWrapperError::NotCliWrapper);
        }

        // Check if command is empty
        let command = command.trim();
        if command.is_empty() {
            return Err(CliWrapperError::EmptyCommand);
        }

        // Get requirements
        let req = skill
            .frontmatter
            .requirements
            .as_ref()
            .ok_or(CliWrapperError::NoRequirements)?;

        // Extract binary name (first token)
        let binary = command
            .split_whitespace()
            .next()
            .ok_or(CliWrapperError::EmptyCommand)?;

        // Check if binary is allowed
        if !req.binaries.iter().any(|b| b == binary) {
            return Err(CliWrapperError::UnauthorizedBinary {
                attempted: binary.to_string(),
                allowed: req.binaries.clone(),
            });
        }

        Ok(())
    }

    /// Check if a skill is a CLI wrapper
    pub fn is_cli_wrapper(skill: &Skill) -> bool {
        skill.frontmatter.cli_wrapper
    }

    /// Get allowed binaries for a CLI wrapper skill
    pub fn allowed_binaries(skill: &Skill) -> Option<&[String]> {
        if !skill.frontmatter.cli_wrapper {
            return None;
        }
        skill
            .frontmatter
            .requirements
            .as_ref()
            .map(|r| r.binaries.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillFrontmatter, SkillRequirements};

    fn mock_cli_wrapper_skill() -> Skill {
        Skill {
            id: "github".to_string(),
            frontmatter: SkillFrontmatter {
                name: "github".to_string(),
                description: "GitHub CLI".to_string(),
                allowed_tools: vec![],
                triggers: vec![],
                emoji: Some("🐙".to_string()),
                category: Some("developer".to_string()),
                cli_wrapper: true,
                requirements: Some(SkillRequirements {
                    binaries: vec!["gh".to_string()],
                    platforms: None,
                    install: vec![],
                }),
            },
            instructions: String::new(),
        }
    }

    fn mock_non_cli_skill() -> Skill {
        Skill {
            id: "simple".to_string(),
            frontmatter: SkillFrontmatter {
                name: "simple".to_string(),
                description: "Simple".to_string(),
                allowed_tools: vec![],
                triggers: vec![],
                emoji: None,
                category: None,
                cli_wrapper: false,
                requirements: None,
            },
            instructions: String::new(),
        }
    }

    #[test]
    fn test_validate_command_allowed() {
        let skill = mock_cli_wrapper_skill();
        let result = CliWrapperValidator::validate_command(&skill, "gh pr list");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_command_unauthorized_binary() {
        let skill = mock_cli_wrapper_skill();
        let result = CliWrapperValidator::validate_command(&skill, "rm -rf /");
        assert!(matches!(
            result,
            Err(CliWrapperError::UnauthorizedBinary { .. })
        ));
    }

    #[test]
    fn test_validate_command_not_cli_wrapper() {
        let skill = mock_non_cli_skill();
        let result = CliWrapperValidator::validate_command(&skill, "echo hello");
        assert!(matches!(result, Err(CliWrapperError::NotCliWrapper)));
    }

    #[test]
    fn test_validate_command_empty() {
        let skill = mock_cli_wrapper_skill();
        let result = CliWrapperValidator::validate_command(&skill, "");
        assert!(matches!(result, Err(CliWrapperError::EmptyCommand)));
    }

    #[test]
    fn test_is_cli_wrapper() {
        assert!(CliWrapperValidator::is_cli_wrapper(&mock_cli_wrapper_skill()));
        assert!(!CliWrapperValidator::is_cli_wrapper(&mock_non_cli_skill()));
    }

    #[test]
    fn test_allowed_binaries() {
        let skill = mock_cli_wrapper_skill();
        let binaries = CliWrapperValidator::allowed_binaries(&skill);
        assert_eq!(binaries, Some(vec!["gh".to_string()].as_slice()));

        let non_cli = mock_non_cli_skill();
        assert!(CliWrapperValidator::allowed_binaries(&non_cli).is_none());
    }
}
```

**Step 4: Export the module**

Add to `core/src/skills/mod.rs`:

```rust
pub mod cli_wrapper;
```

And re-exports:

```rust
pub use cli_wrapper::{CliWrapperError, CliWrapperValidator};
```

**Step 5: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::cli_wrapper --lib`
Expected: PASS (6 tests)

**Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements
git add core/src/skills/cli_wrapper.rs core/src/skills/mod.rs
git commit -m "feat(skills): implement CLI Wrapper validator"
```

---

## Task 7: Extend ExecContext for Skill Origin

**Files:**
- Modify: `core/src/exec/decision.rs`

**Step 1: Write the failing test**

Add to `core/src/exec/decision.rs` tests:

```rust
#[test]
fn test_context_with_skill_info() {
    let ctx = ExecContext {
        agent_id: "main".into(),
        session_key: "agent:main:main".into(),
        cwd: None,
        command: "gh pr list".into(),
        from_skill: true,
        skill_id: Some("github".into()),
        skill_name: Some("GitHub CLI".into()),
    };

    assert!(ctx.from_skill);
    assert_eq!(ctx.skill_id, Some("github".into()));
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test exec::decision::tests::test_context_with_skill_info --lib`
Expected: FAIL with "no field `skill_id`"

**Step 3: Update ExecContext**

Modify `core/src/exec/decision.rs` ExecContext struct:

```rust
/// Context for execution decision
#[derive(Debug, Clone)]
pub struct ExecContext {
    pub agent_id: String,
    pub session_key: String,
    pub cwd: Option<String>,
    pub command: String,
    /// Whether this command is from a skill
    pub from_skill: bool,
    /// Skill ID if from a CLI Wrapper skill
    pub skill_id: Option<String>,
    /// Skill name if from a CLI Wrapper skill
    pub skill_name: Option<String>,
}
```

Update the `context` helper function in tests:

```rust
fn context(command: &str) -> ExecContext {
    ExecContext {
        agent_id: "main".into(),
        session_key: "agent:main:main".into(),
        cwd: None,
        command: command.into(),
        from_skill: false,
        skill_id: None,
        skill_name: None,
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test exec::decision --lib`
Expected: PASS

**Step 5: Update any other code that creates ExecContext**

Search for `ExecContext {` in the codebase and update all usages to include the new fields.

**Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements
git add core/src/exec/decision.rs
git commit -m "feat(exec): extend ExecContext with skill origin info"
```

---

## Task 8: Add skill_allowlist to Config

**Files:**
- Modify: `core/src/exec/config.rs`

**Step 1: Write the failing test**

Add to config tests:

```rust
#[test]
fn test_exec_config_with_skill_allowlist() {
    let yaml = r#"
security: allowlist
skill_allowlist:
  - github
  - ffmpeg
"#;
    let config: AgentExecConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(
        config.skill_allowlist,
        Some(vec!["github".to_string(), "ffmpeg".to_string()])
    );
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL with "unknown field `skill_allowlist`"

**Step 3: Add skill_allowlist field**

Add to `AgentExecConfig` in `core/src/exec/config.rs`:

```rust
/// Skills that are pre-approved for CLI execution
#[serde(default)]
pub skill_allowlist: Option<Vec<String>>,
```

Also add to `ResolvedExecConfig`:

```rust
/// Skills that are pre-approved for CLI execution
pub skill_allowlist: Vec<String>,
```

Update the resolution logic to include:

```rust
skill_allowlist: agent.skill_allowlist.clone().unwrap_or_default(),
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test exec::config --lib`
Expected: PASS

**Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements
git add core/src/exec/config.rs
git commit -m "feat(exec): add skill_allowlist config option"
```

---

## Task 9: Integrate Skill Allowlist in Decision Logic

**Files:**
- Modify: `core/src/exec/decision.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_skill_allowlist_auto_approve() {
    let config = ResolvedExecConfig {
        security: ExecSecurity::Allowlist,
        ask: ExecAsk::OnMiss,
        ask_fallback: ExecSecurity::Deny,
        auto_allow_skills: false,
        allowlist: vec![],
        skill_allowlist: vec!["github".to_string()],
    };
    let analysis = analyze_shell_command("gh pr list", None, None);
    let ctx = ExecContext {
        agent_id: "main".into(),
        session_key: "agent:main:main".into(),
        cwd: None,
        command: "gh pr list".into(),
        from_skill: true,
        skill_id: Some("github".into()),
        skill_name: Some("GitHub CLI".into()),
    };

    let decision = decide_exec_approval(&config, &analysis, &ctx);
    assert!(matches!(decision, ApprovalDecision::Allow));
}

#[test]
fn test_skill_not_in_allowlist_needs_approval() {
    let config = ResolvedExecConfig {
        security: ExecSecurity::Allowlist,
        ask: ExecAsk::OnMiss,
        ask_fallback: ExecSecurity::Deny,
        auto_allow_skills: false,
        allowlist: vec![],
        skill_allowlist: vec!["other-skill".to_string()],
    };
    let analysis = analyze_shell_command("gh pr list", None, None);
    let ctx = ExecContext {
        agent_id: "main".into(),
        session_key: "agent:main:main".into(),
        cwd: None,
        command: "gh pr list".into(),
        from_skill: true,
        skill_id: Some("github".into()),
        skill_name: Some("GitHub CLI".into()),
    };

    let decision = decide_exec_approval(&config, &analysis, &ctx);
    assert!(matches!(decision, ApprovalDecision::NeedApproval { .. }));
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL (skill_allowlist not checked)

**Step 3: Update decision logic**

In `decide_exec_approval`, after the `auto_allow_skills` check, add:

```rust
// 3.5. Check skill allowlist
if context.from_skill {
    if let Some(skill_id) = &context.skill_id {
        if config.skill_allowlist.contains(skill_id) {
            return ApprovalDecision::Allow;
        }
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test exec::decision --lib`
Expected: PASS

**Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements
git add core/src/exec/decision.rs
git commit -m "feat(exec): check skill_allowlist in approval decision"
```

---

## Task 10: Run Full Test Suite

**Files:** None (verification only)

**Step 1: Run all tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test --lib 2>&1 | tail -50`
Expected: All tests pass (except pre-existing fastembed failures)

**Step 2: Run clippy**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo clippy --lib -- -D warnings 2>&1 | head -50`
Expected: No new warnings

**Step 3: Verify backwards compatibility**

Run: `cd /Volumes/TBU4/Workspace/Aether/.worktrees/skill-requirements && cargo test skills::tests::test_parse_valid_skill --lib`
Expected: PASS (existing SKILL.md format still works)

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Core types (PackageManager, SkillRequirements, SkillHealth) | types.rs |
| 2 | Extend SkillFrontmatter | mod.rs |
| 3 | HealthChecker implementation | health.rs |
| 4 | Install suggestion methods | installer.rs |
| 5 | Health methods in Registry | registry.rs |
| 6 | CLI Wrapper Validator | cli_wrapper.rs |
| 7 | ExecContext skill fields | decision.rs |
| 8 | skill_allowlist config | config.rs |
| 9 | Skill allowlist in decision | decision.rs |
| 10 | Full test verification | - |

Total: ~400 lines of new code, 30+ tests
