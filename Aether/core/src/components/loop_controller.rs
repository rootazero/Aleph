//! Loop controller component - manages agentic loop with protection mechanisms.
//!
//! Subscribes to: ToolCallCompleted, ToolCallFailed, PlanCreated
//! Publishes: LoopContinue, LoopStop

/// Configuration for the agentic loop
#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub max_iterations: u32,
    pub max_tokens: u64,
    pub doom_loop_threshold: u32,
    pub doom_loop_window_secs: u64,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            max_tokens: 100_000,
            doom_loop_threshold: 5,
            doom_loop_window_secs: 60,
        }
    }
}

/// Loop Controller - manages loop state and protection
pub struct LoopController;
