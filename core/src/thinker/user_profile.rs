//! User Profile
//!
//! Structured representation of the user being helped, loaded from
//! ~/.aleph/user_profile.md (Markdown with YAML frontmatter).

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::thinker::soul::Verbosity;

/// Proactivity level for AI interactions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProactivityLevel {
    /// Only respond when called
    Reactive,
    /// Occasionally offer suggestions
    Balanced,
    /// Actively provide help
    Proactive,
}

impl Default for ProactivityLevel {
    fn default() -> Self {
        Self::Balanced
    }
}

/// Interaction preferences for AI behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionPrefs {
    /// Response verbosity preference
    #[serde(default)]
    pub verbosity: Verbosity,
    /// Proactivity level for the AI
    #[serde(default)]
    pub proactivity: ProactivityLevel,
}

impl Default for InteractionPrefs {
    fn default() -> Self {
        Self {
            verbosity: Verbosity::default(),
            proactivity: ProactivityLevel::default(),
        }
    }
}

/// Structured user profile loaded from markdown with YAML frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    /// User's full name
    pub name: String,
    /// Preferred name / nickname to address the user
    #[serde(default)]
    pub preferred_name: Option<String>,
    /// User's timezone (e.g., "Asia/Shanghai")
    #[serde(default)]
    pub timezone: Option<String>,
    /// Preferred language for responses
    #[serde(default)]
    pub language: Option<String>,
    /// Additional context notes about the user
    #[serde(default)]
    pub context_notes: Vec<String>,
    /// Interaction style preferences
    #[serde(default)]
    pub interaction_preferences: InteractionPrefs,
    /// Raw text addendum appended to the prompt
    #[serde(default)]
    pub addendum: Option<String>,
}

impl Default for UserProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            preferred_name: None,
            timezone: None,
            language: None,
            context_notes: Vec::new(),
            interaction_preferences: InteractionPrefs::default(),
            addendum: None,
        }
    }
}

impl UserProfile {
    /// Load user profile from a markdown file with YAML frontmatter.
    ///
    /// The file is expected to have YAML frontmatter between `---` delimiters.
    /// If no frontmatter delimiters are found, the entire file content is
    /// attempted as YAML. Returns `None` if the file doesn't exist or
    /// parsing fails.
    pub fn load_from_file(path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        let trimmed = content.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Try to extract YAML frontmatter
        if trimmed.starts_with("---") {
            let after_first = &trimmed[3..];
            if let Some(end_pos) = after_first.find("\n---") {
                let frontmatter = &after_first[..end_pos];
                return serde_yaml::from_str(frontmatter.trim()).ok();
            }
        }

        // No frontmatter — try parsing whole file as YAML
        serde_yaml::from_str(trimmed).ok()
    }

    /// Generate a prompt section describing this user profile.
    ///
    /// Only non-empty / non-None fields are included.
    pub fn to_prompt_section(&self) -> String {
        let mut section = String::new();
        section.push_str("## User Profile\n");

        // Name line (with optional preferred_name)
        if let Some(ref pn) = self.preferred_name {
            section.push_str(&format!("Name: {} (call them: {})\n", self.name, pn));
        } else {
            section.push_str(&format!("Name: {}\n", self.name));
        }

        // Timezone
        if let Some(ref tz) = self.timezone {
            section.push_str(&format!("Timezone: {}\n", tz));
        }

        // Language preference
        if let Some(ref lang) = self.language {
            section.push_str(&format!("Language preference: {}\n", lang));
        }

        // Interaction style
        section.push_str(&format!(
            "Interaction style: {:?} verbosity, {:?} proactivity\n",
            self.interaction_preferences.verbosity, self.interaction_preferences.proactivity,
        ));

        // Context notes
        if !self.context_notes.is_empty() {
            section.push_str("\nContext:\n");
            for note in &self.context_notes {
                section.push_str(&format!("- {}\n", note));
            }
        }

        // Addendum
        if let Some(ref addendum) = self.addendum {
            section.push('\n');
            section.push_str(addendum);
            section.push('\n');
        }

        section
    }

    /// Returns true if the profile has no meaningful content (empty name).
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_profile_is_empty() {
        let profile = UserProfile::default();
        assert!(profile.is_empty());
    }

    #[test]
    fn test_prompt_section_basic() {
        let profile = UserProfile {
            name: "Alex".to_string(),
            preferred_name: Some("Al".to_string()),
            timezone: Some("Asia/Shanghai".to_string()),
            language: Some("Chinese".to_string()),
            context_notes: vec!["Works on AI projects".to_string()],
            interaction_preferences: InteractionPrefs::default(),
            addendum: None,
        };
        let section = profile.to_prompt_section();
        assert!(section.contains("## User Profile"));
        assert!(section.contains("Alex"));
        assert!(section.contains("Al"));
        assert!(section.contains("Asia/Shanghai"));
        assert!(section.contains("AI projects"));
    }

    #[test]
    fn test_prompt_section_minimal() {
        let profile = UserProfile {
            name: "User".to_string(),
            ..Default::default()
        };
        let section = profile.to_prompt_section();
        assert!(section.contains("User"));
        assert!(!section.contains("Context"));
    }

    #[test]
    fn test_prompt_section_with_addendum() {
        let profile = UserProfile {
            name: "User".to_string(),
            addendum: Some("Prefers detailed explanations".to_string()),
            ..Default::default()
        };
        let section = profile.to_prompt_section();
        assert!(section.contains("detailed explanations"));
    }

    #[test]
    fn test_proactivity_default() {
        assert_eq!(ProactivityLevel::default(), ProactivityLevel::Balanced);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = UserProfile::load_from_file(Path::new("/nonexistent/file.md"));
        assert!(result.is_none());
    }
}
