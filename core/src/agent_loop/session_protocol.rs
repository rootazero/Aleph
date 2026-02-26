//! Session Protocol
//!
//! Describes the automatically loaded context for each session
//! and recommends initial actions for the AI.

/// What gets automatically injected into the session.
#[derive(Debug, Clone)]
pub enum AutoInjectItem {
    SoulManifest,
    UserProfile,
    RuntimeContext,
    EnvironmentContract,
}

/// What the AI should consider reading at session start.
#[derive(Debug, Clone)]
pub enum RecommendedRead {
    RecentMemory,
    ProjectContext,
    PendingTasks,
}

/// Session protocol configuration.
#[derive(Debug, Clone)]
pub struct SessionProtocol {
    pub auto_injected: Vec<AutoInjectItem>,
    pub recommended_reads: Vec<RecommendedRead>,
}

impl Default for SessionProtocol {
    fn default() -> Self {
        Self {
            auto_injected: vec![
                AutoInjectItem::SoulManifest,
                AutoInjectItem::UserProfile,
                AutoInjectItem::RuntimeContext,
                AutoInjectItem::EnvironmentContract,
            ],
            recommended_reads: vec![
                RecommendedRead::RecentMemory,
                RecommendedRead::PendingTasks,
            ],
        }
    }
}

impl SessionProtocol {
    /// Generate the prompt section describing this session's context.
    pub fn to_prompt_section(&self) -> String {
        let mut section = String::from("## Session Context\n\n");

        section.push_str("The following context has been automatically loaded for this session:\n");
        for item in &self.auto_injected {
            let desc = match item {
                AutoInjectItem::SoulManifest => "Your identity and personality (from soul manifest)",
                AutoInjectItem::UserProfile => "User profile (preferences, timezone, interaction style)",
                AutoInjectItem::RuntimeContext => "Runtime environment (OS, shell, working directory)",
                AutoInjectItem::EnvironmentContract => "Available capabilities for this interaction mode",
            };
            section.push_str(&format!("- {}\n", desc));
        }

        if !self.recommended_reads.is_empty() {
            section.push_str("\n### Recommended First Actions\n");
            section.push_str("If this is a continuing conversation, consider:\n");
            for (i, read) in self.recommended_reads.iter().enumerate() {
                let desc = match read {
                    RecommendedRead::RecentMemory => "Checking recent memory for relevant context",
                    RecommendedRead::ProjectContext => "Reviewing the current project state",
                    RecommendedRead::PendingTasks => "Reviewing any pending tasks from previous sessions",
                };
                section.push_str(&format!("{}. {}\n", i + 1, desc));
            }
        }

        section.push('\n');
        section
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_protocol() {
        let protocol = SessionProtocol::default();
        assert_eq!(protocol.auto_injected.len(), 4);
        assert_eq!(protocol.recommended_reads.len(), 2);
    }

    #[test]
    fn test_prompt_section_contains_auto_injected() {
        let protocol = SessionProtocol::default();
        let section = protocol.to_prompt_section();
        assert!(section.contains("## Session Context"));
        assert!(section.contains("soul manifest"));
        assert!(section.contains("User profile"));
        assert!(section.contains("Runtime environment"));
        assert!(section.contains("interaction mode"));
    }

    #[test]
    fn test_prompt_section_contains_recommended_reads() {
        let protocol = SessionProtocol::default();
        let section = protocol.to_prompt_section();
        assert!(section.contains("Recommended First Actions"));
        assert!(section.contains("memory"));
        assert!(section.contains("pending tasks"));
    }

    #[test]
    fn test_empty_recommended_reads() {
        let protocol = SessionProtocol {
            auto_injected: vec![AutoInjectItem::SoulManifest],
            recommended_reads: vec![],
        };
        let section = protocol.to_prompt_section();
        assert!(section.contains("soul manifest"));
        assert!(!section.contains("Recommended First Actions"));
    }
}
