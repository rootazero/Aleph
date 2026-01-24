//! Turn Protector for Conversation Preservation
//!
//! This module provides a dedicated component for protecting recent conversation
//! turns from compaction. A "turn" represents one user input + AI response cycle.
//!
//! # Turn Definition
//!
//! - A turn starts with a `UserInput` part
//! - A turn includes all parts until the next `UserInput`
//! - Turn index increments with each `UserInput` encountered
//!
//! # Protection Logic
//!
//! The most recent N turns are always protected from compaction:
//! - If `turn_index >= (total_turns - protected_turns)`, the turn is protected
//! - Edge cases are handled gracefully (empty sessions, more protection than turns)
//!
//! # Example
//!
//! ```rust,ignore
//! use aether_core::compressor::TurnProtector;
//!
//! let protector = TurnProtector::new(2); // Protect last 2 turns
//!
//! // Check if a specific turn is protected
//! if protector.is_protected(4, 5) {
//!     println!("Turn 4 is in the protected window");
//! }
//!
//! // Calculate turn indices for all parts
//! let turn_indices = protector.calculate_turn_index(&session.parts);
//! for (part_index, turn_index) in turn_indices {
//!     if protector.is_protected(turn_index, total_turns) {
//!         println!("Part {} (turn {}) is protected", part_index, turn_index);
//!     }
//! }
//! ```

use std::ops::Range;

use crate::components::SessionPart;

/// Turn protector for preserving recent conversation turns
///
/// This component tracks turn boundaries and determines which turns
/// should be protected from compaction based on recency.
#[derive(Debug, Clone)]
pub struct TurnProtector {
    /// Number of recent turns to protect from compaction
    protected_turns: usize,
}

impl Default for TurnProtector {
    fn default() -> Self {
        Self::new(2)
    }
}

impl TurnProtector {
    /// Create a new TurnProtector with the specified number of protected turns
    ///
    /// # Arguments
    ///
    /// * `protected_turns` - Number of most recent turns to protect
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let protector = TurnProtector::new(2);
    /// ```
    pub fn new(protected_turns: usize) -> Self {
        Self { protected_turns }
    }

    /// Builder method to set the number of protected turns
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let protector = TurnProtector::default()
    ///     .with_protected_turns(3);
    /// ```
    pub fn with_protected_turns(mut self, turns: usize) -> Self {
        self.protected_turns = turns;
        self
    }

    /// Get the number of protected turns
    pub fn protected_turns(&self) -> usize {
        self.protected_turns
    }

    /// Check if a turn at the given index is protected
    ///
    /// A turn is protected if its index is within the last N turns,
    /// where N is the configured `protected_turns` value.
    ///
    /// # Arguments
    ///
    /// * `turn_index` - The 0-based index of the turn to check
    /// * `total_turns` - Total number of turns in the session
    ///
    /// # Returns
    ///
    /// `true` if the turn is protected, `false` otherwise
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let protector = TurnProtector::new(2);
    /// // With 5 total turns (0,1,2,3,4), turns 3 and 4 are protected
    /// assert!(!protector.is_protected(2, 5)); // Not protected
    /// assert!(protector.is_protected(3, 5));  // Protected
    /// assert!(protector.is_protected(4, 5));  // Protected
    /// ```
    pub fn is_protected(&self, turn_index: usize, total_turns: usize) -> bool {
        if total_turns == 0 || self.protected_turns == 0 {
            return false;
        }

        let protected_start = total_turns.saturating_sub(self.protected_turns);
        turn_index >= protected_start
    }

