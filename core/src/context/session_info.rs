//! Session metadata for prompt context.

/// Information about the current agent session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub capabilities: Vec<String>,
}

impl Default for SessionInfo {
    fn default() -> Self {
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now(),
            capabilities: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_session_has_valid_id() {
        let session = SessionInfo::default();
        assert!(!session.session_id.is_empty());
        // UUID v4 format: 8-4-4-4-12 hex chars
        assert_eq!(session.session_id.len(), 36);
        assert!(session.capabilities.is_empty());
    }

    #[test]
    fn two_defaults_have_different_ids() {
        let a = SessionInfo::default();
        let b = SessionInfo::default();
        assert_ne!(a.session_id, b.session_id);
    }
}
