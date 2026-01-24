//! Configuration types for session compaction

use std::future::Future;
use std::pin::Pin;

/// Configuration for session compaction behavior
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Enable automatic compaction when overflow detected
    pub auto_compact: bool,
    /// Enable pruning of old tool outputs
    pub prune_enabled: bool,
    /// Minimum tokens to save before pruning (default: 20,000)
    pub prune_minimum: u64,
    /// Protect this many tokens of recent tool outputs (default: 40,000)
    pub prune_protect: u64,
    /// Tools that should never have their outputs pruned
    pub protected_tools: Vec<String>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto_compact: true,
            prune_enabled: true,
            prune_minimum: 20_000,
            prune_protect: 40_000,
            protected_tools: vec!["skill".to_string()],
        }
    }
}

/// Information about a pruning operation
///
/// This struct tracks the results of a prune_with_thresholds operation,
/// including how many tokens and parts were pruned or protected.
#[derive(Debug, Clone, Default)]
pub struct PruneInfo {
    /// Total tokens pruned (estimated)
    pub tokens_pruned: u64,
    /// Number of parts whose outputs were pruned
    pub parts_pruned: usize,
    /// Number of parts protected from pruning (e.g., skill tool outputs)
    pub parts_protected: usize,
}

/// Type alias for LLM callback function
///
/// The callback takes a system prompt and user content, returns a future that
/// resolves to the LLM's response string.
pub type LlmCallback = Box<
    dyn Fn(String, String) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync,
>;

/// Compaction summary prompt (matches OpenCode's compaction.txt)
///
/// This prompt guides the LLM to generate a comprehensive summary that
/// enables seamless continuation of the conversation in a new session.
pub(super) const COMPACTION_PROMPT: &str = r#"You are a helpful AI assistant tasked with summarizing conversations.

Provide a detailed prompt for continuing our conversation above. Focus on information that would be helpful for continuing the conversation:
- What was done
- What is currently being worked on
- Which files are being modified
- What needs to be done next
- Key user requests, constraints, or preferences
- Important technical decisions and why they were made

Write in a way that allows a new session to continue seamlessly without access to the full conversation history."#;

/// Get the compaction prompt used for LLM-driven summarization
///
/// This is exposed for testing and customization purposes.
pub fn compaction_prompt() -> &'static str {
    COMPACTION_PROMPT
}
