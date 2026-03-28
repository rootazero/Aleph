//! Compression strategies (stubbed)
//!
//! Previously used LoopStep from the old OTAF agent loop.
//! Preserved as stubs for backward compatibility.

/// Prompt template for LLM-based compression (stubbed)
pub struct CompressionPrompt;

impl CompressionPrompt {
    /// Build compression prompt (stubbed - returns empty string)
    pub fn build(_current_summary: &str, _steps: &[()], _target_tokens: usize) -> String {
        String::new()
    }
}
