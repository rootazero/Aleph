/// Progress information for long-running generation operations
///
/// Used for video and audio generation which may take significant time.
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProgress {
    /// Current progress percentage (0-100)
    pub percentage: f32,
    /// Current step/phase description
    pub step: String,
    /// Estimated time remaining
    pub eta: Option<Duration>,
    /// Whether the operation is complete
    pub is_complete: bool,
    /// Optional preview URL
    pub preview_url: Option<String>,
}

impl GenerationProgress {
    /// Create a new progress indicator
    ///
    /// # Arguments
    ///
    /// * `percentage` - Progress from 0 to 100
    /// * `step` - Description of current step
    pub fn new<S: Into<String>>(percentage: f32, step: S) -> Self {
        Self {
            percentage: percentage.clamp(0.0, 100.0),
            step: step.into(),
            eta: None,
            is_complete: percentage >= 100.0,
            preview_url: None,
        }
    }

    /// Create a progress indicator for a started operation
    pub fn started<S: Into<String>>(step: S) -> Self {
        Self::new(0.0, step)
    }

    /// Create a progress indicator for a completed operation
    pub fn completed() -> Self {
        Self {
            percentage: 100.0,
            step: "Complete".to_string(),
            eta: None,
            is_complete: true,
            preview_url: None,
        }
    }

    /// Set the ETA
    pub fn with_eta(mut self, eta: Duration) -> Self {
        self.eta = Some(eta);
        self
    }

    /// Set a preview URL
    pub fn with_preview<S: Into<String>>(mut self, url: S) -> Self {
        self.preview_url = Some(url.into());
        self
    }
}

impl Default for GenerationProgress {
    fn default() -> Self {
        Self::new(0.0, "Starting")
    }
}
