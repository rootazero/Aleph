//! SessionResumeLayer — inject previous session context (priority 1760)
//!
//! Sits just after SessionContextGuideLayer (1750).
//! Only injects content when `input.session_snapshot` is set.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

/// Injects a previous session snapshot into the system prompt so the LLM
/// can seamlessly continue from where the last session left off.
pub struct SessionResumeLayer;

impl PromptLayer for SessionResumeLayer {
    fn name(&self) -> &'static str {
        "session_resume"
    }

    fn priority(&self) -> u32 {
        1760
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let snapshot = match input.session_snapshot {
            Some(s) => s,
            None => return,
        };
        output.push_str("\n\n");
        output.push_str(&snapshot.to_prompt_text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::session_resume::SessionSnapshot;
    use crate::thinker::prompt_builder::PromptConfig;
    use chrono::Utc;

    fn make_snapshot() -> SessionSnapshot {
        SessionSnapshot {
            session_id: "test-session".into(),
            created_at: Utc::now(),
            summary: "Worked on feature X.".into(),
            key_decisions: vec!["Use trait objects".into()],
            active_files: vec!["src/lib.rs".into()],
            tool_state: None,
            pending_tasks: vec!["Write tests".into()],
        }
    }

    #[test]
    fn injects_snapshot_when_present() {
        let layer = SessionResumeLayer;
        let config = PromptConfig::default();
        let snapshot = make_snapshot();
        let input = LayerInput::basic(&config, &[]).with_session_snapshot(&snapshot);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Previous Session Context"));
        assert!(out.contains("Worked on feature X."));
        assert!(out.contains("Use trait objects"));
    }

    #[test]
    fn skips_when_none() {
        let layer = SessionResumeLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn stability_is_dynamic() {
        assert_eq!(SessionResumeLayer.stability(), LayerStability::Dynamic);
    }

    #[test]
    fn priority_is_1760() {
        assert_eq!(SessionResumeLayer.priority(), 1760);
    }

    #[test]
    fn supports_full_only() {
        let layer = SessionResumeLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }
}
