//! Tool Safety Level
//!
//! Safety level classification for tool confirmation and rollback behavior.

use serde::{Deserialize, Serialize};

/// Tool safety level for confirmation and rollback behavior
///
/// Determines whether user confirmation is required before execution
/// and whether the operation can be rolled back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ToolSafetyLevel {
    /// Read-only operations that don't modify anything
    /// No confirmation required, instant execution
    #[default]
    ReadOnly,

    /// Operations that can be undone/reversed
    /// May require confirmation based on config
    Reversible,

    /// Operations that cannot be undone but have low impact
    /// (e.g., sending a message, posting a comment)
    /// Usually requires confirmation
    IrreversibleLowRisk,

    /// Operations that cannot be undone and have high impact
    /// (e.g., deleting files, dropping tables)
    /// Always requires confirmation
    IrreversibleHighRisk,
}

impl ToolSafetyLevel {
    /// Check if this safety level requires user confirmation
    pub fn requires_confirmation(&self) -> bool {
        matches!(
            self,
            ToolSafetyLevel::IrreversibleLowRisk | ToolSafetyLevel::IrreversibleHighRisk
        )
    }

    /// Get a human-readable label for this safety level
    pub fn label(&self) -> &'static str {
        match self {
            ToolSafetyLevel::ReadOnly => "Read Only",
            ToolSafetyLevel::Reversible => "Reversible",
            ToolSafetyLevel::IrreversibleLowRisk => "Low Risk",
            ToolSafetyLevel::IrreversibleHighRisk => "High Risk",
        }
    }

    /// Get a badge color hint for UI (SF Symbol color name)
    pub fn color_hint(&self) -> &'static str {
        match self {
            ToolSafetyLevel::ReadOnly => "green",
            ToolSafetyLevel::Reversible => "blue",
            ToolSafetyLevel::IrreversibleLowRisk => "yellow",
            ToolSafetyLevel::IrreversibleHighRisk => "red",
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safety_level_requires_confirmation() {
        assert!(!ToolSafetyLevel::ReadOnly.requires_confirmation());
        assert!(!ToolSafetyLevel::Reversible.requires_confirmation());
        assert!(ToolSafetyLevel::IrreversibleLowRisk.requires_confirmation());
        assert!(ToolSafetyLevel::IrreversibleHighRisk.requires_confirmation());
    }

    #[test]
    fn test_safety_level_label() {
        assert_eq!(ToolSafetyLevel::ReadOnly.label(), "Read Only");
        assert_eq!(ToolSafetyLevel::Reversible.label(), "Reversible");
        assert_eq!(ToolSafetyLevel::IrreversibleLowRisk.label(), "Low Risk");
        assert_eq!(ToolSafetyLevel::IrreversibleHighRisk.label(), "High Risk");
    }

    #[test]
    fn test_safety_level_color_hint() {
        assert_eq!(ToolSafetyLevel::ReadOnly.color_hint(), "green");
        assert_eq!(ToolSafetyLevel::Reversible.color_hint(), "blue");
        assert_eq!(ToolSafetyLevel::IrreversibleLowRisk.color_hint(), "yellow");
        assert_eq!(ToolSafetyLevel::IrreversibleHighRisk.color_hint(), "red");
    }

    #[test]
    fn test_safety_level_default() {
        let default = ToolSafetyLevel::default();
        assert_eq!(default, ToolSafetyLevel::ReadOnly);
    }
}
