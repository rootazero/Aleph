//! POE context for injection into PromptPipeline.

use crate::poe::types::SuccessManifest;

/// Context passed to PoePromptLayer for injection into the system prompt.
#[derive(Debug, Clone, Default)]
pub struct PoePromptContext {
    /// Active success contract
    pub manifest: Option<SuccessManifest>,
    /// Current step hint (from StepEvaluator, consumed once)
    pub current_hint: Option<String>,
    /// Progress summary (e.g., "3/5 constraints met")
    pub progress_summary: Option<String>,
}

impl PoePromptContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_manifest(mut self, manifest: SuccessManifest) -> Self {
        self.manifest = Some(manifest);
        self
    }

    pub fn with_hint(mut self, hint: String) -> Self {
        self.current_hint = Some(hint);
        self
    }

    pub fn with_progress(mut self, summary: String) -> Self {
        self.progress_summary = Some(summary);
        self
    }

    /// Whether any POE context is present (worth injecting).
    pub fn has_content(&self) -> bool {
        self.manifest.is_some() || self.current_hint.is_some() || self.progress_summary.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_context_has_no_content() {
        let ctx = PoePromptContext::new();
        assert!(!ctx.has_content());
    }

    #[test]
    fn test_context_with_manifest_has_content() {
        let manifest = SuccessManifest::new("t1", "do X");
        let ctx = PoePromptContext::new().with_manifest(manifest);
        assert!(ctx.has_content());
    }

    #[test]
    fn test_context_with_hint_has_content() {
        let ctx = PoePromptContext::new().with_hint("check output".into());
        assert!(ctx.has_content());
    }
}