    /// Calculate the turn index for each part in a session
    ///
    /// Returns a vector of (part_index, turn_index) pairs. Each `UserInput`
    /// part starts a new turn, and all subsequent parts belong to that turn
    /// until the next `UserInput`.
    ///
    /// # Arguments
    ///
    /// * `parts` - Slice of session parts to analyze
    ///
    /// # Returns
    ///
    /// A vector of tuples where each tuple contains:
    /// - `part_index`: The index of the part in the input slice
    /// - `turn_index`: The turn number this part belongs to (0-based)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Parts: [UserInput, AiResponse, ToolCall, UserInput, AiResponse]
    /// // Turns: [0,         0,          0,        1,         1        ]
    /// let indices = protector.calculate_turn_index(&parts);
    /// assert_eq!(indices, vec![(0, 0), (1, 0), (2, 0), (3, 1), (4, 1)]);
    /// ```
    pub fn calculate_turn_index(&self, parts: &[SessionPart]) -> Vec<(usize, usize)> {
        if parts.is_empty() {
            return Vec::new();
        }

        let mut result = Vec::with_capacity(parts.len());
        let mut current_turn: i32 = -1; // Start at -1 to handle first UserInput

        for (part_index, part) in parts.iter().enumerate() {
            // UserInput starts a new turn
            if matches!(part, SessionPart::UserInput(_)) {
                current_turn += 1;
            }

            // Parts before any UserInput belong to turn 0
            let turn_index = current_turn.max(0) as usize;
            result.push((part_index, turn_index));
        }

        result
    }

    /// Get the total number of turns from a list of session parts
    ///
    /// Counts the number of `UserInput` parts, which represent turn boundaries.
    ///
    /// # Arguments
    ///
    /// * `parts` - Slice of session parts to analyze
    ///
    /// # Returns
    ///
    /// The total number of turns (equivalent to the count of `UserInput` parts)
    pub fn count_turns(&self, parts: &[SessionPart]) -> usize {
        parts
            .iter()
            .filter(|p| matches!(p, SessionPart::UserInput(_)))
            .count()
    }

    /// Get the range of protected turn indices
    ///
    /// Returns a range representing which turn indices are protected.
    ///
    /// # Arguments
    ///
    /// * `total_turns` - Total number of turns in the session
    ///
    /// # Returns
    ///
    /// A `Range<usize>` where all indices in the range are protected.
    /// Returns an empty range if there are no protected turns.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let protector = TurnProtector::new(2);
    /// let range = protector.protected_range(5);
    /// assert_eq!(range, 3..5); // Turns 3 and 4 are protected
    /// ```
    pub fn protected_range(&self, total_turns: usize) -> Range<usize> {
        if total_turns == 0 || self.protected_turns == 0 {
            return 0..0;
        }

        let start = total_turns.saturating_sub(self.protected_turns);
        start..total_turns
    }

    /// Check if a part at the given index is protected
    ///
    /// This is a convenience method that calculates turn indices internally
    /// and checks if the part's turn is protected.
    ///
    /// # Arguments
    ///
    /// * `parts` - Slice of session parts
    /// * `part_index` - Index of the part to check
    ///
    /// # Returns
    ///
    /// `true` if the part belongs to a protected turn, `false` otherwise.
    /// Returns `false` if `part_index` is out of bounds.
    pub fn is_part_protected(&self, parts: &[SessionPart], part_index: usize) -> bool {
        if part_index >= parts.len() {
            return false;
        }

        let turn_indices = self.calculate_turn_index(parts);
        let total_turns = self.count_turns(parts);

        if let Some((_, turn_index)) = turn_indices.get(part_index) {
            self.is_protected(*turn_index, total_turns)
        } else {
            false
        }
    }

