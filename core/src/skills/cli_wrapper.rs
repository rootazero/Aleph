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
    pub fn validate_command(skill: &Skill, command: &str) -> Result<(), CliWrapperError> {
        if !skill.frontmatter.cli_wrapper {
            return Err(CliWrapperError::NotCliWrapper);
        }

        let command = command.trim();
        if command.is_empty() {
            return Err(CliWrapperError::EmptyCommand);
        }

        let req = skill
            .frontmatter
            .requirements
            .as_ref()
            .ok_or(CliWrapperError::NoRequirements)?;

        let binary = command
            .split_whitespace()
            .next()
            .ok_or(CliWrapperError::EmptyCommand)?;

        // Reject shell metacharacters that could enable command injection
        if command.contains(';') || command.contains('|') || command.contains('&')
            || command.contains('`') || command.contains("$(")
        {
            return Err(CliWrapperError::UnauthorizedBinary {
                attempted: "shell metacharacters detected".to_string(),
                allowed: req.binaries.clone(),
            });
        }

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
