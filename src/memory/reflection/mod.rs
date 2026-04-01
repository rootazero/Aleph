//! Session-end reflection system.
pub mod mapper;
pub mod parser;
pub mod prompt;
pub mod service;

/// An action item extracted from session reflection.
/// Open loops are NOT memory facts — they are follow-up actions
/// that the DreamDaemon will track and resolve.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenLoopAction {
    /// Description of the follow-up action.
    pub description: String,
    /// Session that generated this open loop.
    pub source_session_id: String,
    /// When this open loop was detected.
    pub created_at: i64,
    /// Whether this has been resolved.
    pub resolved: bool,
}

impl OpenLoopAction {
    pub fn new(description: String, session_id: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Self {
            description,
            source_session_id: session_id,
            created_at: now,
            resolved: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_loop_action_defaults() {
        let action = OpenLoopAction::new("Check perf".into(), "sess-1".into());
        assert_eq!(action.description, "Check perf");
        assert!(!action.resolved);
        assert!(action.created_at > 0);
    }
}
