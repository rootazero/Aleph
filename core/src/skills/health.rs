//! Health checking for skill dependencies.

use crate::skills::{Skill, SkillHealth};
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
    use crate::skills::{SkillFrontmatter, SkillRequirements};

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
        assert!(!HealthChecker::check_platform(&Some(vec![
            "nonexistent_os".to_string()
        ])));
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
        assert_eq!(
            HealthChecker::check_skill(&skill),
            SkillHealth::Unsupported
        );
    }
}
