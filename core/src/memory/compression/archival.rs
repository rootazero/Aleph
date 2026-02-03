//! Archival Service
//!
//! Handles scratchpad archiving during compression based on trigger reason.

use crate::error::AetherError;
use crate::memory::compression::trigger::{CompressionAggressiveness, TriggerReason};
use crate::memory::scratchpad::{ScratchpadManager, SessionHistory};

/// Configuration for archival behavior
#[derive(Debug, Clone)]
pub struct ArchivalConfig {
    /// Whether to extract facts from scratchpad before archiving
    pub extract_facts: bool,
    /// Whether to archive scratchpad on milestone completion
    pub archive_on_milestone: bool,
    /// Whether to archive scratchpad on token threshold trigger
    pub archive_on_threshold: bool,
}

impl Default for ArchivalConfig {
    fn default() -> Self {
        Self {
            extract_facts: true,
            archive_on_milestone: true,
            archive_on_threshold: false, // Only archive on semantic boundaries
        }
    }
}

/// Result of an archival operation
#[derive(Debug)]
pub struct ArchivalResult {
    /// Whether scratchpad was archived
    pub archived: bool,
    /// Content that was archived (if any)
    pub archived_content: Option<String>,
    /// Number of facts extracted
    pub facts_extracted: usize,
    /// Reason for archival decision
    pub reason: String,
}

/// Service for archiving scratchpad content during compression
pub struct ArchivalService {
    config: ArchivalConfig,
}

impl ArchivalService {
    /// Create a new archival service
    pub fn new(config: ArchivalConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(ArchivalConfig::default())
    }

    /// Determine if scratchpad should be archived based on trigger reason
    pub fn should_archive(&self, reason: &TriggerReason) -> bool {
        match reason.aggressiveness() {
            CompressionAggressiveness::Full => self.config.archive_on_milestone,
            CompressionAggressiveness::Aggressive => self.config.archive_on_threshold,
            CompressionAggressiveness::TopicOnly => false, // Don't archive on topic switch
            CompressionAggressiveness::Normal => false,
        }
    }

    /// Archive scratchpad content
    ///
    /// This method:
    /// 1. Reads the current scratchpad content
    /// 2. Appends to session history
    /// 3. Clears the scratchpad
    pub async fn archive_scratchpad(
        &self,
        manager: &ScratchpadManager,
        session_id: &str,
    ) -> Result<ArchivalResult, AetherError> {
        // Check if scratchpad has meaningful content
        if !manager.exists() || !manager.has_content().await? {
            return Ok(ArchivalResult {
                archived: false,
                archived_content: None,
                facts_extracted: 0,
                reason: "Scratchpad empty or has no meaningful content".into(),
            });
        }

        // Read content before archiving
        let content = manager.read().await?;

        // Get history path and create SessionHistory
        let history = SessionHistory::new(manager.history_path());

        // Append to history
        history.append(&content, session_id).await?;

        // Clear scratchpad
        manager.clear().await?;

        Ok(ArchivalResult {
            archived: true,
            archived_content: Some(content),
            facts_extracted: 0, // TODO: integrate with FactExtractor
            reason: "Scratchpad archived on task completion".into(),
        })
    }

    /// Process archival based on trigger reason
    pub async fn process(
        &self,
        reason: &TriggerReason,
        manager: &ScratchpadManager,
        session_id: &str,
    ) -> Result<ArchivalResult, AetherError> {
        if !self.should_archive(reason) {
            return Ok(ArchivalResult {
                archived: false,
                archived_content: None,
                facts_extracted: 0,
                reason: format!(
                    "Archival not triggered for {:?}",
                    reason.aggressiveness()
                ),
            });
        }

        self.archive_scratchpad(manager, session_id).await
    }

    /// Get configuration
    pub fn config(&self) -> &ArchivalConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::compression::signal_detector::CompressionSignal;
    use tempfile::tempdir;

    #[test]
    fn test_should_archive_milestone() {
        let service = ArchivalService::with_defaults();

        let milestone = TriggerReason::Signal(CompressionSignal::Milestone {
            task_description: "test".to_string(),
            completion_indicator: "done".to_string(),
        });

        assert!(service.should_archive(&milestone));
    }

    #[test]
    fn test_should_not_archive_topic_switch() {
        let service = ArchivalService::with_defaults();

        let topic_switch = TriggerReason::Signal(CompressionSignal::ContextSwitch {
            from_topic: "old".to_string(),
            to_topic: "new".to_string(),
        });

        assert!(!service.should_archive(&topic_switch));
    }

    #[test]
    fn test_should_not_archive_token_threshold_by_default() {
        let service = ArchivalService::with_defaults();

        let threshold = TriggerReason::TokenThreshold {
            current: 120_000,
            max: 115_200,
        };

        assert!(!service.should_archive(&threshold));
    }

    #[tokio::test]
    async fn test_archive_empty_scratchpad() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::new(temp.path().to_path_buf(), "test-session");
        let service = ArchivalService::with_defaults();

        let result = service
            .archive_scratchpad(&manager, "test-session")
            .await
            .unwrap();

        assert!(!result.archived);
    }

    #[tokio::test]
    async fn test_archive_with_content() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::new(temp.path().to_path_buf(), "test-session");
        let service = ArchivalService::with_defaults();

        // Initialize with content
        manager.initialize(Some("Build auth module")).await.unwrap();
        manager.set_plan(&["Step 1", "Step 2"]).await.unwrap();

        let result = service
            .archive_scratchpad(&manager, "test-session")
            .await
            .unwrap();

        assert!(result.archived);
        assert!(result.archived_content.is_some());

        // Verify scratchpad is cleared
        assert!(!manager.has_content().await.unwrap());

        // Verify history has the content
        let history = SessionHistory::new(manager.history_path());
        let entries = history.read_recent(10).await.unwrap();
        assert_eq!(entries.len(), 1);
    }
}
