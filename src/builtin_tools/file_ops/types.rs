//! Type definitions for file operations

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// File operation type
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileOperation {
    /// List directory contents
    List,
    /// Move/rename file or directory
    Move,
    /// Copy file or directory
    Copy,
    /// Delete file or directory
    Delete,
    /// Create directory
    Mkdir,
    /// Search files by glob pattern
    Search,
    /// Batch move files matching a pattern to destination
    BatchMove,
    /// Auto-organize files by type into categorized folders
    Organize,
    /// Recursive stats: per-file line/byte counts plus an aggregate summary.
    /// Use this instead of looping over `read` to count lines.
    Stats,
}

/// Arguments for file operations tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileOpsArgs {
    /// The operation to perform
    pub operation: FileOperation,
    /// Primary path (source path for move/copy, target path for others)
    pub path: String,
    /// Destination path (for move/copy operations)
    #[serde(default)]
    pub destination: Option<String>,
    /// Search pattern (for search/stats operation, glob syntax). Stats defaults to "**/*".
    #[serde(default)]
    pub pattern: Option<String>,
    /// Create parent directories if they don't exist
    #[serde(default = "default_true")]
    pub create_parents: bool,
    /// Max entries returned by `list` / `search` / `stats` (default 500). The
    /// result says so when the cap is reached; raise it or narrow `pattern`.
    #[serde(default)]
    pub limit: Option<usize>,
}

const fn default_true() -> bool {
    true
}

/// Default cap on entries returned by the listing operations.
///
/// These collections were unbounded. `list` on a build directory, or the
/// `stats` call the tool's own description recommends for "how many lines are in
/// this directory", could materialise tens of thousands of `FileInfo` rows —
/// megabytes of JSON, far past any tool's token budget, at which point the whole
/// answer was replaced by a persist marker. The four aggregate numbers `stats`
/// had just computed never reached the model at all. Same order of magnitude as
/// pi's `ls` (500) and `find` (1000) caps, which exist for the same reason.
pub const DEFAULT_ENTRY_LIMIT: usize = 500;

/// Directory names skipped by the recursive `search` / `stats` walks unless the
/// caller's pattern names them explicitly.
///
/// A glob walk that descends into `target/`, `node_modules/` and `.git/` spends
/// its entire entry budget on generated files, so the answer is both enormous and
/// wrong — the model asked about the project, not its build cache. pi gets this
/// from `.gitignore`; a fixed list costs no dependency (R3) and covers the cases
/// that actually blow up. Naming a directory in the pattern still reaches it, so
/// `pattern: "target/**/*.rs"` is not silently empty.
pub const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cargo",
    "vendor",
    ".mypy_cache",
    ".pytest_cache",
];

/// Whether `path` sits inside a [`SKIPPED_DIRS`] directory **below `root`** that
/// `pattern` does not mention. Mentioning it anywhere in the pattern opts back in.
///
/// Only components below `root` are examined, and that is the whole subtlety: the
/// paths a glob walk yields are absolute, so testing every component made the
/// answer depend on where the project happens to live. A workspace at
/// `~/src/build/myapp` or any path containing `vendor` / `dist` / `target` as an
/// *ancestor* matched on that ancestor and every single entry was skipped — the
/// tool would report zero matches for a directory full of files. `root` is the
/// caller's already-canonicalized search root, i.e. the thing the user pointed at;
/// what is above it is not theirs to be judged by.
#[must_use]
pub fn is_skipped_dir_path(root: &std::path::Path, path: &std::path::Path, pattern: &str) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        SKIPPED_DIRS.contains(&name.as_ref()) && !pattern.contains(name.as_ref())
    })
}

/// File metadata in output
#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    /// Newline-terminated line count. Only populated by `stats`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<u64>,
}

/// Aggregate counts produced by the `stats` operation.
#[derive(Debug, Clone, Serialize)]
pub struct StatsSummary {
    pub total_files: usize,
    pub total_lines: u64,
    pub total_bytes: u64,
    /// Files that matched the pattern but were skipped for line counting
    /// (binary, too large, or unreadable).
    pub skipped_files: usize,
}

/// Output from file operations tool
#[derive(Debug, Clone, Serialize)]
pub struct FileOpsOutput {
    pub success: bool,
    pub operation: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_written: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items_affected: Option<usize>,
    /// Aggregate summary, populated only by `stats`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<StatsSummary>,
}
