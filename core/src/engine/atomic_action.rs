//! Atomic Action Types
//!
//! Defines the four atomic operations inspired by OpenClaw's Pi engine:
//! - Read: Read file content with optional line range
//! - Write: Write file content with mode control
//! - Edit: Incremental editing via patches (80-95% token savings)
//! - Bash: Execute shell commands

use serde::{Deserialize, Serialize};

/// Four atomic operations (Pi engine style)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AtomicAction {
    /// Read file (supports line range)
    Read {
        path: String,
        #[serde(default)]
        range: Option<LineRange>,
    },

    /// Write file (supports overwrite/append)
    Write {
        path: String,
        content: String,
        #[serde(default)]
        mode: WriteMode,
    },

    /// Incremental edit (core innovation)
    Edit {
        path: String,
        patches: Vec<super::Patch>,
    },

    /// Execute shell command
    Bash {
        command: String,
        #[serde(default)]
        cwd: Option<String>,
    },
}

impl AtomicAction {
    /// Extract file path if this action operates on a file
    pub fn file_path(&self) -> Option<&str> {
        match self {
            AtomicAction::Read { path, .. } => Some(path),
            AtomicAction::Write { path, .. } => Some(path),
            AtomicAction::Edit { path, .. } => Some(path),
            AtomicAction::Bash { .. } => None,
        }
    }

    /// Get action type as string
    pub fn action_type(&self) -> &'static str {
        match self {
            AtomicAction::Read { .. } => "read",
            AtomicAction::Write { .. } => "write",
            AtomicAction::Edit { .. } => "edit",
            AtomicAction::Bash { .. } => "bash",
        }
    }

    /// Check if this action is read-only
    pub fn is_read_only(&self) -> bool {
        matches!(self, AtomicAction::Read { .. })
    }

    /// Check if this action modifies the filesystem
    pub fn is_mutating(&self) -> bool {
        matches!(
            self,
            AtomicAction::Write { .. } | AtomicAction::Edit { .. }
        )
    }
}

/// Line range for Read operation (1-indexed, inclusive)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    /// Start line (1-indexed)
    pub start: usize,
    /// End line (1-indexed, inclusive)
    pub end: usize,
}

impl LineRange {
    /// Create a new line range
    pub fn new(start: usize, end: usize) -> Result<Self, String> {
        if start == 0 {
            return Err("Line numbers are 1-indexed, start cannot be 0".to_string());
        }
        if end < start {
            return Err(format!("Invalid range: end ({}) < start ({})", end, start));
        }
        Ok(Self { start, end })
    }

    /// Get the number of lines in this range
    pub fn len(&self) -> usize {
        self.end - self.start + 1
    }

    /// Check if this range is empty (should never happen with valid construction)
    pub fn is_empty(&self) -> bool {
        self.end < self.start
    }

    /// Check if this range contains a specific line number
    pub fn contains(&self, line: usize) -> bool {
        line >= self.start && line <= self.end
    }

    /// Check if this range overlaps with another range
    pub fn overlaps(&self, other: &LineRange) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// Write mode for Write operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    /// Overwrite existing file (default)
    Overwrite,
    /// Append to existing file
    Append,
    /// Create only if file doesn't exist (fail if exists)
    CreateOnly,
}

impl Default for WriteMode {
    fn default() -> Self {
        Self::Overwrite
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_action_file_path() {
        let read = AtomicAction::Read {
            path: "test.txt".to_string(),
            range: None,
        };
        assert_eq!(read.file_path(), Some("test.txt"));

        let bash = AtomicAction::Bash {
            command: "ls".to_string(),
            cwd: None,
        };
        assert_eq!(bash.file_path(), None);
    }

    #[test]
    fn test_atomic_action_type() {
        let read = AtomicAction::Read {
            path: "test.txt".to_string(),
            range: None,
        };
        assert_eq!(read.action_type(), "read");

        let edit = AtomicAction::Edit {
            path: "test.txt".to_string(),
            patches: vec![],
        };
        assert_eq!(edit.action_type(), "edit");
    }

    #[test]
    fn test_atomic_action_is_read_only() {
        let read = AtomicAction::Read {
            path: "test.txt".to_string(),
            range: None,
        };
        assert!(read.is_read_only());

        let write = AtomicAction::Write {
            path: "test.txt".to_string(),
            content: "content".to_string(),
            mode: WriteMode::Overwrite,
        };
        assert!(!write.is_read_only());
    }

    #[test]
    fn test_atomic_action_is_mutating() {
        let read = AtomicAction::Read {
            path: "test.txt".to_string(),
            range: None,
        };
        assert!(!read.is_mutating());

        let edit = AtomicAction::Edit {
            path: "test.txt".to_string(),
            patches: vec![],
        };
        assert!(edit.is_mutating());
    }

    #[test]
    fn test_line_range_new() {
        let range = LineRange::new(1, 10).unwrap();
        assert_eq!(range.start, 1);
        assert_eq!(range.end, 10);

        // Test invalid ranges
        assert!(LineRange::new(0, 10).is_err()); // start = 0
        assert!(LineRange::new(10, 5).is_err()); // end < start
    }

    #[test]
    fn test_line_range_len() {
        let range = LineRange::new(1, 10).unwrap();
        assert_eq!(range.len(), 10);

        let range = LineRange::new(5, 5).unwrap();
        assert_eq!(range.len(), 1);
    }

    #[test]
    fn test_line_range_contains() {
        let range = LineRange::new(5, 10).unwrap();
        assert!(!range.contains(4));
        assert!(range.contains(5));
        assert!(range.contains(7));
        assert!(range.contains(10));
        assert!(!range.contains(11));
    }

    #[test]
    fn test_line_range_overlaps() {
        let range1 = LineRange::new(5, 10).unwrap();
        let range2 = LineRange::new(8, 15).unwrap();
        let range3 = LineRange::new(11, 20).unwrap();

        assert!(range1.overlaps(&range2)); // 5-10 overlaps 8-15
        assert!(range2.overlaps(&range1)); // symmetric
        assert!(!range1.overlaps(&range3)); // 5-10 doesn't overlap 11-20
    }

    #[test]
    fn test_atomic_action_serialization() {
        let action = AtomicAction::Read {
            path: "test.txt".to_string(),
            range: Some(LineRange::new(1, 10).unwrap()),
        };

        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("\"type\":\"read\""));
        assert!(json.contains("\"path\":\"test.txt\""));

        let deserialized: AtomicAction = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, action);
    }

    #[test]
    fn test_write_mode_default() {
        assert_eq!(WriteMode::default(), WriteMode::Overwrite);
    }
}
