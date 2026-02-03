//! Skill types for requirements and health checking.

use serde::{Deserialize, Serialize};

/// Package manager type for installation commands
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageManager {
    Brew,   // macOS Homebrew
    Apt,    // Debian/Ubuntu apt
    Winget, // Windows winget
    Cargo,  // Rust cargo
    Pip,    // Python pip
}

/// Single installation command for a package manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallCommand {
    pub manager: PackageManager,
    pub package: String,
    #[serde(default)]
    pub args: Option<String>,
}

/// Skill dependency requirements
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRequirements {
    #[serde(default)]
    pub binaries: Vec<String>,
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
    #[serde(default)]
    pub install: Vec<InstallCommand>,
}

/// Skill health status after dependency check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillHealth {
    Healthy,
    Degraded { missing: Vec<String> },
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
            SkillHealth::Degraded {
                missing: vec!["gh".into()]
            }
        );
    }
}
