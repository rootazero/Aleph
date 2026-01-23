//! Confidence threshold configuration and classification

use serde::{Deserialize, Serialize};

/// Action to take based on confidence level
///
/// The confidence score determines what action the dispatcher should take:
/// - Very low confidence: No tool match, fall back to general chat
/// - Low confidence: Tool match but requires user confirmation
/// - Medium confidence: Tool match with optional confirmation (based on config)
/// - High confidence: Auto-execute without confirmation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceAction {
    /// Confidence too low - no tool matched, fall back to general chat
    NoMatch,

    /// Tool matched but confidence is low - requires user confirmation
    RequiresConfirmation,

    /// Tool matched with medium confidence - confirmation is optional
    OptionalConfirmation,

    /// Tool matched with high confidence - auto-execute without confirmation
    AutoExecute,
}

/// Unified confidence threshold configuration
///
/// Provides a single source of truth for all confidence thresholds used
/// in the Dispatcher Layer. This eliminates scattered threshold definitions
/// and ensures consistent behavior across L1/L2/L3 routing.
///
/// # Threshold Ordering
///
/// The thresholds must be ordered: `no_match < requires_confirmation <= auto_execute`
///
/// # Default Values
///
/// - `no_match`: 0.3 - Below this, no tool is considered matched
/// - `requires_confirmation`: 0.7 - Below this, confirmation is required
/// - `auto_execute`: 0.9 - Above this, auto-execute without confirmation
///
/// # Confidence Ranges
///
/// ```text
/// 0.0 ─────────── no_match ─────────── requires_confirmation ─────────── auto_execute ─────────── 1.0
///      NoMatch              RequiresConfirmation              OptionalConfirmation       AutoExecute
/// ```
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::dispatcher::ConfidenceThresholds;
///
/// let thresholds = ConfidenceThresholds::default();
///
/// // Classify confidence scores
/// assert_eq!(thresholds.classify(0.2), ConfidenceAction::NoMatch);
/// assert_eq!(thresholds.classify(0.5), ConfidenceAction::RequiresConfirmation);
/// assert_eq!(thresholds.classify(0.8), ConfidenceAction::OptionalConfirmation);
/// assert_eq!(thresholds.classify(0.95), ConfidenceAction::AutoExecute);
/// ```
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ConfidenceThresholds {
    /// Minimum confidence for a tool to be considered matched (default: 0.3)
    /// Below this threshold, the input falls back to general chat.
    pub no_match: f32,

    /// Confidence below which confirmation is always required (default: 0.7)
    /// Between `no_match` and this threshold, confirmation is mandatory.
    pub requires_confirmation: f32,

    /// Confidence above which auto-execute is allowed (default: 0.9)
    /// Above this threshold, tools execute without confirmation.
    pub auto_execute: f32,
}

impl Default for ConfidenceThresholds {
    fn default() -> Self {
        Self {
            no_match: 0.3,
            requires_confirmation: 0.7,
            auto_execute: 0.9,
        }
    }
}

impl ConfidenceThresholds {
    /// Create thresholds with custom values
    pub fn new(no_match: f32, requires_confirmation: f32, auto_execute: f32) -> Self {
        Self {
            no_match,
            requires_confirmation,
            auto_execute,
        }
    }

    /// Validate the threshold ordering
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Thresholds are valid
    /// * `Err(String)` - Validation error message
    ///
    /// # Validation Rules
    ///
    /// 1. All thresholds must be in range [0.0, 1.0]
    /// 2. no_match < requires_confirmation <= auto_execute
    pub fn validate(&self) -> std::result::Result<(), String> {
        // Check range
        if self.no_match < 0.0 || self.no_match > 1.0 {
            return Err(format!(
                "no_match threshold must be in [0.0, 1.0], got {}",
                self.no_match
            ));
        }
        if self.requires_confirmation < 0.0 || self.requires_confirmation > 1.0 {
            return Err(format!(
                "requires_confirmation threshold must be in [0.0, 1.0], got {}",
                self.requires_confirmation
            ));
        }
        if self.auto_execute < 0.0 || self.auto_execute > 1.0 {
            return Err(format!(
                "auto_execute threshold must be in [0.0, 1.0], got {}",
                self.auto_execute
            ));
        }

        // Check ordering
        if self.no_match >= self.requires_confirmation {
            return Err(format!(
                "no_match ({}) must be less than requires_confirmation ({})",
                self.no_match, self.requires_confirmation
            ));
        }
        if self.requires_confirmation > self.auto_execute {
            return Err(format!(
                "requires_confirmation ({}) must not exceed auto_execute ({})",
                self.requires_confirmation, self.auto_execute
            ));
        }

        Ok(())
    }

    /// Classify a confidence score into an action
    ///
    /// # Arguments
    ///
    /// * `confidence` - The confidence score (0.0 to 1.0)
    ///
    /// # Returns
    ///
    /// The appropriate `ConfidenceAction` for the given confidence level.
    pub fn classify(&self, confidence: f32) -> ConfidenceAction {
        if confidence < self.no_match {
            ConfidenceAction::NoMatch
        } else if confidence < self.requires_confirmation {
            ConfidenceAction::RequiresConfirmation
        } else if confidence < self.auto_execute {
            ConfidenceAction::OptionalConfirmation
        } else {
            ConfidenceAction::AutoExecute
        }
    }

    /// Check if confirmation is needed for a given confidence
    ///
    /// This is a convenience method that returns true if the confidence
    /// falls in the RequiresConfirmation or OptionalConfirmation range.
    ///
    /// # Arguments
    ///
    /// * `confidence` - The confidence score
    /// * `confirmation_enabled` - Whether confirmation is enabled in config
    ///
    /// # Returns
    ///
    /// `true` if confirmation should be shown, `false` otherwise
    pub fn needs_confirmation(&self, confidence: f32, confirmation_enabled: bool) -> bool {
        if !confirmation_enabled {
            return false;
        }

        match self.classify(confidence) {
            ConfidenceAction::NoMatch => false, // No match = no confirmation (fall back to chat)
            ConfidenceAction::RequiresConfirmation => true,
            ConfidenceAction::OptionalConfirmation => false, // Optional = don't require
            ConfidenceAction::AutoExecute => false,
        }
    }
}