    /// Get all protected part indices from a session
    ///
    /// Returns a vector of part indices that belong to protected turns.
    ///
    /// # Arguments
    ///
    /// * `parts` - Slice of session parts
    ///
    /// # Returns
    ///
    /// A vector of part indices that are protected from compaction.
    pub fn protected_part_indices(&self, parts: &[SessionPart]) -> Vec<usize> {
        let turn_indices = self.calculate_turn_index(parts);
        let total_turns = self.count_turns(parts);

        turn_indices
            .into_iter()
            .filter(|(_, turn_index)| self.is_protected(*turn_index, total_turns))
            .map(|(part_index, _)| part_index)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{AiResponsePart, ToolCallPart, ToolCallStatus, UserInputPart};
    use serde_json::json;

    // =========================================================================
    // Constructor and Builder Tests
    // =========================================================================

    #[test]
    fn test_new() {
        let protector = TurnProtector::new(3);
        assert_eq!(protector.protected_turns(), 3);
    }

    #[test]
    fn test_default() {
        let protector = TurnProtector::default();
        assert_eq!(protector.protected_turns(), 2);
    }

    #[test]
    fn test_with_protected_turns() {
        let protector = TurnProtector::default().with_protected_turns(5);
        assert_eq!(protector.protected_turns(), 5);
    }

    // =========================================================================
    // is_protected Tests
    // =========================================================================

    #[test]
    fn test_is_protected_recent_turns() {
        let protector = TurnProtector::new(2);

        // With 5 total turns, turns 3 and 4 are protected (last 2)
        assert!(!protector.is_protected(0, 5)); // Not protected
        assert!(!protector.is_protected(1, 5)); // Not protected
        assert!(!protector.is_protected(2, 5)); // Not protected
        assert!(protector.is_protected(3, 5)); // Protected
        assert!(protector.is_protected(4, 5)); // Protected
    }

    #[test]
    fn test_is_protected_zero_turns() {
        let protector = TurnProtector::new(2);
        // Edge case: no turns
        assert!(!protector.is_protected(0, 0));
    }

    #[test]
    fn test_is_protected_zero_protection() {
        let protector = TurnProtector::new(0);
        // No protection configured
        assert!(!protector.is_protected(0, 5));
        assert!(!protector.is_protected(4, 5));
    }

    #[test]
    fn test_is_protected_all_protected() {
        let protector = TurnProtector::new(10);

        // When protected_turns > total_turns, all turns are protected
        assert!(protector.is_protected(0, 5));
        assert!(protector.is_protected(2, 5));
        assert!(protector.is_protected(4, 5));
    }

    #[test]
    fn test_is_protected_single_turn() {
        let protector = TurnProtector::new(1);

        // With 3 turns, only turn 2 is protected
        assert!(!protector.is_protected(0, 3));
        assert!(!protector.is_protected(1, 3));
        assert!(protector.is_protected(2, 3));
    }

    // =========================================================================
    // protected_range Tests
    // =========================================================================

    #[test]
    fn test_protected_range_normal() {
        let protector = TurnProtector::new(2);
        let range = protector.protected_range(5);
        assert_eq!(range, 3..5);
    }

    #[test]
    fn test_protected_range_zero_turns() {
        let protector = TurnProtector::new(2);
        let range = protector.protected_range(0);
        assert_eq!(range, 0..0);
    }

    #[test]
    fn test_protected_range_zero_protection() {
        let protector = TurnProtector::new(0);
        let range = protector.protected_range(5);
        assert_eq!(range, 0..0);
    }

    #[test]
    fn test_protected_range_all_protected() {
        let protector = TurnProtector::new(10);
        let range = protector.protected_range(5);
        assert_eq!(range, 0..5);
    }

    #[test]
    fn test_protected_range_exact_match() {
        let protector = TurnProtector::new(3);
        let range = protector.protected_range(3);
        assert_eq!(range, 0..3);
    }

    // =========================================================================
    // calculate_turn_index Tests
    // =========================================================================

    fn create_user_input(text: &str) -> SessionPart {
        SessionPart::UserInput(UserInputPart {
            text: text.to_string(),
            context: None,
            timestamp: 1000,
        })
    }

    fn create_ai_response(content: &str) -> SessionPart {
        SessionPart::AiResponse(AiResponsePart {
            content: content.to_string(),
            reasoning: None,
            timestamp: 1000,
        })
    }

    fn create_tool_call(name: &str) -> SessionPart {
        SessionPart::ToolCall(ToolCallPart {
            id: format!("call-{}", name),
            tool_name: name.to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output: Some("result".to_string()),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        })
    }

    #[test]
    fn test_calculate_turn_index_empty() {
        let protector = TurnProtector::new(2);
        let parts: Vec<SessionPart> = vec![];
        let indices = protector.calculate_turn_index(&parts);
        assert!(indices.is_empty());
    }

    #[test]
    fn test_calculate_turn_index_single_turn() {
        let protector = TurnProtector::new(2);
        let parts = vec![
            create_user_input("Hello"),
            create_ai_response("Hi there!"),
        ];

        let indices = protector.calculate_turn_index(&parts);

        assert_eq!(indices.len(), 2);
        assert_eq!(indices[0], (0, 0)); // UserInput at turn 0
        assert_eq!(indices[1], (1, 0)); // AiResponse at turn 0
    }

    #[test]
    fn test_calculate_turn_index_multiple_turns() {
        let protector = TurnProtector::new(2);
        let parts = vec![
            create_user_input("Hello"),
            create_ai_response("Hi!"),
            create_tool_call("search"),
            create_user_input("Search for X"),
            create_ai_response("Here are results"),
            create_user_input("Thanks"),
            create_ai_response("You're welcome"),
        ];

        let indices = protector.calculate_turn_index(&parts);

        assert_eq!(indices.len(), 7);
        // Turn 0: Hello + Hi! + search tool
        assert_eq!(indices[0], (0, 0));
        assert_eq!(indices[1], (1, 0));
        assert_eq!(indices[2], (2, 0));
        // Turn 1: Search for X + results
        assert_eq!(indices[3], (3, 1));
        assert_eq!(indices[4], (4, 1));
        // Turn 2: Thanks + welcome
        assert_eq!(indices[5], (5, 2));
        assert_eq!(indices[6], (6, 2));
    }

    #[test]
    fn test_calculate_turn_index_parts_before_user_input() {
        let protector = TurnProtector::new(2);
        // Edge case: parts before any UserInput (e.g., system initialization)
        let parts = vec![
            create_ai_response("System ready"), // No UserInput yet
            create_tool_call("init"),           // Still no UserInput
            create_user_input("Hello"),
            create_ai_response("Hi!"),
        ];

        let indices = protector.calculate_turn_index(&parts);

        assert_eq!(indices.len(), 4);
        // Parts before first UserInput belong to turn 0
        assert_eq!(indices[0], (0, 0));
        assert_eq!(indices[1], (1, 0));
        // First UserInput also at turn 0
        assert_eq!(indices[2], (2, 0));
        assert_eq!(indices[3], (3, 0));
    }

    // =========================================================================
    // count_turns Tests
    // =========================================================================

    #[test]
    fn test_count_turns_empty() {
        let protector = TurnProtector::new(2);
        let parts: Vec<SessionPart> = vec![];
        assert_eq!(protector.count_turns(&parts), 0);
    }

    #[test]
    fn test_count_turns_single() {
        let protector = TurnProtector::new(2);
        let parts = vec![
            create_user_input("Hello"),
            create_ai_response("Hi!"),
        ];
        assert_eq!(protector.count_turns(&parts), 1);
    }

    #[test]
    fn test_count_turns_multiple() {
        let protector = TurnProtector::new(2);
        let parts = vec![
            create_user_input("Hello"),
            create_ai_response("Hi!"),
            create_user_input("Question"),
            create_tool_call("search"),
            create_ai_response("Answer"),
            create_user_input("Thanks"),
        ];
        assert_eq!(protector.count_turns(&parts), 3);
    }

    #[test]
    fn test_count_turns_no_user_input() {
        let protector = TurnProtector::new(2);
        let parts = vec![
            create_ai_response("System message"),
            create_tool_call("init"),
        ];
        assert_eq!(protector.count_turns(&parts), 0);
    }

    // =========================================================================
    // is_part_protected Tests
    // =========================================================================

    #[test]
    fn test_is_part_protected_simple() {
        let protector = TurnProtector::new(1);
        let parts = vec![
            create_user_input("Hello"),      // Turn 0, part 0
            create_ai_response("Hi!"),       // Turn 0, part 1
            create_user_input("Question"),   // Turn 1, part 2
            create_ai_response("Answer"),    // Turn 1, part 3
        ];

        // With 2 turns and 1 protected, only turn 1 (parts 2,3) is protected
        assert!(!protector.is_part_protected(&parts, 0));
        assert!(!protector.is_part_protected(&parts, 1));
        assert!(protector.is_part_protected(&parts, 2));
        assert!(protector.is_part_protected(&parts, 3));
    }

    #[test]
    fn test_is_part_protected_out_of_bounds() {
        let protector = TurnProtector::new(2);
        let parts = vec![
            create_user_input("Hello"),
            create_ai_response("Hi!"),
        ];

        assert!(!protector.is_part_protected(&parts, 10)); // Out of bounds
    }

    #[test]
    fn test_is_part_protected_empty() {
        let protector = TurnProtector::new(2);
        let parts: Vec<SessionPart> = vec![];

        assert!(!protector.is_part_protected(&parts, 0));
    }

    // =========================================================================
    // protected_part_indices Tests
    // =========================================================================

    #[test]
    fn test_protected_part_indices_simple() {
        let protector = TurnProtector::new(1);
        let parts = vec![
            create_user_input("Hello"),      // Turn 0, part 0
            create_ai_response("Hi!"),       // Turn 0, part 1
            create_user_input("Question"),   // Turn 1, part 2
            create_ai_response("Answer"),    // Turn 1, part 3
        ];

        let protected = protector.protected_part_indices(&parts);

        // Only turn 1 (parts 2 and 3) should be protected
        assert_eq!(protected, vec![2, 3]);
    }

    #[test]
    fn test_protected_part_indices_all_protected() {
        let protector = TurnProtector::new(5);
        let parts = vec![
            create_user_input("Hello"),
            create_ai_response("Hi!"),
            create_user_input("Question"),
            create_ai_response("Answer"),
        ];

        let protected = protector.protected_part_indices(&parts);

        // All parts should be protected
        assert_eq!(protected, vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_protected_part_indices_none_protected() {
        let protector = TurnProtector::new(0);
        let parts = vec![
            create_user_input("Hello"),
            create_ai_response("Hi!"),
        ];

        let protected = protector.protected_part_indices(&parts);

        assert!(protected.is_empty());
    }

    #[test]
    fn test_protected_part_indices_empty() {
        let protector = TurnProtector::new(2);
        let parts: Vec<SessionPart> = vec![];

        let protected = protector.protected_part_indices(&parts);

        assert!(protected.is_empty());
    }

    // =========================================================================
    // Integration Tests with Complex Sessions
    // =========================================================================

    #[test]
    fn test_complex_session_protection() {
        // Simulate a realistic session with multiple turns and tool calls
        let protector = TurnProtector::new(2);
        let parts = vec![
            // Turn 0
            create_user_input("Find the config file"),
            create_tool_call("search"),
            create_tool_call("read_file"),
            create_ai_response("Found config at /app/config.toml"),
            // Turn 1
            create_user_input("Update the database setting"),
            create_tool_call("edit_file"),
            create_ai_response("Updated database.url"),
            // Turn 2
            create_user_input("Run the tests"),
            create_tool_call("bash"),
            create_ai_response("All tests passed"),
        ];

        let total_turns = protector.count_turns(&parts);
        assert_eq!(total_turns, 3);

        let protected_range = protector.protected_range(total_turns);
        assert_eq!(protected_range, 1..3); // Turns 1 and 2

        let protected_parts = protector.protected_part_indices(&parts);
        // Turn 1: indices 4, 5, 6
        // Turn 2: indices 7, 8, 9
        assert_eq!(protected_parts, vec![4, 5, 6, 7, 8, 9]);
    }
}
