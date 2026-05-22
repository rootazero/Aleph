//! Standalone `file_read` tool: a windowed, line-numbered, binary-aware reader.
//!
//! Reading is line-oriented end to end: `offset`/`limit` count *lines*, output
//! carries `cat -n`-style line numbers, and content is decoded with lossy
//! UTF-8. This removes a real panic in the previous byte-index implementation
//! (`&content[start..end]` could split a multi-byte character — a violation of
//! the project's UTF-8-safety redline) and matches how models expect to page
//! through files.

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::ops::read_file_bytes;
use super::path_utils::get_denied_paths;
use super::text::{is_binary, DEFAULT_READ_LINE_LIMIT, MAX_LINE_CHARS};
use crate::error::Result;
use crate::tools::AlephTool;

// =============================================================================
// Args & Output
// =============================================================================

/// Arguments for the file_read tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileReadArgs {
    /// Absolute or relative path to the file to read.
    pub path: String,

    /// 1-based line number to start reading from. Defaults to 1 (start of file).
    #[serde(default)]
    pub offset: Option<u64>,

    /// Maximum number of lines to return. Defaults to 2000.
    #[serde(default)]
    pub limit: Option<u64>,
}

/// Output returned by the file_read tool.
#[derive(Debug, Clone, Serialize)]
pub struct FileReadOutput {
    /// Whether the operation succeeded.
    pub success: bool,
    /// The resolved file path.
    pub path: String,
    /// File content with `cat -n`-style 1-based line numbers. Empty for binary
    /// files and for `offset` values past the end of the file.
    pub content: String,
    /// Total byte size of the file on disk.
    pub size: u64,
    /// Total number of lines in the file.
    pub total_lines: u64,
    /// Number of lines included in `content`.
    pub returned_lines: u64,
    /// True when `content` is a partial view (more lines exist past the window).
    pub truncated: bool,
    /// Human-readable result message.
    pub message: String,
}

// =============================================================================
// FileReadTool
// =============================================================================

/// Standalone tool for reading file contents.
pub struct FileReadTool {
    /// Maximum file size allowed for read operations (default 100 MB).
    max_read_size: u64,
    /// Security-denied path patterns.
    denied_paths: Vec<String>,
    /// Optional ToolContext handle for workspace-scoped output path resolution.
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FileReadTool {
    /// Create a new FileReadTool with default settings.
    pub fn new() -> Self {
        let denied_paths = get_denied_paths();
        info!(
            denied_paths_count = denied_paths.len(),
            "FileReadTool: initialized with denied_paths"
        );

        Self {
            max_read_size: 100 * 1024 * 1024, // 100 MB
            denied_paths,
            tool_context_handle: None,
        }
    }

    /// Configure the tool to use a ToolContext handle for workspace-scoped output paths.
    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    /// Resolve the output directory from the ToolContext handle (if available).
    async fn resolve_output_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            Some(ctx.output_dir.join("documents"))
        } else {
            None
        }
    }
}

