//! Agent response types

/// Response from agent processing
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// Generated response text
    pub content: String,
    /// Tools that were called during processing
    pub tools_called: Vec<String>,
}

impl AgentResponse {
    /// Create a new AgentResponse
    pub fn new(content: String, tools_called: Vec<String>) -> Self {
        Self {
            content,
            tools_called,
        }
    }

    /// Create a simple response with no tools called
    pub fn simple(content: String) -> Self {
        Self {
            content,
            tools_called: Vec::new(),
        }
    }
}
