//! Atomic Action Types
//!
//! Defines the seven atomic operations inspired by OpenClaw's Pi engine:
//! - Read: Read file content with optional line range
//! - Write: Write file content with mode control
//! - Edit: Incremental editing via patches (80-95% token savings)
//! - Bash: Execute shell commands
//! - Search: Semantic search with regex/fuzzy/AST support
//! - Replace: Batch replacement across files with preview
//! - Move: File/directory movement with import path updates

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Seven atomic operations (Pi engine style + extensions)
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

    /// Search files with pattern matching
    Search {
        pattern: SearchPattern,
        scope: SearchScope,
        #[serde(default)]
        filters: Vec<FileFilter>,
    },

    /// Replace text across files
    Replace {
        search: Box<SearchPattern>,
        replacement: String,
        scope: SearchScope,
        #[serde(default)]
        preview: bool,
        #[serde(default)]
        dry_run: bool,
    },

    /// Move file or directory
    Move {
        source: PathBuf,
        destination: PathBuf,
        #[serde(default)]
        update_imports: bool,
        #[serde(default)]
        create_parent: bool,
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
            AtomicAction::Search { .. } => None, // Operates on multiple files
            AtomicAction::Replace { .. } => None, // Operates on multiple files
            AtomicAction::Move { source, .. } => Some(source.to_str().unwrap_or("")),
        }
    }

    /// Get action type as string
    pub fn action_type(&self) -> &'static str {
        match self {
            AtomicAction::Read { .. } => "read",
            AtomicAction::Write { .. } => "write",
            AtomicAction::Edit { .. } => "edit",
            AtomicAction::Bash { .. } => "bash",
            AtomicAction::Search { .. } => "search",
            AtomicAction::Replace { .. } => "replace",
            AtomicAction::Move { .. } => "move",
        }
    }

    /// Check if this action is read-only
    pub fn is_read_only(&self) -> bool {
        matches!(self, AtomicAction::Read { .. } | AtomicAction::Search { .. })
    }

    /// Check if this action modifies the filesystem
    pub fn is_mutating(&self) -> bool {
        matches!(
            self,
            AtomicAction::Write { .. }
                | AtomicAction::Edit { .. }
                | AtomicAction::Replace { .. }
                | AtomicAction::Move { .. }
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
#[derive(Default)]
pub enum WriteMode {
    /// Overwrite existing file (default)
    #[default]
    Overwrite,
    /// Append to existing file
    Append,
    /// Create only if file doesn't exist (fail if exists)
    CreateOnly,
}


/// Search pattern types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchPattern {
    /// Regular expression pattern
    Regex { pattern: String },
    /// Fuzzy text matching
    Fuzzy { text: String, threshold: f32 },
    /// AST-level code search (language-aware)
    Ast { query: String, language: String },
}

/// Search scope
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchScope {
    /// Single file
    File { path: PathBuf },
    /// Directory (recursive)
    Directory { path: PathBuf, recursive: bool },
    /// Entire workspace
    Workspace,
}

/// File filters for search
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFilter {
    /// Only code files (common extensions)
    Code,
    /// Only text files
    Text,
    /// Only files matching extension
    Extension(String),
    /// Exclude files matching pattern
    Exclude(String),
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

    #[test]
    fn test_search_action() {
        let search = AtomicAction::Search {
            pattern: SearchPattern::Regex {
                pattern: r"TODO:.*".to_string(),
            },
            scope: SearchScope::Workspace,
            filters: vec![FileFilter::Code],
        };
        assert_eq!(search.action_type(), "search");
        assert!(search.is_read_only());
        assert!(!search.is_mutating());
    }

    #[test]
    fn test_replace_action() {
        let replace = AtomicAction::Replace {
            search: Box::new(SearchPattern::Regex {
                pattern: r"old_name".to_string(),
            }),
            replacement: "new_name".to_string(),
            scope: SearchScope::Workspace,
            preview: true,
            dry_run: false,
        };
        assert_eq!(replace.action_type(), "replace");
        assert!(!replace.is_read_only());
        assert!(replace.is_mutating());
    }

    #[test]
    fn test_move_action() {
        let move_action = AtomicAction::Move {
            source: PathBuf::from("src/old.rs"),
            destination: PathBuf::from("src/new.rs"),
            update_imports: true,
            create_parent: false,
        };
        assert_eq!(move_action.action_type(), "move");
        assert!(!move_action.is_read_only());
        assert!(move_action.is_mutating());
        assert!(move_action.file_path().is_some());
    }

    #[test]
    fn test_search_pattern_types() {
        let regex = SearchPattern::Regex {
            pattern: r"\d+".to_string(),
        };
        let fuzzy = SearchPattern::Fuzzy {
            text: "hello".to_string(),
            threshold: 0.8,
        };
        let ast = SearchPattern::Ast {
            query: "function_name".to_string(),
            language: "rust".to_string(),
        };

        // Test serialization
        let json = serde_json::to_string(&regex).unwrap();
        assert!(json.contains("\"type\":\"regex\""));

        let json = serde_json::to_string(&fuzzy).unwrap();
        assert!(json.contains("\"type\":\"fuzzy\""));

        let json = serde_json::to_string(&ast).unwrap();
        assert!(json.contains("\"type\":\"ast\""));
    }

    #[test]
    fn test_search_scope_types() {
        let file = SearchScope::File {
            path: PathBuf::from("test.rs"),
        };
        let dir = SearchScope::Directory {
            path: PathBuf::from("src/"),
            recursive: true,
        };
        let workspace = SearchScope::Workspace;

        // Test serialization
        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains("\"type\":\"file\""));

        let json = serde_json::to_string(&dir).unwrap();
        assert!(json.contains("\"type\":\"directory\""));
        assert!(json.contains("\"recursive\":true"));

        let json = serde_json::to_string(&workspace).unwrap();
        assert!(json.contains("\"type\":\"workspace\""));
    }

    #[test]
    fn test_file_filters() {
        let code = FileFilter::Code;
        let _text = FileFilter::Text;
        let ext = FileFilter::Extension("rs".to_string());
        let _exclude = FileFilter::Exclude("*.tmp".to_string());

        // Test serialization
        let json = serde_json::to_string(&code).unwrap();
        assert!(json.contains("\"code\""));

        let json = serde_json::to_string(&ext).unwrap();
        assert!(json.contains("\"rs\""));
    }
}
