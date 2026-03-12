//! Compression strategies (stubbed)
//!
//! Previously used LoopStep from the old OTAF agent loop.
//! Preserved as stubs for backward compatibility.

/// Key information extracted from steps for preservation
#[derive(Debug, Clone, Default)]
pub struct KeyInfo {
    /// Files that were created or modified
    pub file_changes: Vec<String>,
    /// Important tool outputs (search results, errors, etc.)
    pub important_outputs: Vec<String>,
    /// User decisions made during execution
    pub user_decisions: Vec<String>,
    /// Current state description
    pub current_state: String,
}

/// Strategy for extracting key information from steps (stubbed)
pub struct KeyInfoExtractor;

/// Rule-based compression strategy (stubbed)
pub struct RuleBasedStrategy;

/// Prompt template for LLM-based compression (stubbed)
pub struct CompressionPrompt;

impl CompressionPrompt {
    /// Build compression prompt (stubbed - returns empty string)
    pub fn build(_current_summary: &str, _steps: &[()], _target_tokens: usize) -> String {
        String::new()
    }
}
