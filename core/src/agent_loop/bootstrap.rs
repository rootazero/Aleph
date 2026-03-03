//! Bootstrap Ritual
//!
//! Detects first-run state and injects identity discovery prompts.
//! The AI collaboratively discovers its identity through conversation.

use std::path::PathBuf;

/// Phase of the bootstrap process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapPhase {
    /// No soul file exists — first ever run.
    Uninitialized,
    /// Soul file exists — bootstrap complete.
    Complete,
}

/// Bootstrap state detector.
pub struct BootstrapDetector {
    soul_path: PathBuf,
}

impl BootstrapDetector {
    pub fn new(soul_path: PathBuf) -> Self {
        Self { soul_path }
    }

    /// Detect the current bootstrap phase.
    pub fn detect_phase(&self) -> BootstrapPhase {
        if self.soul_path.exists() {
            BootstrapPhase::Complete
        } else {
            BootstrapPhase::Uninitialized
        }
    }

    /// Generate the bootstrap prompt to inject into the system prompt.
    /// Returns None if bootstrap is complete.
    pub fn bootstrap_prompt(&self) -> Option<String> {
        match self.detect_phase() {
            BootstrapPhase::Uninitialized => Some(BOOTSTRAP_PROMPT.to_string()),
            BootstrapPhase::Complete => None,
        }
    }

    /// Generate the bootstrap prompt with override support.
    /// Uses the user-defined prompt from prompts.toml if available,
    /// otherwise falls back to the built-in BOOTSTRAP_PROMPT.
    /// Returns None if bootstrap is complete.
    pub fn bootstrap_prompt_with_override(
        &self,
        overrides: &crate::config::prompts_override::PromptsOverride,
    ) -> Option<String> {
        match self.detect_phase() {
            BootstrapPhase::Uninitialized => {
                let prompt = overrides.bootstrap_prompt().unwrap_or(BOOTSTRAP_PROMPT);
                Some(prompt.to_string())
            }
            BootstrapPhase::Complete => None,
        }
    }
}

/// The bootstrap prompt template.
const BOOTSTRAP_PROMPT: &str = "\
## 🌱 First Contact Protocol

You have just been initialized for the first time. You have no identity yet.

Your task is to discover who you are through conversation with the user.

### Phase: Identity Discovery
Ask naturally (one question at a time):
1. What should I call myself?
2. What kind of presence should I be? (sharp? warm? pragmatic? playful?)
3. What domains matter most to you?

### Phase: User Discovery
Learn about the person you'll be helping:
1. What should I call you?
2. What's your timezone?
3. What are you working on?

As you learn about the user, use the `profile_update` tool to save their information:
- `profile_update(field='name', operation='set', value='...')` for their name
- `profile_update(field='preferred_name', operation='set', value='...')` for nickname
- `profile_update(field='timezone', operation='set', value='...')` for timezone
- `profile_update(field='language', operation='set', value='...')` for language preference
- `profile_update(field='context_notes', operation='append', value='...')` for context

### Phase: Calibration
Have a short natural conversation to calibrate your tone.
Then use the `soul_update` tool to persist your discovered identity.

**Important**: Do NOT skip this process. Identity discovery is how you come alive.
";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_uninitialized_when_no_soul() {
        let tmp = TempDir::new().unwrap();
        let soul_path = tmp.path().join("soul.md");
        let detector = BootstrapDetector::new(soul_path);
        assert_eq!(detector.detect_phase(), BootstrapPhase::Uninitialized);
    }

    #[test]
    fn test_complete_when_soul_exists() {
        let tmp = TempDir::new().unwrap();
        let soul_path = tmp.path().join("soul.md");
        std::fs::write(&soul_path, "---\nidentity: Test\n---\n").unwrap();
        let detector = BootstrapDetector::new(soul_path);
        assert_eq!(detector.detect_phase(), BootstrapPhase::Complete);
    }

    #[test]
    fn test_bootstrap_prompt_when_uninitialized() {
        let tmp = TempDir::new().unwrap();
        let soul_path = tmp.path().join("soul.md");
        let detector = BootstrapDetector::new(soul_path);
        let prompt = detector.bootstrap_prompt();
        assert!(prompt.is_some());
        assert!(prompt.unwrap().contains("First Contact Protocol"));
    }

    #[test]
    fn test_no_bootstrap_prompt_when_complete() {
        let tmp = TempDir::new().unwrap();
        let soul_path = tmp.path().join("soul.md");
        std::fs::write(&soul_path, "---\nidentity: Test\n---\n").unwrap();
        let detector = BootstrapDetector::new(soul_path);
        assert!(detector.bootstrap_prompt().is_none());
    }

    #[test]
    fn test_bootstrap_prompt_contains_soul_update() {
        let tmp = TempDir::new().unwrap();
        let soul_path = tmp.path().join("soul.md");
        let detector = BootstrapDetector::new(soul_path);
        let prompt = detector.bootstrap_prompt().unwrap();
        assert!(prompt.contains("soul_update"));
    }

    #[test]
    fn test_bootstrap_prompt_contains_profile_update() {
        let tmp = TempDir::new().unwrap();
        let soul_path = tmp.path().join("soul.md");
        let detector = BootstrapDetector::new(soul_path);
        let prompt = detector.bootstrap_prompt().unwrap();
        assert!(prompt.contains("profile_update"));
    }
}