impl Default for FileReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FileReadTool {
    fn clone(&self) -> Self {
        Self {
            max_read_size: self.max_read_size,
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

// =============================================================================
// Rendering (pure helpers — independently testable)
// =============================================================================

/// Build the line-numbered window described by `args` from decoded `text`.
fn render_window(text: &str, args: &FileReadArgs, size: u64, path: String) -> FileReadOutput {
    let lines: Vec<&str> = text.lines().collect();
    let total_lines = lines.len() as u64;

    if lines.is_empty() {
        return FileReadOutput {
            success: true,
            path,
            content: String::new(),
            size,
            total_lines: 0,
            returned_lines: 0,
            truncated: false,
            message: "File is empty.".to_string(),
        };
    }

    // `offset` is 1-based; absent or 0 both mean "from the first line".
    let start = args.offset.unwrap_or(1).max(1) as usize - 1;
    let limit = args.limit.unwrap_or(DEFAULT_READ_LINE_LIMIT).max(1) as usize;

    if start >= lines.len() {
        return FileReadOutput {
            success: true,
            path,
            content: String::new(),
            size,
            total_lines,
            returned_lines: 0,
            truncated: false,
            message: format!(
                "offset {} is past the end of the file ({total_lines} lines).",
                start + 1
            ),
        };
    }

    let end = start.saturating_add(limit).min(lines.len());
    let width = end.to_string().len();

    let mut rendered = String::new();
    for (idx, line) in lines[start..end].iter().enumerate() {
        let lineno = start + idx + 1;
        rendered.push_str(&format!("{lineno:>width$}\t{}\n", clamp_line(line)));
    }

    let truncated = end < lines.len();
    let message = if truncated {
        format!(
            "Showing lines {}-{end} of {total_lines}. Pass offset={} to continue.",
            start + 1,
            end + 1
        )
    } else if start > 0 {
        format!("Showing lines {}-{end} of {total_lines}.", start + 1)
    } else {
        format!("Read {total_lines} lines ({size} bytes).")
    };

    FileReadOutput {
        success: true,
        path,
        content: rendered,
        size,
        total_lines,
        returned_lines: (end - start) as u64,
        truncated,
        message,
    }
}

/// Clamp a single line to [`MAX_LINE_CHARS`] characters (char-boundary safe),
/// appending a marker when truncated — guards against minified one-line files.
fn clamp_line(line: &str) -> String {
    let char_count = line.chars().count();
    if char_count <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let head: String = line.chars().take(MAX_LINE_CHARS).collect();
    format!("{head}… [line truncated — {char_count} chars total]")
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for FileReadTool {
    const NAME: &'static str = "file_read";
    const DESCRIPTION: &'static str =
        "Read a file's contents as `cat -n`-style numbered text lines. Returns up \
         to 2000 lines from the start by default; use `offset` (1-based starting \
         line) and `limit` (max lines) to page through larger files. Binary files \
         are detected and reported rather than dumped. The line-number prefixes \
         are for reference only — strip them before passing text to file_edit.";

    type Args = FileReadArgs;
    type Output = FileReadOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        notify_tool_start(Self::NAME, &format!("read: {}", &args.path));

        let path = Path::new(&args.path);
        let output_dir = self.resolve_output_dir().await;

        let (canonical, size, bytes) = read_file_bytes(
            path,
            &self.denied_paths,
            self.max_read_size,
            output_dir.as_deref(),
        )
        .await
        .inspect_err(|e| notify_tool_result(Self::NAME, &e.to_string(), false))?;
        let path_str = canonical.display().to_string();

        // Binary files degrade gracefully: a non-error result the model can act
        // on, instead of a hard `read_to_string` failure or a corrupt dump.
        if is_binary(&bytes) {
            let message = format!("Binary file — {size} bytes, content not displayable.");
            notify_tool_result(Self::NAME, &message, true);
            return Ok(FileReadOutput {
                success: true,
                path: path_str,
                content: String::new(),
                size,
                total_lines: 0,
                returned_lines: 0,
                truncated: false,
                message,
            });
        }

        // Lossy decode: a few stray bytes degrade to U+FFFD rather than failing
        // the whole read.
        let text = String::from_utf8_lossy(&bytes);
        let output = render_window(&text, &args, size, path_str);

        notify_tool_result(Self::NAME, &output.message, output.success);
        Ok(output)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn args(path: &Path, offset: Option<u64>, limit: Option<u64>) -> FileReadArgs {
        FileReadArgs {
            path: path.to_string_lossy().to_string(),
            offset,
            limit,
        }
    }

    #[tokio::test]
    async fn reads_with_line_numbers() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "first\nsecond\nthird\n").unwrap();

        let out = AlephTool::call(&FileReadTool::new(), args(&file, None, None))
            .await
            .unwrap();
        assert!(out.success);
        assert_eq!(out.total_lines, 3);
        assert_eq!(out.returned_lines, 3);
        assert!(!out.truncated);
        assert!(out.content.contains("1\tfirst"));
        assert!(out.content.contains("3\tthird"));
    }

    #[tokio::test]
    async fn offset_and_limit_window() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "l1\nl2\nl3\nl4\nl5\n").unwrap();

        let out = AlephTool::call(&FileReadTool::new(), args(&file, Some(2), Some(2)))
            .await
            .unwrap();
        assert_eq!(out.returned_lines, 2);
        assert!(out.truncated, "lines past the window remain");
        assert!(out.content.contains("2\tl2"));
        assert!(out.content.contains("3\tl3"));
        assert!(!out.content.contains("l1"));
        assert!(!out.content.contains("l4"));
    }

    #[tokio::test]
    async fn offset_past_eof_is_graceful() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "only one line\n").unwrap();

        let out = AlephTool::call(&FileReadTool::new(), args(&file, Some(99), None))
            .await
            .unwrap();
        assert!(out.success);
        assert_eq!(out.returned_lines, 0);
        assert!(out.content.is_empty());
        assert!(out.message.contains("past the end"));
    }

    #[tokio::test]
    async fn empty_file_is_reported() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("empty.txt");
        fs::write(&file, "").unwrap();

        let out = AlephTool::call(&FileReadTool::new(), args(&file, None, None))
            .await
            .unwrap();
        assert!(out.success);
        assert_eq!(out.total_lines, 0);
        assert!(out.message.contains("empty"));
    }

    #[tokio::test]
    async fn binary_file_is_reported_gracefully() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("blob.bin");
        fs::write(&file, [0x89, 0x50, 0x4E, 0x47, 0x00, 0x01, 0x02]).unwrap();

        let out = AlephTool::call(&FileReadTool::new(), args(&file, None, None))
            .await
            .unwrap();
        assert!(out.success, "binary read is not an error");
        assert!(out.content.is_empty());
        assert!(out.message.contains("Binary"));
    }

    #[tokio::test]
    async fn multibyte_content_never_panics() {
        // Regression: the previous byte-index slicing panicked when offset/limit
        // split a multi-byte character. Line-based windowing is char-safe.
        let dir = tempdir().unwrap();
        let file = dir.path().join("utf8.txt");
        fs::write(&file, "日本語\nالعربية\némoji 🎉\n").unwrap();

        let out = AlephTool::call(&FileReadTool::new(), args(&file, Some(2), Some(1)))
            .await
            .unwrap();
        assert_eq!(out.returned_lines, 1);
        assert!(out.content.contains("العربية"));
    }

    #[test]
    fn long_lines_are_clamped() {
        let clamped = clamp_line(&"x".repeat(MAX_LINE_CHARS + 500));
        assert!(clamped.contains("line truncated"));
        assert!(clamped.starts_with(&"x".repeat(MAX_LINE_CHARS)));
    }

    #[test]
    fn render_window_handles_no_trailing_newline() {
        let out = render_window(
            "solo",
            &FileReadArgs {
                path: String::new(),
                offset: None,
                limit: None,
            },
            4,
            String::new(),
        );
        assert_eq!(out.total_lines, 1);
        assert!(out.content.contains("1\tsolo"));
    }
}
